import { api, Settings, Status } from "../api";

interface Props {
  settings: Settings;
  save: (s: Settings) => void;
  status: Status | null;
  onError: (e: string) => void;
}

function Copy({ text }: { text: string }) {
  return (
    <div className="copy">
      <code>{text}</code>
      <button onClick={() => navigator.clipboard.writeText(text)}>Copy</button>
    </div>
  );
}

export default function Games({ settings: s, save, status, onError }: Props) {
  const g = status?.game;
  const set = (patch: Partial<Settings>) => save({ ...s, ...patch });
  const ok = (b: boolean | undefined) => (b ? "pill on" : "pill");

  return (
    <div className="page">
      <h2>Counter-Strike 2</h2>
      <div className="row">
        <label className="check">
          <input type="checkbox" checked={s.cs2_enabled} onChange={(e) => set({ cs2_enabled: e.target.checked })} /> Enable CS2 integration
        </label>
      </div>

      <div className="status-grid">
        <span>Install</span>
        <span className={ok(!!g?.install)}>{g?.install ?? "not found"}</span>
        <span>Game</span>
        <span className={ok(g?.running)}>{g?.running ? "running" : "not running"}</span>
        <span>Console log</span>
        <span className={ok(g?.log_active)}>{g?.log_active ? "receiving" : g?.log ? "found, idle" : "not found yet"}</span>
        <span>Config</span>
        <span className={ok(g?.cfg_written)}>{g?.cfg_written ? "dj.cfg written" : "not written"}</span>
        <span>Push-to-talk key</span>
        <span className="pill">{g?.ptt_key ?? "not detected"}</span>
        <span>Last event</span>
        <span className="mono">{g?.last_event ?? "–"}</span>
      </div>

      <h3>One-time setup</h3>
      <ol className="steps">
        <li>
          Add to CS2's Steam launch options (keep your existing ones):
          <Copy text={g?.launch_options ?? "-con_logfile pipewire_dj.log"} />
        </li>
        <li>
          Add this line to <code>game/csgo/cfg/autoexec.cfg</code> (create it if missing):
          <Copy text="exec dj" />
        </li>
        <li>
          In CS2 audio settings, set the voice input device to <b>PipeWire DJ</b>, or leave it on the default device (PipeWire DJ is the system default while this app runs).
        </li>
      </ol>

      <h3>How it works in game</h3>
      <ul className="muted">
        <li>
          <code>dj_list</code> prints your tracks. <code>dj_3</code> or <code>dj_&lt;tag&gt;</code> loads a track. The play key toggles it.
        </li>
        <li>
          While the app runs, CS2 is put into open-mic mode and the app decides what goes out: your voice while you hold push-to-talk, and the track while it plays. You can talk over a playing track.
        </li>
        <li>CS2 cannot be controlled from outside, so after closing the app CS2 returns to normal push-to-talk on your next key press.</li>
      </ul>

      <h3>Keys</h3>
      <div className="grid">
        <label>
          Play / pause key
          <input value={s.cs2_play_key} onChange={(e) => set({ cs2_play_key: e.target.value.trim().toLowerCase() })} />
        </label>
        <label>
          Push-to-talk key (blank = detect from your CS2 binds)
          <input
            placeholder={g?.ptt_key ?? "k"}
            value={s.cs2_ptt_key ?? ""}
            onChange={(e) => set({ cs2_ptt_key: e.target.value.trim().toLowerCase() || null })}
          />
        </label>
      </div>
      <div className="row">
        <button onClick={() => api.writeGameConfig().catch((e) => onError(String(e)))}>Rewrite dj.cfg</button>
        <span className="muted">After changing keys or tracks while CS2 is running, type <code>exec dj</code> in the console.</span>
      </div>
    </div>
  );
}
