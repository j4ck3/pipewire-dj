//! Counter-Strike 2 adapter.
//!
//! CS2 removed `-netconport` and its `-concommandpipe` requires `-insecure`, so the
//! game cannot be driven from outside. The integration is one-way, game -> app:
//!
//! * `dj.cfg` (exec'd from autoexec) defines `dj_*` aliases that `echo PWDJ:...` markers.
//! * CS2 writes its console to a log file (`-con_logfile pipewire_dj.log`); the app tails it.
//! * CS2 runs open-mic (`voice_vox 1`) on the PipeWire DJ source, and the app gates what
//!   is audible: your voice only while push-to-talk is held, tracks while they play.
//! * `dj_state.cfg` holds `voice_vox 1` while the app runs and `voice_vox 0` after it
//!   exits; every push-to-talk release and play press re-execs it, so CS2 falls back to
//!   normal push-to-talk shortly after the app closes.

use crate::library::Library;
use std::path::{Path, PathBuf};

pub const LOG_NAME: &str = "pipewire_dj.log";
pub const MARK: &str = "PWDJ:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameEvent {
    Toggle,
    Stop,
    Restart,
    Load(String),
    Play(String),
    Ptt(bool),
}

/// Parse one console line. Markers may be prefixed by a timestamp or channel tag.
pub fn parse_line(line: &str) -> Option<GameEvent> {
    let rest = &line[line.find(MARK)? + MARK.len()..];
    let tok = rest.split_whitespace().next()?;
    let mut parts = tok.splitn(2, ':');
    let cmd = parts.next()?;
    let arg = parts.next().unwrap_or("").to_string();
    Some(match cmd {
        "toggle" => GameEvent::Toggle,
        "stop" => GameEvent::Stop,
        "restart" => GameEvent::Restart,
        "load" if !arg.is_empty() => GameEvent::Load(arg),
        "play" if !arg.is_empty() => GameEvent::Play(arg),
        "ptt" => GameEvent::Ptt(arg == "1"),
        _ => return None,
    })
}

/// Find the CS2 install via Steam's library folders.
pub fn find_install() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let steam_roots = [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];
    let re = regex::Regex::new(r#""path"\s+"([^"]+)""#).ok()?;
    for root in steam_roots {
        let vdf = root.join("steamapps/libraryfolders.vdf");
        let mut libs = vec![root.clone()];
        if let Ok(text) = std::fs::read_to_string(&vdf) {
            libs.extend(re.captures_iter(&text).map(|c| PathBuf::from(&c[1])));
        }
        for lib in libs {
            let p = lib.join("steamapps/common/Counter-Strike Global Offensive");
            if p.join("game/csgo").is_dir() {
                return Some(p);
            }
        }
    }
    None
}

/// `find_install`, re-scanned at most every 10 s.
pub fn find_install_cached() -> Option<PathBuf> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Option<PathBuf>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    if let Some((t, p)) = c.as_ref() {
        if t.elapsed() < Duration::from_secs(10) {
            return p.clone();
        }
    }
    let p = find_install();
    *c = Some((Instant::now(), p.clone()));
    p
}

pub fn cfg_dir(install: &Path) -> PathBuf {
    install.join("game/csgo/cfg")
}

/// Places CS2 may write the console log, depending on version and options.
pub fn log_candidates(install: &Path) -> Vec<PathBuf> {
    let g = install.join("game");
    vec![
        g.join("csgo").join(LOG_NAME),
        g.join(LOG_NAME),
        g.join("bin/linuxsteamrt64").join(LOG_NAME),
        g.join("csgo/console.log"),
    ]
}

