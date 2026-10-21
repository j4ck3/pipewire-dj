//! PipeWire thread: owns the main loop, the registry view of the graph, and all streams.

use super::mixer::{Mixer, CH, RATE};
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// `node.name` of the virtual microphone.
pub const SOURCE_NAME: &str = "pipewire_dj";
pub const SOURCE_DESC: &str = "PipeWire DJ";

pub enum PwCmd {
    SetMic(Option<String>),
    SetMonitor { enabled: bool, target: Option<String> },
    /// Capture exactly this set of app stream nodes (by object.serial).
    SetApps(Vec<u32>),
    Quit,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Node {
    pub id: u32,
    pub serial: u32,
    pub name: String,
    pub description: String,
    pub class: String,
    pub app: String,
    pub pid: Option<u32>,
}

pub type Graph = Arc<Mutex<Vec<Node>>>;

struct Live {
    stream: pw::stream::StreamRc,
    _listener: pw::stream::StreamListener<()>,
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.stream.disconnect();
    }
}

#[derive(Default)]
struct State {
    mic: Option<Live>,
    mic_target: Option<String>,
    monitor: Option<Live>,
    monitor_cfg: Option<(bool, Option<String>)>,
    apps: HashMap<u32, Live>,
}

fn format_param() -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE as u32);
    info.set_channels(CH as u32);
    let mut pos = [0; spa::param::audio::MAX_CHANNELS];
    pos[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    pos[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(pos);
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .expect("serialize format")
    .0
    .into_inner()
}

#[derive(Clone, Copy)]
enum Out {
    Source,
    Monitor,
}

fn output_stream(
    core: &pw::core::CoreRc,
    name: &str,
    props: pw::properties::PropertiesBox,
    mixer: Arc<Mutex<Mixer>>,
    kind: Out,
    autoconnect: bool,
) -> Result<Live, pw::Error> {
    let stream = pw::stream::StreamRc::new(core.clone(), name, props)?;
    let listener = stream
        .add_local_listener_with_user_data(())
        .process(move |s, _| {
            let Some(mut b) = s.dequeue_buffer() else { return };
            let req = b.requested() as usize;
            let d = &mut b.datas_mut()[0];
            let mut frames = 0;
            if let Some(sl) = d.data() {
                let max = sl.len() / (CH * 4);
                frames = if req > 0 { req.min(max) } else { max };
                let mut buf = vec![0f32; frames * CH];
                {
                    let mut m = mixer.lock().unwrap();
                    match kind {
                        Out::Source => m.on_source(&mut buf),
                        Out::Monitor => m.on_monitor(&mut buf),
                    }
                }
                for (i, v) in buf.iter().enumerate() {
                    sl[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            let c = d.chunk_mut();
            *c.offset_mut() = 0;
            *c.stride_mut() = (CH * 4) as i32;
            *c.size_mut() = (frames * CH * 4) as u32;
        })
        .register()?;
    let p = format_param();
    let mut flags = pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS;
    if autoconnect {
        flags |= pw::stream::StreamFlags::AUTOCONNECT;
    }
    stream.connect(spa::utils::Direction::Output, None, flags, &mut [Pod::from_bytes(&p).unwrap()])?;
    Ok(Live { stream, _listener: listener })
}

#[derive(Clone, Copy)]
enum Cap {
    Mic,
    App(u32),
}

fn input_stream(
    core: &pw::core::CoreRc,
    name: &str,
    props: pw::properties::PropertiesBox,
    mixer: Arc<Mutex<Mixer>>,
    cap: Cap,
) -> Result<Live, pw::Error> {
    let stream = pw::stream::StreamRc::new(core.clone(), name, props)?;
    let listener = stream
        .add_local_listener_with_user_data(())
        .process(move |s, _| {
            let Some(mut b) = s.dequeue_buffer() else { return };
            let d = &mut b.datas_mut()[0];
            let off = d.chunk().offset() as usize;
            let size = d.chunk().size() as usize;
            if let Some(sl) = d.data() {
                let end = (off + size).min(sl.len());
                let bytes = &sl[off.min(end)..end];
                let buf: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                let mut m = mixer.lock().unwrap();
                match cap {
                    Cap::Mic => m.on_mic(&buf),
                    Cap::App(k) => m.push_app(k, &buf),
                }
            }
        })
        .register()?;
    let p = format_param();
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut [Pod::from_bytes(&p).unwrap()],
    )?;
    Ok(Live { stream, _listener: listener })
}

/// Start the PipeWire thread. Returns once the virtual source exists (or fails).
pub fn spawn(mixer: Arc<Mutex<Mixer>>, graph: Graph) -> anyhow::Result<pw::channel::Sender<PwCmd>> {
    let (tx, rx) = pw::channel::channel::<PwCmd>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new().name("pipewire".into()).spawn(move || {
        if let Err(e) = run(mixer, graph, rx, &ready_tx) {
            let _ = ready_tx.send(Err(e.to_string()));
        }
    })?;
    ready_rx.recv()?.map_err(anyhow::Error::msg)?;
    Ok(tx)
}

fn run(
    mixer: Arc<Mutex<Mixer>>,
    graph: Graph,
    rx: pw::channel::Receiver<PwCmd>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), pw::Error> {
    pw::init();
    let ml = pw::main_loop::MainLoopRc::new(None)?;
    let ctx = pw::context::ContextRc::new(&ml, None)?;
    let core = ctx.connect_rc(None)?;
    let registry = core.get_registry_rc()?;

    let (g_add, g_rm) = (graph.clone(), graph.clone());
    let _reg = registry
        .add_listener_local()
        .global(move |o| {
            if o.type_ != pw::types::ObjectType::Node {
                return;
            }
            let Some(p) = o.props else { return };
            let get = |k: &str| p.get(k).unwrap_or("").to_string();
            let mut description = get("node.description");
            if description.is_empty() {
                description = get("node.nick");
            }
            if description.is_empty() {
                description = get("media.name");
            }
            let node = Node {
                id: o.id,
                serial: get("object.serial").parse().unwrap_or(0),
                name: get("node.name"),
                description,
                class: get("media.class"),
                app: get("application.name"),
                pid: p.get("application.process.id").and_then(|s| s.parse().ok()),
            };
            let mut g = g_add.lock().unwrap();
            g.retain(|n| n.id != o.id);
            g.push(node);
        })
        .global_remove(move |id| {
            g_rm.lock().unwrap().retain(|n| n.id != id);
        })
        .register();

    // The virtual microphone. ("Audio/Source/Virtual" crashes audioconvert on
    // PipeWire 1.6.8 for client streams; plain "Audio/Source" behaves the same for apps.)
    let _source = output_stream(
        &core,
        SOURCE_DESC,
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CLASS => "Audio/Source",
            *pw::keys::NODE_NAME => SOURCE_NAME,
            *pw::keys::NODE_DESCRIPTION => SOURCE_DESC,
            *pw::keys::NODE_LATENCY => "256/48000",
        },
        mixer.clone(),
        Out::Source,
        false,
    )?;

    let state = Rc::new(RefCell::new(State::default()));
    let weak_ml = ml.downgrade();
    let core2 = core.clone();
    let _rx = rx.attach(ml.loop_(), move |cmd| {
        let mut st = state.borrow_mut();
        let res: Result<(), pw::Error> = (|| {
            match cmd {
                PwCmd::Quit => {
                    st.apps.clear();
                    st.mic = None;
                    st.monitor = None;
                    if let Some(ml) = weak_ml.upgrade() {
                        ml.quit();
                    }
                }
                PwCmd::SetMic(target) => {
                    if st.mic_target == target && st.mic.is_some() {
                        return Ok(());
                    }
                    st.mic = None;
                    st.mic_target = target.clone();
                    if let Some(t) = target {
                        let props = properties! {
                            *pw::keys::MEDIA_TYPE => "Audio",
                            *pw::keys::MEDIA_CATEGORY => "Capture",
                            *pw::keys::MEDIA_ROLE => "Communication",
                            *pw::keys::NODE_NAME => "pipewire_dj.mic",
                            *pw::keys::NODE_DESCRIPTION => "PipeWire DJ mic input",
                            *pw::keys::TARGET_OBJECT => t,
                            *pw::keys::NODE_LATENCY => "256/48000",
                            // Never fall back to the default source: that is us.
                            "node.dont-reconnect" => "true",
                            "node.dont-fallback" => "true",
                            "node.dont-move" => "true",
                        };
                        st.mic = Some(input_stream(&core2, "pipewire-dj-mic", props, mixer.clone(), Cap::Mic)?);
                    }
                }
                PwCmd::SetMonitor { enabled, target } => {
                    let cfg = Some((enabled, target.clone()));
                    if st.monitor_cfg == cfg {
                        return Ok(());
                    }
                    st.monitor_cfg = cfg;
                    st.monitor = None;
                    if enabled {
                        let mut props = properties! {
                            *pw::keys::MEDIA_TYPE => "Audio",
                            *pw::keys::MEDIA_CATEGORY => "Playback",
                            *pw::keys::MEDIA_ROLE => "Music",
                            *pw::keys::NODE_NAME => "pipewire_dj.monitor",
                            *pw::keys::NODE_DESCRIPTION => "PipeWire DJ monitor",
                            *pw::keys::NODE_LATENCY => "256/48000",
                        };
                        if let Some(t) = target {
                            props.insert(*pw::keys::TARGET_OBJECT, t);
                        }
                        st.monitor = Some(output_stream(&core2, "pipewire-dj-monitor", props, mixer.clone(), Out::Monitor, true)?);
                    }
                }
                PwCmd::SetApps(serials) => {
                    let gone: Vec<u32> = st.apps.keys().filter(|k| !serials.contains(k)).copied().collect();
                    for k in gone {
                        st.apps.remove(&k);
                        mixer.lock().unwrap().remove_app(k);
                    }
                    for k in serials {
                        if st.apps.contains_key(&k) {
                            continue;
                        }
                        mixer.lock().unwrap().add_app(k);
                        let props = properties! {
                            *pw::keys::MEDIA_TYPE => "Audio",
                            *pw::keys::MEDIA_CATEGORY => "Capture",
                            *pw::keys::NODE_NAME => format!("pipewire_dj.app.{k}"),
                            *pw::keys::NODE_DESCRIPTION => "PipeWire DJ app capture",
                            *pw::keys::TARGET_OBJECT => k.to_string(),
                            "node.dont-reconnect" => "true",
                            "node.dont-fallback" => "true",
                            "node.dont-move" => "true",
                        };
                        let live = input_stream(&core2, "pipewire-dj-app", props, mixer.clone(), Cap::App(k))?;
                        st.apps.insert(k, live);
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            eprintln!("pipewire-dj: stream error: {e}");
        }
    });

    let _ = ready.send(Ok(()));
    ml.run();
    Ok(())
}
