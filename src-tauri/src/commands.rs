//! Tauri commands called by the React UI.

use crate::app::{app_label, App, Settings, Status};
use crate::engine::pw::Node;
use crate::library::Track;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

type R<T> = Result<T, String>;
fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

#[derive(Serialize)]
pub struct Devices {
    pub sources: Vec<Node>,
    pub sinks: Vec<Node>,
    pub apps: Vec<String>,
}

#[tauri::command]
pub fn get_status(app: State<Arc<App>>) -> Status {
    app.status()
}

#[tauri::command]
pub fn get_settings(app: State<Arc<App>>) -> Settings {
    app.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_settings(app: State<Arc<App>>, settings: Settings) -> R<()> {
    app.save_settings(settings).map_err(e)
}

#[tauri::command]
pub fn get_devices(app: State<Arc<App>>) -> Devices {
    let nodes = app.devices();
    let ours = |n: &Node| n.name.starts_with("pipewire_dj");
    let mut sources: Vec<Node> = nodes.iter().filter(|n| n.class == "Audio/Source" && !ours(n)).cloned().collect();
    let mut sinks: Vec<Node> = nodes.iter().filter(|n| n.class == "Audio/Sink").cloned().collect();
    let mut apps: Vec<String> = nodes
        .iter()
        .filter(|n| n.class == "Stream/Output/Audio" && !ours(n))
        .map(app_label)
        .collect();
    // Keep previously selected apps visible even when not currently playing.
    apps.extend(app.settings.lock().unwrap().apps.iter().cloned());
    apps.sort();
    apps.dedup();
    sources.sort_by(|a, b| a.description.cmp(&b.description));
    sinks.sort_by(|a, b| a.description.cmp(&b.description));
    Devices { sources, sinks, apps }
}

#[tauri::command]
pub fn get_library(app: State<Arc<App>>) -> Vec<Track> {
    app.library.lock().unwrap().tracks.clone()
}

#[tauri::command]
pub fn update_track(app: State<Arc<App>>, track: Track) -> R<()> {
    {
        let mut l = app.library.lock().unwrap();
        let t = l.get_mut(track.id).ok_or("no such track")?;
        let duration = t.duration;
        *t = Track { duration, ..track.clone() };
        t.tags.retain(|g| !g.trim().is_empty());
        t.volume = t.volume.clamp(0.0, 4.0);
        t.trim_start = t.trim_start.clamp(0.0, duration);
        t.trim_end = t.trim_end.map(|x| x.clamp(t.trim_start, duration));
        t.bind = t.bind.take().map(|b| b.trim().to_lowercase()).filter(|b| !b.is_empty());
    }
    app.save_library().map_err(e)?;
    app.track_changed(track.id);
    Ok(())
}

#[tauri::command]
pub fn delete_track(app: State<Arc<App>>, id: u64) -> R<()> {
    app.library.lock().unwrap().tracks.retain(|t| t.id != id);
    app.forget_track(id);
    app.save_library().map_err(e)
}

#[tauri::command]
pub fn move_track(app: State<Arc<App>>, id: u64, delta: i32) -> R<()> {
    {
        let mut l = app.library.lock().unwrap();
        let Some(i) = l.tracks.iter().position(|t| t.id == id) else { return Err("no such track".into()) };
        let j = (i as i32 + delta).clamp(0, l.tracks.len() as i32 - 1) as usize;
        let t = l.tracks.remove(i);
        l.tracks.insert(j, t);
    }
    app.save_library().map_err(e)
}

#[tauri::command]
pub async fn import_files(app: State<'_, Arc<App>>, paths: Vec<String>) -> R<Vec<String>> {
    let mut errors = vec![];
    for p in paths {
        let pb = PathBuf::from(&p);
        if let Err(x) = app.import_file(pb, None, p.clone()).await {
            errors.push(format!("{p}: {x}"));
        }
    }
    Ok(errors)
}

#[tauri::command]
pub async fn import_youtube(app: State<'_, Arc<App>>, url: String) -> R<Track> {
    app.import_youtube(url.trim()).await.map_err(e)
}

#[tauri::command]
pub fn load_track(app: State<Arc<App>>, id: u64) -> R<()> {
    app.load(&id.to_string()).map(|_| ()).map_err(e)
}

#[tauri::command]
pub fn play_track(app: State<Arc<App>>, id: u64) -> R<()> {
    app.play(&id.to_string()).map_err(e)
}

#[tauri::command]
pub fn transport(app: State<Arc<App>>, action: String) -> R<()> {
    match action.as_str() {
        "toggle" => app.toggle(),
        "stop" => app.stop(),
        "restart" => app.restart(),
        _ => return Err(format!("unknown action {action}")),
    }
    Ok(())
}

#[tauri::command]
pub fn set_ptt(app: State<Arc<App>>, on: bool) {
    app.on_game_event(crate::games::cs2::GameEvent::Ptt(on));
}

#[tauri::command]
pub fn waveform(app: State<Arc<App>>, id: u64, buckets: usize) -> R<Vec<f32>> {
    app.waveform(id, buckets.clamp(16, 4000)).map_err(e)
}

#[tauri::command]
pub fn write_game_config(app: State<Arc<App>>) -> R<()> {
    app.write_game_cfgs();
    if app.game.lock().unwrap().cfg_written {
        Ok(())
    } else {
        Err("CS2 install not found or cfg folder not writable".into())
    }
}
