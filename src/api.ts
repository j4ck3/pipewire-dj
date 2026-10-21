import { invoke } from "@tauri-apps/api/core";

export type GateMode = "off" | "always" | "while_game";

export interface Track {
  id: number;
  name: string;
  tags: string[];
  volume: number;
  trim_start: number;
  trim_end: number | null;
  duration: number;
  bind: string | null;
  instant: boolean;
  source: string;
}

export interface Settings {
  mic_device: string | null;
  monitor_enabled: boolean;
  monitor_device: string | null;
  set_default_source: boolean;
  apps: string[];
  mic_volume: number;
  mic_mute: boolean;
  denoise: boolean;
  track_volume: number;
  apps_volume: number;
  monitor_volume: number;
  pause_on_toggle: boolean;
  gate_mode: GateMode;
  open_mic_while_track: boolean;
  limiter: boolean;
  cs2_enabled: boolean;
  cs2_play_key: string;
  cs2_ptt_key: string | null;
}

export interface Node {
  id: number;
  serial: number;
  name: string;
  description: string;
  class: string;
  app: string;
}

export interface Devices {
  sources: Node[];
  sinks: Node[];
  apps: string[];
}

export interface GameStatus {
  install: string | null;
  running: boolean;
  log: string | null;
  log_active: boolean;
  last_event: string | null;
  ptt_key: string | null;
  launch_options: string;
  cfg_written: boolean;
  ptt: boolean;
}

export interface Status {
  playback: { loaded: number | null; playing: boolean; position: number; start: number; end: number };
  meters: { mic: number; track: number; apps: number; out: number; monitor: number };
  ptt: boolean;
  mic_open: boolean;
  game: GameStatus;
}

export const api = {
  status: () => invoke<Status>("get_status"),
  settings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  devices: () => invoke<Devices>("get_devices"),
  library: () => invoke<Track[]>("get_library"),
  updateTrack: (track: Track) => invoke<void>("update_track", { track }),
  deleteTrack: (id: number) => invoke<void>("delete_track", { id }),
  moveTrack: (id: number, delta: number) => invoke<void>("move_track", { id, delta }),
  importFiles: (paths: string[]) => invoke<string[]>("import_files", { paths }),
  importYoutube: (url: string) => invoke<Track>("import_youtube", { url }),
  load: (id: number) => invoke<void>("load_track", { id }),
  play: (id: number) => invoke<void>("play_track", { id }),
  transport: (action: "toggle" | "stop" | "restart") => invoke<void>("transport", { action }),
  setPtt: (on: boolean) => invoke<void>("set_ptt", { on }),
  waveform: (id: number, buckets: number) => invoke<number[]>("waveform", { id, buckets }),
  writeGameConfig: () => invoke<void>("write_game_config"),
};

export const fmtTime = (s: number) => {
  const m = Math.floor(s / 60);
  const r = s - m * 60;
  return `${m}:${r.toFixed(1).padStart(4, "0")}`;
};
