import { useCallback, useEffect, useState } from "react";
import { api, fmtTime, Settings, Status, Track } from "./api";
import Library from "./pages/Library";
import Mixer from "./pages/Mixer";
import Games from "./pages/Games";
import SettingsPage from "./pages/Settings";

const PAGES = ["Library", "Mixer", "Games", "Settings"] as const;
type Page = (typeof PAGES)[number];

export default function App() {
  const [page, setPage] = useState<Page>("Library");
  const [status, setStatus] = useState<Status | null>(null);
  const [settings, setSettingsState] = useState<Settings | null>(null);
  const [tracks, setTracks] = useState<Track[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refreshLibrary = useCallback(() => api.library().then(setTracks).catch((e) => setError(String(e))), []);

  useEffect(() => {
    api.settings().then(setSettingsState).catch((e) => setError(String(e)));
    refreshLibrary();
    let alive = true;
    const tick = async () => {
      try {
        const s = await api.status();
        if (alive) setStatus(s);
      } catch {
        /* backend busy; next tick */
      }
    };
    const t = setInterval(tick, 50);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [refreshLibrary]);

  const saveSettings = async (s: Settings) => {
    setSettingsState(s);
    try {
      await api.setSettings(s);
    } catch (e) {
      setError(String(e));
    }
  };

  const pb = status?.playback;
  const loaded = tracks.find((t) => t.id === pb?.loaded);
  const run = (p: Promise<unknown>) => p.catch((e) => setError(String(e)));

  return (
    <div className="app">
      <nav className="sidebar">
        <div className="brand">PipeWire DJ</div>
        {PAGES.map((p) => (
          <button key={p} className={page === p ? "nav active" : "nav"} onClick={() => setPage(p)}>
            {p}
          </button>
        ))}
        <div className="spacer" />
        <div className={status?.ptt ? "pill on" : "pill"}>{status?.ptt ? "Talking" : "PTT idle"}</div>
        {settings?.mic_mute && <div className="pill warn">Mic muted</div>}
      </nav>
      <main>
        {error && (
          <div className="error" onClick={() => setError(null)}>
            {error} <span className="muted">(click to dismiss)</span>
          </div>
        )}
        {page === "Library" && <Library tracks={tracks} status={status} refresh={refreshLibrary} onError={setError} />}
        {page === "Mixer" && settings && <Mixer settings={settings} save={saveSettings} status={status} />}
        {page === "Games" && settings && <Games settings={settings} save={saveSettings} status={status} onError={setError} />}
        {page === "Settings" && settings && <SettingsPage settings={settings} save={saveSettings} />}
      </main>
      <footer className="transport">
        <div className="now">
          <div className="title">{loaded ? `${loaded.id}. ${loaded.name}` : "No track loaded"}</div>
          {pb && loaded && (
            <div className="muted">
              {fmtTime(Math.max(0, pb.position - pb.start))} / {fmtTime(pb.end - pb.start)}
            </div>
          )}
        </div>
        <div className="progress">
          <div
            className="bar"
            style={{ width: pb && pb.end > pb.start ? `${((pb.position - pb.start) / (pb.end - pb.start)) * 100}%` : "0%" }}
          />
        </div>
        <button onClick={() => run(api.transport("toggle"))} disabled={!loaded}>
          {pb?.playing ? "Pause" : "Play"}
        </button>
        <button onClick={() => run(api.transport("restart"))} disabled={!loaded}>
          Restart
        </button>
        <button onClick={() => run(api.transport("stop"))} disabled={!loaded}>
          Stop
        </button>
        <button
          className="ptt"
          onPointerDown={() => run(api.setPtt(true))}
          onPointerUp={() => run(api.setPtt(false))}
          onPointerLeave={() => status?.ptt && run(api.setPtt(false))}
        >
          Hold to talk
        </button>
      </footer>
    </div>
  );
}
