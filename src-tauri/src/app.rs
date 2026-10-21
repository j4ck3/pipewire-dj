//! Application core shared by the UI commands, the CLI socket, hotkeys and game adapters.

use crate::engine::mixer::{GateMode, LoadedTrack, Meters, MixParams, Mixer, PlaybackStatus, RATE};
use crate::engine::pw::{self as epw, Graph, Node, PwCmd};
use crate::engine::defaults;
use crate::games::cs2;
use crate::library::{self, Library, Paths, Track};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// node.name of the real mic; None = the system default at startup.
    pub mic_device: Option<String>,
    pub monitor_enabled: bool,
    pub monitor_device: Option<String>,
    pub set_default_source: bool,
    /// application.name values whose output is mixed into the mic.
    pub apps: Vec<String>,
    pub mic_volume: f32,
    pub mic_mute: bool,
    pub denoise: bool,
    pub track_volume: f32,
    pub apps_volume: f32,
    pub monitor_volume: f32,
    pub pause_on_toggle: bool,
    pub gate_mode: GateMode,
    pub open_mic_while_track: bool,
    pub limiter: bool,
    pub cs2_enabled: bool,
    pub cs2_play_key: String,
    /// Key that should act as push-to-talk through the app; None = detect from CS2 config.
    pub cs2_ptt_key: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let m = MixParams::default();
        Self {
            mic_device: None,
            monitor_enabled: true,
            monitor_device: None,
            set_default_source: true,
            apps: vec![],
            mic_volume: m.mic_volume,
            mic_mute: m.mic_mute,
            denoise: m.denoise,
            track_volume: m.track_volume,
            apps_volume: m.apps_volume,
            monitor_volume: m.monitor_volume,
            pause_on_toggle: m.pause_on_toggle,
            gate_mode: m.gate_mode,
            open_mic_while_track: m.open_mic_while_track,
            limiter: m.limiter,
            cs2_enabled: true,
            cs2_play_key: "kp_enter".into(),
            cs2_ptt_key: None,
        }
    }
}

impl Settings {
    fn mix(&self) -> MixParams {
        MixParams {
            mic_volume: self.mic_volume,
            mic_mute: self.mic_mute,
            denoise: self.denoise,
            track_volume: self.track_volume,
            apps_volume: self.apps_volume,
            monitor_volume: self.monitor_volume,
            pause_on_toggle: self.pause_on_toggle,
            gate_mode: self.gate_mode,
            open_mic_while_track: self.open_mic_while_track,
            limiter: self.limiter,
        }
    }
}

#[derive(Clone, Serialize, Default)]
pub struct GameStatus {
    pub install: Option<String>,
    pub running: bool,
    pub log: Option<String>,
    pub log_active: bool,
    pub last_event: Option<String>,
    pub ptt_key: Option<String>,
    pub launch_options: String,
    pub cfg_written: bool,
    pub ptt: bool,
}

#[derive(Clone, Serialize)]
pub struct Status {
    pub playback: PlaybackStatus,
    pub meters: Meters,
    pub ptt: bool,
    pub mic_open: bool,
    pub game: GameStatus,
}

pub struct App {
    pub paths: Paths,
    pub settings: Mutex<Settings>,
    pub library: Mutex<Library>,
    pub mixer: Arc<Mutex<Mixer>>,
    pub graph: Graph,
    pw: Mutex<Option<pipewire::channel::Sender<PwCmd>>>,
    cache: Mutex<HashMap<u64, Arc<Vec<f32>>>>,
    pub game: Mutex<GameStatus>,
    /// The user's real mic, captured before we became the default source.
    real_mic: Mutex<Option<String>>,
    claimed: AtomicBool,
    /// Default-source swap waits until the UI is up, so a failed start never leaves it swapped.
    pub ui_ready: AtomicBool,
    pub import_lock: tokio::sync::Mutex<()>,
}

pub fn launch_options() -> String {
    format!("-con_logfile {}", cs2::LOG_NAME)
}

impl App {
    pub fn new() -> anyhow::Result<Arc<Self>> {
        let paths = Paths::new();
        let settings: Settings = library::load_json(&paths.settings());
        let library: Library = library::load_json(&paths.index());
        let mixer = Arc::new(Mutex::new(Mixer::new()));
        mixer.lock().unwrap().set_params(settings.mix());
        let graph: Graph = Arc::new(Mutex::new(Vec::new()));
        let app = Arc::new(Self {
            real_mic: Mutex::new(defaults::real_default_source(&paths.data, epw::SOURCE_NAME)),
            paths,
            settings: Mutex::new(settings),
            library: Mutex::new(library),
            mixer: mixer.clone(),
            graph: graph.clone(),
            pw: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
            game: Mutex::new(GameStatus { launch_options: launch_options(), ..Default::default() }),
            claimed: AtomicBool::new(false),
            ui_ready: AtomicBool::new(false),
            import_lock: tokio::sync::Mutex::new(()),
        });
        let tx = epw::spawn(mixer, graph)?;
        *app.pw.lock().unwrap() = Some(tx);
        app.apply_settings();
        Ok(app)
    }

