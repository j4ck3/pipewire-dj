import { useEffect, useRef, useState } from "react";
import { api, Devices, Settings, Status } from "../api";

interface Props {
  settings: Settings;
  save: (s: Settings) => void;
  status: Status | null;
}

/** Peak meter with ~1 s decay, fed by per-poll peaks. */
function Meter({ value }: { value: number }) {
  const held = useRef(0);
  const [v, setV] = useState(0);
  useEffect(() => {
    held.current = Math.max(value, held.current * 0.85);
    setV(held.current);
  }, [value]);
  const db = v > 0 ? 20 * Math.log10(v) : -60;
  const pct = Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
  return (
    <div className="meter" title={`${db.toFixed(1)} dBFS`}>
      <div className={db > -1 ? "fill hot" : db > -9 ? "fill warm" : "fill"} style={{ width: `${pct}%` }} />
    </div>
  );
}

function Channel(props: { label: string; value: number; onChange: (v: number) => void; meter: number; max?: number; children?: React.ReactNode }) {
  const { label, value, onChange, meter, max = 2, children } = props;
  return (
    <div className="channel">
      <div className="ch-head">
        <span>{label}</span>
        <span className="mono">{Math.round(value * 100)}%</span>
      </div>
      <input type="range" min={0} max={max} step={0.01} value={value} onChange={(e) => onChange(+e.target.value)} />
      <Meter value={meter} />
      {children}
    </div>
  );
}

export default function Mixer({ settings: s, save, status }: Props) {
  const [dev, setDev] = useState<Devices | null>(null);
  useEffect(() => {
    const load = () => api.devices().then(setDev).catch(() => {});
    load();
    const t = setInterval(load, 2000);
    return () => clearInterval(t);
  }, []);
  const m = status?.meters ?? { mic: 0, track: 0, apps: 0, out: 0, monitor: 0 };
  const set = (patch: Partial<Settings>) => save({ ...s, ...patch });

  return (
    <div className="page">
      <h2>Mixer</h2>
      <p className="muted">
        Everything below is mixed into the <b>PipeWire DJ</b> microphone. Pick it as the input device in your game, Discord or OBS.
      </p>
      <div className="channels">
        <Channel label="Mic" value={s.mic_volume} onChange={(v) => set({ mic_volume: v })} meter={m.mic}>
          <label className="check">
            <input type="checkbox" checked={s.mic_mute} onChange={(e) => set({ mic_mute: e.target.checked })} /> Mute
          </label>
          <label className="check">
            <input type="checkbox" checked={s.denoise} onChange={(e) => set({ denoise: e.target.checked })} /> Noise suppression (RNNoise)
          </label>
        </Channel>
        <Channel label="Tracks" value={s.track_volume} onChange={(v) => set({ track_volume: v })} meter={m.track} />
        <Channel label="Apps" value={s.apps_volume} onChange={(v) => set({ apps_volume: v })} meter={m.apps} />
        <Channel label="Monitor (you)" value={s.monitor_volume} onChange={(v) => set({ monitor_volume: v })} meter={m.monitor}>
          <label className="check">
            <input type="checkbox" checked={s.monitor_enabled} onChange={(e) => set({ monitor_enabled: e.target.checked })} /> Enabled
          </label>
        </Channel>
        <div className="channel out">
          <div className="ch-head">
            <span>Mic output</span>
            <span className={status?.mic_open ? "pill on" : "pill"}>{status?.mic_open ? "voice open" : "voice gated"}</span>
          </div>
          <Meter value={m.out} />
          <label className="check">
            <input type="checkbox" checked={s.limiter} onChange={(e) => set({ limiter: e.target.checked })} /> Limiter (-1 dBFS)
          </label>
        </div>
      </div>

      <h3>Apps mixed into the mic</h3>
      <p className="muted">You still hear these normally. Only apps currently playing audio (or previously selected) are listed.</p>
      <div className="apps">
        {dev?.apps.length === 0 && <div className="muted">No apps are playing audio right now.</div>}
        {dev?.apps.map((a) => (
          <label key={a} className="check app-pick">
            <input
              type="checkbox"
              checked={s.apps.includes(a)}
              onChange={(e) => set({ apps: e.target.checked ? [...s.apps, a] : s.apps.filter((x) => x !== a) })}
            />
            {a}
          </label>
        ))}
      </div>
    </div>
  );
}
