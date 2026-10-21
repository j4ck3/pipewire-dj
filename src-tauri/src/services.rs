//! Background services: CS2 log tailer, CLI control socket, desktop global shortcuts.

use crate::app::App;
use crate::games::cs2;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("pipewire-dj.sock")
}

// ---- CS2 console log tailer --------------------------------------------------

pub async fn cs2_watch(app: Arc<App>) {
    let mut cur: Option<(PathBuf, u64)> = None;
    let mut pending = String::new();
    let mut tick: u32 = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(30)).await;
        tick = tick.wrapping_add(1);
        if !app.settings.lock().unwrap().cs2_enabled {
            continue;
        }
        let Some(install) = cs2::find_install_cached() else { continue };
        // Housekeeping twice a second.
        if tick % 16 == 0 {
            let running = cs2::is_running();
            app.set_game_connected(running);
            let mut g = app.game.lock().unwrap();
            g.running = running;
            g.install = Some(install.display().to_string());
            if let Some(p) = cs2::find_log(&install) {
                if cur.as_ref().map(|(c, _)| c != &p).unwrap_or(true) {
                    let len = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    cur = Some((p.clone(), len));
                    pending.clear();
                }
                let fresh = std::fs::metadata(&p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .map(|d| d < Duration::from_secs(120))
                    .unwrap_or(false);
                g.log = Some(p.display().to_string());
                g.log_active = running && fresh;
            }
            if !running {
                drop(g);
                // Game gone: never leave the mic latched open by a lost PTT release.
                if app.mixer.lock().unwrap().ptt {
                    app.on_game_event(cs2::GameEvent::Ptt(false));
                }
            }
        }
        let Some((path, off)) = cur.as_mut() else { continue };
        let Ok(mut f) = std::fs::File::open(&*path) else { continue };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < *off {
            *off = 0; // truncated: a new game session started
            pending.clear();
        }
        if len == *off {
            continue;
        }
        if f.seek(SeekFrom::Start(*off)).is_err() {
            continue;
        }
        let mut buf = Vec::new();
        if f.take(1 << 20).read_to_end(&mut buf).is_err() {
            continue;
        }
        *off += buf.len() as u64;
        pending.push_str(&String::from_utf8_lossy(&buf));
        while let Some(i) = pending.find('\n') {
            let line: String = pending.drain(..=i).collect();
            if let Some(ev) = cs2::parse_line(&line) {
                app.on_game_event(ev);
            }
        }
    }
}

// ---- CLI control socket -----------------------------------------------------

/// Run a control command; returns the reply line.
pub fn exec_command(app: &App, line: &str) -> String {
    let mut it = line.split_whitespace();
    let cmd = it.next().unwrap_or("");
    let arg = it.collect::<Vec<_>>().join(" ");
    let r: anyhow::Result<String> = match cmd {
        "toggle" | "play-pause" => {
            app.toggle();
            Ok("ok".into())
        }
        "stop" => {
            app.stop();
            Ok("ok".into())
        }
        "restart" => {
            app.restart();
            Ok("ok".into())
        }
        "load" => app.load(&arg).map(|id| format!("loaded {id}")),
        "play" => app.play(&arg).map(|_| "ok".into()),
        "ptt-on" => {
            app.on_game_event(cs2::GameEvent::Ptt(true));
            Ok("ok".into())
        }
        "ptt-off" => {
            app.on_game_event(cs2::GameEvent::Ptt(false));
            Ok("ok".into())
        }
        "mute" => {
            let mut s = app.settings.lock().unwrap().clone();
            s.mic_mute = !s.mic_mute;
            let m = s.mic_mute;
            app.save_settings(s).map(|_| if m { "muted".into() } else { "unmuted".into() })
        }
        "list" => {
            let l = app.library.lock().unwrap();
            Ok(l.tracks.iter().map(|t| format!("{}\t{}\t{}", t.id, t.name, t.tags.join(","))).collect::<Vec<_>>().join("\n"))
        }
        "status" => Ok(serde_json::to_string(&app.status()).unwrap_or_default()),
        "show" => Ok("show".into()),
        _ => Err(anyhow::anyhow!("unknown command '{cmd}'")),
    };
    match r {
        Ok(s) => s,
        Err(e) => format!("error: {e}"),
    }
}

pub async fn control_socket(app: Arc<App>, on_show: impl Fn() + Send + Sync + 'static) {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("pipewire-dj: control socket {}: {e}", path.display());
            return;
        }
    };
    let on_show = Arc::new(on_show);
    loop {
        let Ok((sock, _)) = listener.accept().await else { continue };
        let app = app.clone();
        let on_show = on_show.clone();
        tokio::spawn(async move {
            let (r, mut w) = sock.into_split();
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let reply = if let Some(src) = line.strip_prefix("import ") {
                    let src = src.trim();
                    let res = if src.starts_with("http://") || src.starts_with("https://") {
                        app.import_youtube(src).await
                    } else {
                        app.import_file(src.into(), None, src.to_string()).await
                    };
                    match res {
                        Ok(t) => format!("imported {} {}", t.id, t.name),
                        Err(e) => format!("error: {e}"),
                    }
                } else {
                    exec_command(&app, &line)
                };
                if reply == "show" {
                    on_show();
                }
                let _ = w.write_all(format!("{reply}\n").as_bytes()).await;
            }
        });
    }
}

/// Client side: send one command to a running instance.
pub async fn send_to_running(cmd: &str) -> std::io::Result<String> {
    let mut s = UnixStream::connect(socket_path()).await?;
    s.write_all(format!("{cmd}\n").as_bytes()).await?;
    s.shutdown().await?;
    let mut out = String::new();
    let mut r = BufReader::new(s);
    while r.read_line(&mut out).await? > 0 {}
    Ok(out.trim_end().to_string())
}

// ---- Desktop global shortcuts (XDG portal) ------------------------------------

pub async fn shortcuts(app: Arc<App>) -> anyhow::Result<()> {
    use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
    use futures_util::StreamExt;

    // Unsandboxed apps must register an app id with the portal first. The connection
    // used for registration must be the one later used for the portal calls.
    let conn = zbus::Connection::session().await?;
    let app_id: ashpd::AppID = "dev.pipewire_dj.PipeWireDJ".try_into()?;
    if let Err(e) = ashpd::register_host_app_with_connection(conn.clone(), app_id).await {
        eprintln!("pipewire-dj: portal host registration failed ({e}); continuing");
    }
    let gs = GlobalShortcuts::with_connection(conn).await?;
    let session = gs.create_session(Default::default()).await?;
    let list = [
        NewShortcut::new("toggle", "Play / pause loaded track"),
        NewShortcut::new("stop", "Stop track"),
        NewShortcut::new("restart", "Restart track"),
        NewShortcut::new("ptt", "Push to talk (hold)"),
        NewShortcut::new("mute", "Toggle mic mute"),
    ];
    gs.bind_shortcuts(&session, &list, None, BindShortcutsOptions::default()).await?.response()?;
    let mut act = gs.receive_activated().await?;
    let mut deact = gs.receive_deactivated().await?;
    loop {
        tokio::select! {
            Some(a) = act.next() => {
                let cmd = match a.shortcut_id() { "ptt" => "ptt-on", other => other };
                exec_command(&app, cmd);
            }
            Some(d) = deact.next() => {
                if d.shortcut_id() == "ptt" { exec_command(&app, "ptt-off"); }
            }
            else => break,
        }
    }
    let _ = session;
    Ok(())
}