    fn send(&self, c: PwCmd) {
        if let Some(tx) = self.pw.lock().unwrap().as_ref() {
            let _ = tx.send(c);
        }
    }

    pub fn mic_target(&self) -> Option<String> {
        let s = self.settings.lock().unwrap();
        s.mic_device.clone().or_else(|| self.real_mic.lock().unwrap().clone())
    }

    /// Push settings to the mixer and PipeWire, and swap the default source if enabled.
    pub fn apply_settings(&self) {
        let s = self.settings.lock().unwrap().clone();
        self.mixer.lock().unwrap().set_params(s.mix());
        self.send(PwCmd::SetMic(self.mic_target()));
        self.send(PwCmd::SetMonitor { enabled: s.monitor_enabled, target: s.monitor_device.clone() });
        self.sync_apps();
        if !self.ui_ready.load(Ordering::SeqCst) {
            return;
        }
        if s.set_default_source && !self.claimed.load(Ordering::SeqCst) {
            if defaults::claim(&self.paths.data, epw::SOURCE_NAME) {
                self.claimed.store(true, Ordering::SeqCst);
            }
        } else if !s.set_default_source && self.claimed.load(Ordering::SeqCst) {
            defaults::restore(&self.paths.data, epw::SOURCE_NAME);
            self.claimed.store(false, Ordering::SeqCst);
        }
    }

    /// Map selected app names onto current stream nodes and (re)connect captures.
    pub fn sync_apps(&self) {
        let names = self.settings.lock().unwrap().apps.clone();
        let mut serials: Vec<u32> = self
            .graph
            .lock()
            .unwrap()
            .iter()
            .filter(|n| n.class == "Stream/Output/Audio" && !n.name.starts_with("pipewire_dj"))
            .filter(|n| names.iter().any(|a| *a == app_label(n)))
            .map(|n| n.serial)
            .collect();
        serials.sort();
        self.send(PwCmd::SetApps(serials));
    }

    pub fn save_settings(&self, s: Settings) -> anyhow::Result<()> {
        library::save_json(&self.paths.settings(), &s)?;
        *self.settings.lock().unwrap() = s;
        self.apply_settings();
        self.write_game_cfgs();
        Ok(())
    }

    pub fn save_library(&self) -> anyhow::Result<()> {
        library::save_json(&self.paths.index(), &*self.library.lock().unwrap())?;
        self.write_game_cfgs();
        Ok(())
    }

    // ---- transport -----------------------------------------------------------

    fn loaded(&self, t: &Track) -> anyhow::Result<LoadedTrack> {
        let samples = {
            let mut c = self.cache.lock().unwrap();
            match c.get(&t.id) {
                Some(s) => s.clone(),
                None => {
                    let s = library::read_samples(&self.paths.track(t.id))?;
                    c.insert(t.id, s.clone());
                    s
                }
            }
        };
        let frames = samples.len() / 2;
        let (start, end) = trim_frames(t, frames);
        Ok(LoadedTrack { id: t.id, samples, volume: t.volume, start, end })
    }

    pub fn load(&self, sel: &str) -> anyhow::Result<u64> {
        let t = self.library.lock().unwrap().resolve(sel).cloned().ok_or_else(|| anyhow::anyhow!("no track '{sel}'"))?;
        let lt = self.loaded(&t)?;
        self.mixer.lock().unwrap().load(lt);
        Ok(t.id)
    }

    pub fn play(&self, sel: &str) -> anyhow::Result<()> {
        self.load(sel)?;
        let mut m = self.mixer.lock().unwrap();
        if !m.is_playing() {
            m.toggle();
        }
        Ok(())
    }

    pub fn toggle(&self) {
        self.mixer.lock().unwrap().toggle();
    }
    pub fn stop(&self) {
        self.mixer.lock().unwrap().stop();
    }
    pub fn restart(&self) {
        self.mixer.lock().unwrap().restart();
    }

    pub fn track_changed(&self, id: u64) {
        let t = self.library.lock().unwrap().get(id).cloned();
        if let Some(t) = t {
            if let Some(s) = self.cache.lock().unwrap().get(&id).cloned() {
                let (a, b) = trim_frames(&t, s.len() / 2);
                self.mixer.lock().unwrap().update_loaded(id, t.volume, a, b);
            }
        }
    }

