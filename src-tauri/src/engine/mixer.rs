//! Real-time mixer. Everything here runs on the PipeWire data thread, behind one
//! mutex shared by all stream callbacks (they execute serially on that thread).

use nnnoiseless::DenoiseState;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

pub const RATE: usize = 48_000;
pub const CH: usize = 2;
/// Cap for every queue between callbacks, in frames (~100 ms). Older data is dropped.
const QUEUE_CAP: usize = RATE / 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateMode {
    /// Mic always passes.
    Off,
    /// Mic only passes while push-to-talk is held (or a track plays).
    Always,
    /// Gate only while a game integration is connected; open otherwise.
    WhileGame,
}

/// User-facing mix parameters, copied in from settings.
#[derive(Clone, Debug)]
pub struct MixParams {
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
}

impl Default for MixParams {
    fn default() -> Self {
        Self {
            mic_volume: 1.0,
            mic_mute: false,
            denoise: false,
            track_volume: 1.0,
            apps_volume: 0.6,
            monitor_volume: 0.6,
            pause_on_toggle: true,
            gate_mode: GateMode::WhileGame,
            open_mic_while_track: true,
            limiter: true,
        }
    }
}

/// A decoded track ready for playback (interleaved stereo f32 at 48 kHz).
pub struct LoadedTrack {
    pub id: u64,
    pub samples: Arc<Vec<f32>>,
    pub volume: f32,
    /// Frame range to play.
    pub start: usize,
    pub end: usize,
}

#[derive(Default, Clone, Copy, serde::Serialize)]
pub struct Meters {
    pub mic: f32,
    pub track: f32,
    pub apps: f32,
    pub out: f32,
    pub monitor: f32,
}

#[derive(Clone, serde::Serialize)]
pub struct PlaybackStatus {
    pub loaded: Option<u64>,
    pub playing: bool,
    pub position: f64,
    pub start: f64,
    pub end: f64,
}

/// Mic audio effect slot. RNNoise is the first; a voice changer can implement this later.
pub trait MicEffect: Send {
    fn process(&mut self, frames: &mut [f32]);
}

/// RNNoise on a mono downmix; adds one 10 ms frame of latency.
pub struct Denoise {
    state: Box<DenoiseState<'static>>,
    pending: Vec<f32>,
    ready: VecDeque<f32>,
    frame_out: Vec<f32>,
}

impl Denoise {
    pub fn new() -> Self {
        let mut ready = VecDeque::new();
        ready.extend(std::iter::repeat(0.0).take(DenoiseState::FRAME_SIZE));
        Self {
            state: DenoiseState::new(),
            pending: Vec::with_capacity(DenoiseState::FRAME_SIZE),
            ready,
            frame_out: vec![0.0; DenoiseState::FRAME_SIZE],
        }
    }
}

impl MicEffect for Denoise {
    fn process(&mut self, frames: &mut [f32]) {
        let n = frames.len() / CH;
        for i in 0..n {
            let mono = (frames[i * CH] + frames[i * CH + 1]) * 0.5;
            self.pending.push(mono * 32768.0);
            if self.pending.len() == DenoiseState::FRAME_SIZE {
                self.state.process_frame(&mut self.frame_out, &self.pending);
                self.ready.extend(self.frame_out.iter().map(|s| s / 32768.0));
                self.pending.clear();
            }
            let v = self.ready.pop_front().unwrap_or(0.0);
            frames[i * CH] = v;
            frames[i * CH + 1] = v;
        }
    }
}

/// Which callback produced the most recent mix; used to pick a single clock.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Driver {
    Mic = 0,
    Source = 1,
    Monitor = 2,
}

pub struct Mixer {
    pub params: MixParams,
    pub ptt: bool,
    pub game_connected: bool,
    track: Option<LoadedTrack>,
    pos: usize,
    playing: bool,
    mic_fx: Option<Box<dyn MicEffect>>,
    apps: HashMap<u32, VecDeque<f32>>,
    out_q: VecDeque<f32>,
    mon_q: VecDeque<f32>,
    limiter_env: f32,
    pub meters: Meters,
    last_driver: [Option<Instant>; 3],
    scratch: Vec<f32>,
    /// Set when a track ends; read and cleared by the control side.
    pub ended: bool,
}

fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

fn push_capped(q: &mut VecDeque<f32>, data: &[f32]) {
    q.extend(data.iter().copied());
    let cap = QUEUE_CAP * CH;
    if q.len() > cap {
        let drop = q.len() - cap;
        q.drain(..drop);
    }
}

/// Pop `out.len()` samples, first dropping backlog beyond two buffers so a queue
/// that filled up while nobody was reading does not add permanent latency.
fn pop_into(q: &mut VecDeque<f32>, out: &mut [f32]) {
    let keep = out.len() * 2;
    if q.len() > keep {
        let drop = q.len() - keep;
        q.drain(..drop);
    }
    for s in out.iter_mut() {
        *s = q.pop_front().unwrap_or(0.0);
    }
}

impl Mixer {
    pub fn new() -> Self {
        Self {
            params: MixParams::default(),
            ptt: false,
            game_connected: false,
            track: None,
            pos: 0,
            playing: false,
            mic_fx: None,
            apps: HashMap::new(),
            out_q: VecDeque::new(),
            mon_q: VecDeque::new(),
            limiter_env: 0.0,
            meters: Meters::default(),
            last_driver: [None; 3],
            scratch: Vec::new(),
            ended: false,
        }
    }

    pub fn set_params(&mut self, p: MixParams) {
        match (p.denoise, self.mic_fx.is_some()) {
            (true, false) => self.mic_fx = Some(Box::new(Denoise::new())),
            (false, true) => self.mic_fx = None,
            _ => {}
        }
        self.params = p;
    }

    // ---- transport -------------------------------------------------------

    /// Load a track. Re-loading the loaded track keeps its position (pause mode).
    pub fn load(&mut self, t: LoadedTrack) {
        let same = self.track.as_ref().map(|c| c.id) == Some(t.id);
        if same && self.params.pause_on_toggle {
            let (s, e) = (t.start, t.end);
            self.track = Some(t);
            if self.pos < s || self.pos >= e {
                self.pos = s;
            }
        } else {
            self.pos = t.start;
            self.playing = false;
            self.track = Some(t);
        }
    }

    /// Update volume/trim of the loaded track without touching position.
    pub fn update_loaded(&mut self, id: u64, volume: f32, start: usize, end: usize) {
        if let Some(t) = self.track.as_mut().filter(|t| t.id == id) {
            t.volume = volume;
            t.start = start;
            t.end = end;
            if self.pos < start || self.pos >= end {
                self.pos = start;
            }
        }
    }

    pub fn unload_if(&mut self, id: u64) {
        if self.track.as_ref().map(|t| t.id) == Some(id) {
            self.track = None;
            self.playing = false;
        }
    }

    pub fn toggle(&mut self) {
        let Some(t) = &self.track else { return };
        if self.playing {
            self.playing = false;
            if !self.params.pause_on_toggle {
                self.pos = t.start;
            }
        } else {
            if self.pos >= t.end || self.pos < t.start {
                self.pos = t.start;
            }
            self.playing = true;
        }
    }

    pub fn stop(&mut self) {
        self.playing = false;
        if let Some(t) = &self.track {
            self.pos = t.start;
        }
    }

    pub fn restart(&mut self) {
        if let Some(t) = &self.track {
            self.pos = t.start;
            self.playing = true;
        }
    }

