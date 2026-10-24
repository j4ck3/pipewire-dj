import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, fmtTime, Status, Track } from "../api";

const MEDIA = ["mp3", "wav", "ogg", "opus", "flac", "aac", "m4a", "wma", "webm", "mp4", "mkv", "avi", "mov", "wmv", "m4v"];

interface Props {
  tracks: Track[];
  status: Status | null;
  refresh: () => void;
  onError: (e: string) => void;
}

export default function Library({ tracks, status, refresh, onError }: Props) {
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [url, setUrl] = useState("");
  const [editing, setEditing] = useState<number | null>(null);

  const importFiles = async () => {
    const sel = await open({ multiple: true, filters: [{ name: "Audio / video", extensions: MEDIA }] });
    const paths = Array.isArray(sel) ? sel : sel ? [sel] : [];
    if (!paths.length) return;
    setBusy(`Importing ${paths.length} file(s)…`);
    try {
      const errs = await api.importFiles(paths);
      if (errs.length) onError(errs.join("\n"));
    } catch (e) {
      onError(String(e));
    }
    setBusy(null);
    refresh();
  };

  const importUrl = async () => {
    if (!url.trim()) return;
    setBusy("Downloading…");
    try {
      await api.importYoutube(url);
      setUrl("");
    } catch (e) {
      onError(String(e));
    }
    setBusy(null);
    refresh();
  };

  const f = filter.toLowerCase();
  const shown = tracks.filter(
    (t) => !f || t.name.toLowerCase().includes(f) || t.tags.some((g) => g.toLowerCase().includes(f)) || String(t.id) === f,
  );
  const pb = status?.playback;
  const act = (p: Promise<unknown>) => p.catch((e) => onError(String(e)));

  return (
    <div className="page">
      <div className="toolbar">
        <button onClick={importFiles} disabled={!!busy}>
          Import files
        </button>
        <div className="url-field grow">
          <input
            placeholder="YouTube or other URL (yt-dlp)"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && importUrl()}
          />
          <button
            type="button"
            className="link"
            title="Open the list of sites yt-dlp supports"
            onClick={() => api.openSupportedSites().catch((e) => onError(String(e)))}
          >
            Supported sites ↗
          </button>
        </div>
        <button onClick={importUrl} disabled={!!busy || !url.trim()}>
          Import URL
        </button>
        <input placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} />
      </div>
      {busy && <div className="info">{busy}</div>}
      {tracks.length === 0 && <div className="empty">No tracks yet. Import some audio or video files.</div>}
      <table className="tracks">
        <thead>
          <tr>
            <th>#</th>
            <th>Name</th>
            <th>Tags</th>
            <th>Bind</th>
            <th>Length</th>
            <th>Vol</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {shown.map((t) => (
            <TrackRow
              key={t.id}
              t={t}
              loaded={pb?.loaded === t.id}
              playing={pb?.loaded === t.id && !!pb?.playing}
              editing={editing === t.id}
              setEditing={(on) => setEditing(on ? t.id : null)}
              refresh={refresh}
              act={act}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

function TrackRow(props: {
  t: Track;
  loaded: boolean;
  playing: boolean;
  editing: boolean;
  setEditing: (on: boolean) => void;
  refresh: () => void;
  act: (p: Promise<unknown>) => void;
}) {
  const { t, loaded, playing, editing, setEditing, refresh, act } = props;
  const len = (t.trim_end ?? t.duration) - t.trim_start;
  return (
    <>
      <tr className={loaded ? "loaded" : ""} onDoubleClick={() => act(api.load(t.id))}>
        <td className="num">{t.id}</td>
        <td>{t.name}</td>
        <td>
          {t.tags.map((g) => (
            <span key={g} className="tag">
              {g}
            </span>
          ))}
        </td>
        <td className="mono">{t.bind ? `${t.bind}${t.instant ? " ▶" : ""}` : ""}</td>
        <td className="mono">{fmtTime(len)}</td>
        <td className="mono">{Math.round(t.volume * 100)}%</td>
        <td className="actions">
          <button onClick={() => act(api.load(t.id))} title="Load (play key starts it)">
            Load
          </button>
          <button onClick={() => act(playing ? api.transport("toggle") : api.play(t.id))}>{playing ? "Pause" : "Play"}</button>
          <button onClick={() => setEditing(!editing)}>{editing ? "Close" : "Edit"}</button>
          <button onClick={() => act(api.moveTrack(t.id, -1).then(refresh))} title="Move up">
            ↑
          </button>
          <button onClick={() => act(api.moveTrack(t.id, 1).then(refresh))} title="Move down">
            ↓
          </button>
        </td>
      </tr>
      {editing && (
        <tr className="editor-row">
          <td colSpan={7}>
            <TrackEditor t={t} refresh={refresh} act={act} close={() => setEditing(false)} />
          </td>
        </tr>
      )}
    </>
  );
}

function TrackEditor({ t, refresh, act, close }: { t: Track; refresh: () => void; act: (p: Promise<unknown>) => void; close: () => void }) {
  const [d, setD] = useState<Track>(t);
  const [tags, setTags] = useState(t.tags.join(", "));
  useEffect(() => {
    setD(t);
    setTags(t.tags.join(", "));
  }, [t]);

  const save = () =>
    act(
      api
        .updateTrack({ ...d, tags: tags.split(",").map((s) => s.trim()).filter(Boolean) })
        .then(refresh),
    );
  const del = () => {
    if (confirm(`Delete "${t.name}"? Track number ${t.id} will not be reused.`)) {
      act(api.deleteTrack(t.id).then(() => (close(), refresh())));
    }
  };

  return (
    <div className="editor">
      <div className="grid">
        <label>
          Name
          <input value={d.name} onChange={(e) => setD({ ...d, name: e.target.value })} />
        </label>
        <label>
          Tags (comma separated, become dj_&lt;tag&gt;)
          <input value={tags} onChange={(e) => setTags(e.target.value)} />
        </label>
        <label>
          In-game bind key
          <input placeholder="e.g. kp_1, f5" value={d.bind ?? ""} onChange={(e) => setD({ ...d, bind: e.target.value || null })} />
        </label>
        <label className="check">
          <input type="checkbox" checked={d.instant} onChange={(e) => setD({ ...d, instant: e.target.checked })} />
          Bind plays immediately (instead of only loading)
        </label>
        <label>
          Volume {Math.round(d.volume * 100)}%
          <input type="range" min={0} max={2} step={0.01} value={d.volume} onChange={(e) => setD({ ...d, volume: +e.target.value })} />
        </label>
      </div>
      <Trim t={t} d={d} setD={setD} />
      <div className="row">
        <button className="primary" onClick={save}>
          Save
        </button>
        <button onClick={() => act(api.play(t.id))}>Preview</button>
        <div className="grow" />
        <button className="danger" onClick={del}>
          Delete
        </button>
      </div>
    </div>
  );
}

function Trim({ t, d, setD }: { t: Track; d: Track; setD: (t: Track) => void }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [wave, setWave] = useState<number[]>([]);
  const drag = useRef<"start" | "end" | null>(null);

  useEffect(() => {
    api.waveform(t.id, 600).then(setWave).catch(() => setWave([]));
  }, [t.id]);

  const end = d.trim_end ?? d.duration;
  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const ctx = c.getContext("2d")!;
    const { width: w, height: h } = c;
    ctx.clearRect(0, 0, w, h);
    const sx = (d.trim_start / d.duration) * w;
    const ex = (end / d.duration) * w;
    wave.forEach((v, i) => {
      const x = (i / wave.length) * w;
      ctx.fillStyle = x >= sx && x <= ex ? "#3ddc97" : "#3a4450";
      const bh = Math.max(1, v * h);
      ctx.fillRect(x, (h - bh) / 2, Math.max(1, w / wave.length - 0.5), bh);
    });
    ctx.fillStyle = "#f5f7fa";
    ctx.fillRect(sx - 1, 0, 2, h);
    ctx.fillRect(ex - 1, 0, 2, h);
  }, [wave, d.trim_start, end, d.duration]);

  const toTime = (e: React.PointerEvent) => {
    const r = canvas.current!.getBoundingClientRect();
    return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)) * d.duration;
  };
  const onDown = (e: React.PointerEvent) => {
    const tm = toTime(e);
    drag.current = Math.abs(tm - d.trim_start) < Math.abs(tm - end) ? "start" : "end";
    (e.target as Element).setPointerCapture(e.pointerId);
    onMove(e);
  };
  const onMove = (e: React.PointerEvent) => {
    if (!drag.current) return;
    const tm = toTime(e);
    if (drag.current === "start") setD({ ...d, trim_start: Math.min(tm, end - 0.05) });
    else setD({ ...d, trim_end: Math.max(tm, d.trim_start + 0.05) });
  };

  return (
    <div className="trim">
      <canvas
        ref={canvas}
        width={1200}
        height={90}
        onPointerDown={onDown}
        onPointerMove={onMove}
        onPointerUp={() => (drag.current = null)}
      />
      <div className="row muted">
        Trim {fmtTime(d.trim_start)} – {fmtTime(end)} (of {fmtTime(d.duration)}). Drag the markers.
        <button onClick={() => setD({ ...d, trim_start: 0, trim_end: null })}>Reset trim</button>
      </div>
    </div>
  );
}