    pub fn forget_track(&self, id: u64) {
        self.cache.lock().unwrap().remove(&id);
        self.mixer.lock().unwrap().unload_if(id);
        let _ = std::fs::remove_file(self.paths.track(id));
    }

    pub fn waveform(&self, id: u64, buckets: usize) -> anyhow::Result<Vec<f32>> {
        let t = self.library.lock().unwrap().get(id).cloned().ok_or_else(|| anyhow::anyhow!("no track"))?;
        let lt = self.loaded(&t)?;
        Ok(library::waveform(&lt.samples, buckets))
    }

    // ---- import --------------------------------------------------------------

    pub async fn import_file(&self, path: PathBuf, name: Option<String>, source: String) -> anyhow::Result<Track> {
        let _g = self.import_lock.lock().await;
        let id = self.library.lock().unwrap().alloc_id();
        let duration = library::convert(&path, &self.paths.track(id)).await?;
        let name = name.unwrap_or_else(|| {
            path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| format!("Track {id}"))
        });
        let t = Track {
            id,
            name,
            tags: vec![],
            volume: 1.0,
            trim_start: 0.0,
            trim_end: None,
            duration,
            bind: None,
            instant: false,
            source,
        };
        self.library.lock().unwrap().tracks.push(t.clone());
        self.save_library()?;
        Ok(t)
    }

    pub async fn import_youtube(&self, url: &str) -> anyhow::Result<Track> {
        let dir = self.paths.data.join("downloads");
        std::fs::create_dir_all(&dir)?;
        let (file, title) = library::youtube(url, &dir).await?;
        let r = self.import_file(file.clone(), Some(title), url.to_string()).await;
        let _ = std::fs::remove_file(&file);
        r
    }

    // ---- game ----------------------------------------------------------------

    pub fn write_game_cfgs(&self) {
        let s = self.settings.lock().unwrap().clone();
        if !s.cs2_enabled {
            return;
        }
        let Some(install) = cs2::find_install() else { return };
        let ptt = s.cs2_ptt_key.clone().or_else(cs2::detect_ptt_key);
        let lib = self.library.lock().unwrap();
        let ok = cs2::write_cfgs(&install, &lib, &cs2::CfgOptions { play_key: &s.cs2_play_key, ptt_key: ptt.as_deref() }, true).is_ok();
        let mut g = self.game.lock().unwrap();
        g.install = Some(install.display().to_string());
        g.ptt_key = ptt;
        g.cfg_written = ok;
    }

    pub fn on_game_event(&self, ev: cs2::GameEvent) {
        use cs2::GameEvent::*;
        self.game.lock().unwrap().last_event = Some(format!("{ev:?}"));
        let r = match &ev {
            Toggle => {
                self.toggle();
                Ok(())
            }
            Stop => {
                self.stop();
                Ok(())
            }
            Restart => {
                self.restart();
                Ok(())
            }
            Load(s) => self.load(s).map(|_| ()),
            Play(s) => self.play(s),
            Ptt(on) => {
                self.mixer.lock().unwrap().ptt = *on;
                self.game.lock().unwrap().ptt = *on;
                Ok(())
            }
        };
        if let Err(e) = r {
            eprintln!("pipewire-dj: {ev:?}: {e}");
        }
    }

    pub fn set_game_connected(&self, on: bool) {
        self.mixer.lock().unwrap().game_connected = on;
    }

    pub fn status(&self) -> Status {
        let mut m = self.mixer.lock().unwrap();
        let playback = m.status();
        let meters = m.take_meters();
        let ptt = m.ptt;
        let mic_open = m.mic_open();
        drop(m);
        Status { playback, meters, ptt, mic_open, game: self.game.lock().unwrap().clone() }
    }

    pub fn devices(&self) -> Vec<Node> {
        self.graph.lock().unwrap().clone()
    }

    /// Restore system defaults and tell CS2 to drop open-mic. Safe to call twice.
    pub fn shutdown(&self) {
        if self.claimed.swap(false, Ordering::SeqCst) {
            defaults::restore(&self.paths.data, epw::SOURCE_NAME);
        }
        if let Some(install) = cs2::find_install() {
            let _ = cs2::write_state(&install, false);
        }
        self.send(PwCmd::Quit);
    }
}

pub fn app_label(n: &Node) -> String {
    if !n.app.is_empty() {
        n.app.clone()
    } else if !n.description.is_empty() {
        n.description.clone()
    } else {
        n.name.clone()
    }
}

pub fn trim_frames(t: &Track, frames: usize) -> (usize, usize) {
    let start = ((t.trim_start.max(0.0) * RATE as f64) as usize).min(frames);
    let end = t.trim_end.map(|e| (e * RATE as f64) as usize).unwrap_or(frames).min(frames).max(start);
    (start, end)
}