    pub fn status(&self) -> PlaybackStatus {
        let f = |x: usize| x as f64 / RATE as f64;
        PlaybackStatus {
            loaded: self.track.as_ref().map(|t| t.id),
            playing: self.playing,
            position: f(self.pos),
            start: self.track.as_ref().map(|t| f(t.start)).unwrap_or(0.0),
            end: self.track.as_ref().map(|t| f(t.end)).unwrap_or(0.0),
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    // ---- inputs ------------------------------------------------------------

    pub fn add_app(&mut self, key: u32) {
        self.apps.entry(key).or_default();
    }

    pub fn remove_app(&mut self, key: u32) {
        self.apps.remove(&key);
    }

    pub fn push_app(&mut self, key: u32, data: &[f32]) {
        if let Some(q) = self.apps.get_mut(&key) {
            push_capped(q, data);
        }
    }

    pub fn mic_open(&self) -> bool {
        if self.params.mic_mute {
            return false;
        }
        let gated = match self.params.gate_mode {
            GateMode::Off => false,
            GateMode::Always => true,
            GateMode::WhileGame => self.game_connected,
        };
        !gated || self.ptt || (self.params.open_mic_while_track && self.playing)
    }

    fn driver_allowed(&mut self, d: Driver) -> bool {
        let now = Instant::now();
        let higher_alive = self.last_driver[..d as usize]
            .iter()
            .any(|t| t.is_some_and(|t| now.duration_since(t).as_millis() < 100));
        if higher_alive {
            return false;
        }
        self.last_driver[d as usize] = Some(now);
        true
    }

    /// Render `mic.len()/2` frames using the given mic data (silence if none),
    /// pushing results to the virtual-source and monitor queues.
    fn render(&mut self, mic: &mut [f32]) {
        let n = mic.len() / CH;
        let p = self.params.clone();

        if let Some(fx) = self.mic_fx.as_mut() {
            fx.process(mic);
        }
        for s in mic.iter_mut() {
            *s *= p.mic_volume;
        }
        self.meters.mic = self.meters.mic.max(peak(mic));
        let mic_on = self.mic_open();

        // Track.
        let mut track = std::mem::take(&mut self.scratch);
        track.clear();
        track.resize(n * CH, 0.0);
        if self.playing {
            if let Some(t) = &self.track {
                let gain = t.volume * p.track_volume;
                let end = t.end.min(t.samples.len() / CH);
                let avail = end.saturating_sub(self.pos).min(n);
                let src = &t.samples[self.pos * CH..(self.pos + avail) * CH];
                for (d, s) in track.iter_mut().zip(src) {
                    *d = s * gain;
                }
                self.pos += avail;
                if self.pos >= end {
                    self.playing = false;
                    self.pos = t.start;
                    self.ended = true;
                }
            }
        }
        self.meters.track = self.meters.track.max(peak(&track));

        // Apps.
        let mut apps = vec![0.0f32; n * CH];
        let mut tmp = vec![0.0f32; n * CH];
        for q in self.apps.values_mut() {
            pop_into(q, &mut tmp);
            for (a, s) in apps.iter_mut().zip(&tmp) {
                *a += s * p.apps_volume;
            }
        }
        self.meters.apps = self.meters.apps.max(peak(&apps));

        // Monitor: tracks + apps, never the mic.
        for (i, s) in tmp.iter_mut().enumerate() {
            *s = (track[i] + apps[i]) * p.monitor_volume;
        }
        soft_clip(&mut tmp);
        self.meters.monitor = self.meters.monitor.max(peak(&tmp));
        push_capped(&mut self.mon_q, &tmp);

        // Virtual mic.
        for i in 0..n * CH {
            let m = if mic_on { mic[i] } else { 0.0 };
            tmp[i] = m + track[i] + apps[i];
        }
        if p.limiter {
            self.limit(&mut tmp);
        } else {
            for s in tmp.iter_mut() {
                *s = s.clamp(-1.0, 1.0);
            }
        }
        self.meters.out = self.meters.out.max(peak(&tmp));
        push_capped(&mut self.out_q, &tmp);
        self.scratch = track;
    }

    /// Peak limiter at -1 dBFS with instant attack and ~150 ms release.
    fn limit(&mut self, buf: &mut [f32]) {
        const THR: f32 = 0.891;
        let rel = (-1.0 / (0.15 * RATE as f32)).exp();
        for f in buf.chunks_mut(CH) {
            let lvl = f.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            self.limiter_env = if lvl > self.limiter_env { lvl } else { self.limiter_env * rel + lvl * (1.0 - rel) };
            let g = if self.limiter_env > THR { THR / self.limiter_env } else { 1.0 };
            for s in f.iter_mut() {
                *s = (*s * g).clamp(-1.0, 1.0);
            }
        }
    }

    // ---- stream callbacks -------------------------------------------------

    pub fn on_mic(&mut self, data: &[f32]) {
        if self.driver_allowed(Driver::Mic) {
            let mut buf = data.to_vec();
            self.render(&mut buf);
        }
    }

    pub fn on_source(&mut self, out: &mut [f32]) {
        if self.out_q.len() < out.len() && self.driver_allowed(Driver::Source) {
            let mut silence = vec![0.0; out.len()];
            self.render(&mut silence);
        }
        pop_into(&mut self.out_q, out);
    }

    pub fn on_monitor(&mut self, out: &mut [f32]) {
        if self.mon_q.len() < out.len() && self.driver_allowed(Driver::Monitor) {
            let mut silence = vec![0.0; out.len()];
            self.render(&mut silence);
            // Nobody consumes the virtual mic right now; keep its queue bounded.
        }
        pop_into(&mut self.mon_q, out);
    }

    pub fn take_meters(&mut self) -> Meters {
        std::mem::take(&mut self.meters)
    }
}

fn soft_clip(buf: &mut [f32]) {
    for s in buf.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: u64, frames: usize) -> LoadedTrack {
        LoadedTrack { id, samples: Arc::new(vec![0.5; frames * CH]), volume: 1.0, start: 0, end: frames }
    }

    #[test]
    fn pause_resumes_same_track() {
        let mut m = Mixer::new();
        m.load(track(1, 4800));
        m.toggle();
        m.on_source(&mut vec![0.0; 960 * CH]);
        let pos = m.status().position;
        assert!(pos > 0.0);
        m.toggle(); // pause
        m.load(track(1, 4800)); // reload same id keeps position
        m.toggle();
        assert_eq!(m.status().position, pos);
        assert!(m.is_playing());
    }

    #[test]
    fn other_track_resets_and_stop_mode_restarts() {
        let mut m = Mixer::new();
        m.load(track(1, 4800));
        m.toggle();
        m.on_source(&mut vec![0.0; 960 * CH]);
        m.load(track(2, 4800));
        assert_eq!(m.status().position, 0.0);
        assert!(!m.is_playing());
        let mut p = m.params.clone();
        p.pause_on_toggle = false;
        m.set_params(p);
        m.toggle();
        m.on_source(&mut vec![0.0; 960 * CH]);
        m.toggle();
        assert_eq!(m.status().position, 0.0);
    }

    #[test]
    fn track_end_stops_and_flags() {
        let mut m = Mixer::new();
        m.load(track(1, 500));
        m.toggle();
        m.on_source(&mut vec![0.0; 960 * CH]);
        assert!(!m.is_playing());
        assert!(m.ended);
    }

    #[test]
    fn gate_blocks_mic_until_ptt() {
        let mut m = Mixer::new();
        let mut p = m.params.clone();
        p.gate_mode = GateMode::Always;
        m.set_params(p);
        m.on_mic(&vec![0.5; 480 * CH]);
        let mut out = vec![0.0; 480 * CH];
        m.on_source(&mut out);
        assert_eq!(peak(&out), 0.0);
        m.ptt = true;
        m.on_mic(&vec![0.5; 480 * CH]);
        m.on_source(&mut out);
        assert!(peak(&out) > 0.4);
    }

    #[test]
    fn mic_and_track_mix_and_limit() {
        let mut m = Mixer::new();
        let mut p = m.params.clone();
        p.gate_mode = GateMode::Off;
        m.set_params(p);
        m.load(track(1, 48_000));
        m.toggle();
        m.on_mic(&vec![0.8; 480 * CH]);
        let mut out = vec![0.0; 480 * CH];
        m.on_source(&mut out);
        assert!(peak(&out) <= 0.892, "limited: {}", peak(&out));
        assert!(peak(&out) > 0.8);
    }
}
