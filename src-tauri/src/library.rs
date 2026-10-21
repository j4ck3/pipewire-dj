//! Track library: JSON index plus normalized audio files in the data directory.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;

/// On-disk format of library tracks: raw interleaved f32le, 48 kHz stereo.
pub const TRACK_EXT: &str = "f32";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    /// Stable id: never reused, so binds and console numbers don't shift.
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Linear gain applied on top of loudness normalization.
    #[serde(default = "one")]
    pub volume: f32,
    /// Trim, seconds.
    #[serde(default)]
    pub trim_start: f64,
    #[serde(default)]
    pub trim_end: Option<f64>,
    pub duration: f64,
    /// In-game bind key that loads this track, e.g. "kp_1".
    #[serde(default)]
    pub bind: Option<String>,
    /// Bind plays the track immediately instead of only loading it.
    #[serde(default)]
    pub instant: bool,
    #[serde(default)]
    pub source: String,
}

fn one() -> f32 {
    1.0
}

#[derive(Default, Serialize, Deserialize)]
pub struct Library {
    pub next_id: u64,
    pub tracks: Vec<Track>,
}

pub struct Paths {
    pub data: PathBuf,
}

impl Paths {
    pub fn new() -> Self {
        let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("pipewire-dj");
        let _ = std::fs::create_dir_all(data.join("tracks"));
        Self { data }
    }
    pub fn index(&self) -> PathBuf {
        self.data.join("library.json")
    }
    pub fn settings(&self) -> PathBuf {
        self.data.join("settings.json")
    }
    pub fn track(&self, id: u64) -> PathBuf {
        self.data.join("tracks").join(format!("{id}.{TRACK_EXT}"))
    }
}

/// Write JSON atomically: temp file + rename, so a crash never leaves a torn file.
pub fn save_json<T: Serialize>(path: &Path, v: &T) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn load_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

impl Library {
    pub fn get(&self, id: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }
    pub fn alloc_id(&mut self) -> u64 {
        self.next_id = self.next_id.max(self.tracks.iter().map(|t| t.id).max().unwrap_or(0)) + 1;
        self.next_id
    }
    /// Resolve a console selector: a track number or a tag (case-insensitive).
    pub fn resolve(&self, sel: &str) -> Option<&Track> {
        if let Ok(id) = sel.parse::<u64>() {
            return self.get(id);
        }
        let s = sel.to_lowercase();
        self.tracks.iter().find(|t| t.tags.iter().any(|g| g.to_lowercase() == s))
    }
}

/// Load a converted track into memory.
pub fn read_samples(path: &Path) -> anyhow::Result<Arc<Vec<f32>>> {
    let bytes = std::fs::read(path)?;
    Ok(Arc::new(
        bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
    ))
}

/// Decode any media file ffmpeg understands to 48 kHz stereo f32, loudness-normalized
/// to -16 LUFS (single pass `loudnorm`; good enough for short clips).
pub async fn convert(input: &Path, out: &Path) -> anyhow::Result<f64> {
    let tmp = out.with_extension("part");
    let st = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-vn", "-af", "loudnorm=I=-16:TP=-1.5:LRA=11", "-ac", "2", "-ar", "48000", "-f", "f32le", "-c:a", "pcm_f32le"])
        .arg(&tmp)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !st.status.success() {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("ffmpeg failed: {}", String::from_utf8_lossy(&st.stderr).trim());
    }
    let len = std::fs::metadata(&tmp)?.len();
    if len == 0 {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("no audio stream in {}", input.display());
    }
    std::fs::rename(&tmp, out)?;
    Ok(len as f64 / (4.0 * 2.0 * 48_000.0))
}

/// Download audio with yt-dlp into `dir`; returns (file path, title).
pub async fn youtube(url: &str, dir: &Path) -> anyhow::Result<(PathBuf, String)> {
    let out = Command::new("yt-dlp")
        .args(["--no-playlist", "-x", "--no-progress", "--print", "after_move:filepath", "--print", "title", "-o"])
        .arg(dir.join("%(id)s.%(ext)s"))
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .await?;
    if !out.status.success() {
        anyhow::bail!("yt-dlp failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // --print order: title is printed before the move, filepath after.
    let title = lines.first().map(|s| s.to_string()).unwrap_or_else(|| url.to_string());
    let path = lines.last().map(PathBuf::from).ok_or_else(|| anyhow::anyhow!("yt-dlp printed no path"))?;
    Ok((path, title))
}

/// Peak envelope for the trim waveform view.
pub fn waveform(samples: &[f32], buckets: usize) -> Vec<f32> {
    let frames = samples.len() / 2;
    if frames == 0 || buckets == 0 {
        return vec![];
    }
    let per = frames.div_ceil(buckets);
    (0..buckets)
        .map(|b| {
            let s = (b * per).min(frames);
            let e = ((b + 1) * per).min(frames);
            samples[s * 2..e * 2].iter().fold(0.0f32, |m, v| m.max(v.abs()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_after_delete() {
        let mut l = Library::default();
        let a = l.alloc_id();
        l.tracks.push(Track { id: a, name: "a".into(), tags: vec!["Horn".into()], volume: 1.0, trim_start: 0.0, trim_end: None, duration: 1.0, bind: None, instant: false, source: String::new() });
        let b = l.alloc_id();
        l.tracks.push(Track { id: b, name: "b".into(), tags: vec![], volume: 1.0, trim_start: 0.0, trim_end: None, duration: 1.0, bind: None, instant: false, source: String::new() });
        l.tracks.retain(|t| t.id != a);
        assert_eq!(l.alloc_id(), 3);
        assert_eq!(l.resolve("2").unwrap().name, "b");
        assert!(l.resolve("1").is_none());
    }

    #[test]
    fn resolve_by_tag() {
        let mut l = Library::default();
        let id = l.alloc_id();
        l.tracks.push(Track { id, name: "x".into(), tags: vec!["Airhorn".into()], volume: 1.0, trim_start: 0.0, trim_end: None, duration: 1.0, bind: None, instant: false, source: String::new() });
        assert_eq!(l.resolve("airhorn").unwrap().id, id);
    }
}
