import { useEffect, useState } from "react";
import { api, Devices, GateMode, Settings } from "../api";

interface Props {
  settings: Settings;
  save: (s: Settings) => void;
}

export default function SettingsPage({ settings: s, save }: Props) {
  const [dev, setDev] = useState<Devices | null>(null);
  useEffect(() => {
    api.devices().then(setDev).catch(() => {});
  }, []);
  const set = (patch: Partial<Settings>) => save({ ...s, ...patch });

  return (
    <div className="page">
      <h2>Settings</h2>
      <div className="grid">
        <label>
          Real microphone
          <select value={s.mic_device ?? ""} onChange={(e) => set({ mic_device: e.target.value || null })}>
            <option value="">System default (at startup)</option>
            {dev?.sources.map((n) => (
              <option key={n.id} value={n.name}>
                {n.description || n.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Monitor output
          <select value={s.monitor_device ?? ""} onChange={(e) => set({ monitor_device: e.target.value || null })}>
            <option value="">System default</option>
            {dev?.sinks.map((n) => (
              <option key={n.id} value={n.name}>
                {n.description || n.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Mic gate
          <select value={s.gate_mode} onChange={(e) => set({ gate_mode: e.target.value as GateMode })}>
            <option value="while_game">Push-to-talk only while a game is running</option>
            <option value="always">Always push-to-talk</option>
            <option value="off">Always open</option>
          </select>
        </label>
      </div>
      <div className="checks">
        <label className="check">
          <input type="checkbox" checked={s.set_default_source} onChange={(e) => set({ set_default_source: e.target.checked })} />
          Make PipeWire DJ the default microphone while running (restored on exit)
        </label>
        <label className="check">
          <input type="checkbox" checked={s.pause_on_toggle} onChange={(e) => set({ pause_on_toggle: e.target.checked })} />
          Play key pauses and resumes (off: stopping rewinds to the start)
        </label>
        <label className="check">
          <input type="checkbox" checked={s.open_mic_while_track} onChange={(e) => set({ open_mic_while_track: e.target.checked })} />
          Let my voice through while a track plays (talk over tracks without holding push-to-talk)
        </label>
      </div>

      <h3>Desktop shortcuts</h3>
      <p className="muted">
        Shortcuts are registered through the desktop portal (Hyprland, KDE, GNOME). On Hyprland, bind them in hyprland.conf, for example{" "}
        <code>bind = , F9, global, :toggle</code>. Any compositor can also run the CLI:
      </p>
      <pre>{`pipewire-dj toggle | stop | restart | load <n|tag> | play <n|tag>
pipewire-dj ptt-on | ptt-off | mute | list | status
pipewire-dj --restore   # reset the default mic after a crash`}</pre>
    </div>
  );
}