/// Most recently modified existing log candidate.
pub fn find_log(install: &Path) -> Option<PathBuf> {
    log_candidates(install)
        .into_iter()
        .filter_map(|p| std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()).map(|t| (t, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// Key the player has bound to `+voicerecord`, read from the newest user keys file.
pub fn detect_ptt_key() -> Option<String> {
    let home = dirs::home_dir()?;
    let userdata = home.join(".local/share/Steam/userdata");
    let re = regex::Regex::new(r#""([^"]+)"\s+"\+voicerecord""#).ok()?;
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for user in std::fs::read_dir(userdata).ok()?.flatten() {
        let f = user.path().join("730/local/cfg/cs2_user_keys_0_slot0.vcfg");
        let (Ok(text), Ok(meta)) = (std::fs::read_to_string(&f), std::fs::metadata(&f)) else { continue };
        let Some(c) = re.captures(&text) else { continue };
        let t = meta.modified().ok()?;
        if best.as_ref().map(|(bt, _)| t > *bt).unwrap_or(true) {
            best = Some((t, c[1].to_string()));
        }
    }
    best.map(|(_, k)| k)
}

pub fn is_running() -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else { return false };
    dir.flatten().any(|e| {
        std::fs::read_to_string(e.path().join("comm")).map(|c| c.trim() == "cs2").unwrap_or(false)
    })
}

/// Console-safe alias suffix for a tag.
pub fn tag_alias(tag: &str) -> Option<String> {
    let s: String = tag
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_string();
    (!s.is_empty()).then_some(s)
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| *c != '"' && *c != ';' && !c.is_control()).collect()
}

pub struct CfgOptions<'a> {
    pub play_key: &'a str,
    pub ptt_key: Option<&'a str>,
}

/// Build dj.cfg: aliases, binds and the vox/gating hookup.
pub fn render_cfg(lib: &Library, o: &CfgOptions) -> String {
    let mut s = String::new();
    s.push_str("// Generated by PipeWire DJ. Edits are overwritten.\n");
    s.push_str("// Add `exec dj` to your autoexec.cfg.\n");
    s.push_str("con_enable 1\n");
    s.push_str("voice_enable 1\n");
    s.push_str("alias dj_list \"exec dj_list\"\n");
    s.push_str("alias dj_play \"echo PWDJ:toggle; exec dj_state\"\n");
    s.push_str("alias dj_stop \"echo PWDJ:stop\"\n");
    s.push_str("alias dj_restart \"echo PWDJ:restart; exec dj_state\"\n");
    s.push_str("alias +dj_ptt \"+voicerecord; echo PWDJ:ptt:1\"\n");
    s.push_str("alias -dj_ptt \"-voicerecord; echo PWDJ:ptt:0; exec dj_state\"\n");
    s.push_str(&format!("bind \"{}\" \"dj_play\"\n", clean(o.play_key)));
    if let Some(k) = o.ptt_key {
        s.push_str(&format!("bind \"{}\" \"+dj_ptt\"\n", clean(k)));
    }
    let mut seen_tags = std::collections::HashSet::new();
    for t in &lib.tracks {
        s.push_str(&format!("alias dj_{} \"echo PWDJ:load:{}\"\n", t.id, t.id));
        for tag in &t.tags {
            if let Some(a) = tag_alias(tag) {
                if a.parse::<u64>().is_err() && seen_tags.insert(a.clone()) {
                    s.push_str(&format!("alias dj_{} \"echo PWDJ:load:{}\"\n", a, t.id));
                }
            }
        }
        if let Some(k) = &t.bind {
            let verb = if t.instant { "play" } else { "load" };
            s.push_str(&format!("bind \"{}\" \"echo PWDJ:{}:{}; exec dj_state\"\n", clean(k), verb, t.id));
        }
    }
    s.push_str("exec dj_state\n");
    s.push_str("echo \"PipeWire DJ loaded. dj_list shows tracks.\"\n");
    s
}

pub fn render_list(lib: &Library) -> String {
    let mut s = String::from("echo \"---------------- PipeWire DJ ----------------\"\n");
    s.push_str("echo \"Select with dj_<number> or dj_<tag>, then dj_play.\"\n");
    for t in &lib.tracks {
        let tags: Vec<String> = t.tags.iter().filter_map(|g| tag_alias(g)).collect();
        let extra = if tags.is_empty() { String::new() } else { format!(" [{}]", tags.join(", ")) };
        s.push_str(&format!("echo \"{}. {}{}\"\n", t.id, clean(&t.name), extra));
    }
    s.push_str("echo \"----------------------------------------------\"\n");
    s
}

pub fn render_state(app_running: bool) -> String {
    if app_running {
        "voice_vox 1\n".into()
    } else {
        "voice_vox 0\n".into()
    }
}

pub fn write_cfgs(install: &Path, lib: &Library, o: &CfgOptions, app_running: bool) -> std::io::Result<()> {
    let dir = cfg_dir(install);
    std::fs::create_dir_all(&dir)?;
    let w = |name: &str, body: String| -> std::io::Result<()> {
        let p = dir.join(name);
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, body)?;
        std::fs::rename(tmp, p)
    };
    w("dj.cfg", render_cfg(lib, o))?;
    w("dj_list.cfg", render_list(lib))?;
    w("dj_state.cfg", render_state(app_running))
}

pub fn write_state(install: &Path, app_running: bool) -> std::io::Result<()> {
    std::fs::write(cfg_dir(install).join("dj_state.cfg"), render_state(app_running))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Track;

    #[test]
    fn parses_markers() {
        assert_eq!(parse_line("10/21 23:01:02 PWDJ:toggle"), Some(GameEvent::Toggle));
        assert_eq!(parse_line("[Console] PWDJ:load:12"), Some(GameEvent::Load("12".into())));
        assert_eq!(parse_line("PWDJ:ptt:1"), Some(GameEvent::Ptt(true)));
        assert_eq!(parse_line("PWDJ:ptt:0"), Some(GameEvent::Ptt(false)));
        assert_eq!(parse_line("PWDJ:play:3 extra"), Some(GameEvent::Play("3".into())));
        assert_eq!(parse_line("nothing here"), None);
        assert_eq!(parse_line("PWDJ:load:"), None);
    }

    #[test]
    fn cfg_has_aliases_binds_and_no_quotes_from_names() {
        let mut lib = Library::default();
        lib.tracks.push(Track {
            id: 7, name: "Air \"horn\"; quit".into(), tags: vec!["Air Horn".into(), "5".into()], volume: 1.0,
            trim_start: 0.0, trim_end: None, duration: 1.0, bind: Some("kp_1".into()), instant: true, source: String::new(),
        });
        let cfg = render_cfg(&lib, &CfgOptions { play_key: "kp_enter", ptt_key: Some("k") });
        assert!(cfg.contains("alias dj_7 \"echo PWDJ:load:7\""));
        assert!(cfg.contains("alias dj_air_horn \"echo PWDJ:load:7\""));
        assert!(!cfg.contains("alias dj_5 \"echo PWDJ:load:7\""), "numeric tag must not shadow ids");
        assert!(cfg.contains("bind \"kp_1\" \"echo PWDJ:play:7; exec dj_state\""));
        assert!(cfg.contains("bind \"k\" \"+dj_ptt\""));
        let list = render_list(&lib);
        assert!(list.contains("echo \"7. Air horn quit [air_horn, 5]\""));
    }
}
