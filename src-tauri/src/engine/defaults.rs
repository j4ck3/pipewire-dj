//! Swap the system default source to the virtual mic, and restore it afterwards.
//! Uses `pw-metadata` (ships with PipeWire) to edit the "default" metadata object.

use std::path::{Path, PathBuf};
use std::process::Command;

const KEY: &str = "default.configured.audio.source";

fn state_file(data_dir: &Path) -> PathBuf {
    data_dir.join("previous-default-source.json")
}

/// Read a key from the "default" metadata; returns the `name` inside its JSON value.
pub fn read_default(key: &str) -> Option<String> {
    let out = Command::new("pw-metadata").args(["-n", "default", "0", key]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let re = regex::Regex::new(r#"value:'\{"name":"([^"]+)"\}'"#).ok()?;
    re.captures(&text).map(|c| c[1].to_string())
}

fn write_default(name: Option<&str>) -> bool {
    let status = match name {
        Some(n) => Command::new("pw-metadata")
            .args(["-n", "default", "0", KEY, &format!("{{\"name\":\"{n}\"}}"), "Spa:String:JSON"])
            .status(),
        None => Command::new("pw-metadata").args(["-n", "default", "-d", "0", KEY]).status(),
    };
    status.map(|s| s.success()).unwrap_or(false)
}

/// The user's real default mic: the saved pre-swap value, else the current one.
pub fn real_default_source(data_dir: &Path, ours: &str) -> Option<String> {
    if let Some(saved) = load_saved(data_dir) {
        return saved;
    }
    [KEY, "default.audio.source"]
        .iter()
        .filter_map(|k| read_default(k))
        .find(|n| n != ours)
}

fn load_saved(data_dir: &Path) -> Option<Option<String>> {
    let s = std::fs::read_to_string(state_file(data_dir)).ok()?;
    serde_json::from_str::<Option<String>>(&s).ok()
}

/// Make `ours` the default source, remembering what was there before.
/// If a previous run crashed, the saved value is kept rather than overwritten with `ours`.
pub fn claim(data_dir: &Path, ours: &str) -> bool {
    if load_saved(data_dir).is_none() {
        let prev = read_default(KEY).filter(|n| n != ours);
        let _ = std::fs::create_dir_all(data_dir);
        let _ = std::fs::write(state_file(data_dir), serde_json::to_string(&prev).unwrap());
    }
    write_default(Some(ours))
}

/// Restore the pre-swap default (or clear the key) and forget the saved value.
pub fn restore(data_dir: &Path, ours: &str) -> bool {
    let Some(prev) = load_saved(data_dir) else {
        // Nothing saved. Only clean up if the default still points at us.
        if read_default(KEY).as_deref() == Some(ours) {
            return write_default(None);
        }
        return true;
    };
    let ok = write_default(prev.as_deref());
    if ok {
        let _ = std::fs::remove_file(state_file(data_dir));
    }
    ok
}
