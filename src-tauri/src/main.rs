// PipeWire DJ: mix tracks, your mic and app audio into one PipeWire virtual microphone.
// Copyright (C) 2026 PipeWire DJ contributors. Licensed under GPL-3.0-or-later.

mod app;
mod commands;
mod engine;
mod games;
mod library;
mod services;

use std::sync::Arc;
use tauri::Manager;

const USAGE: &str = "usage: pipewire-dj [import <file|url>|toggle|stop|restart|load <n|tag>|play <n|tag>|ptt-on|ptt-off|mute|list|status|show|--restore]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    if let Some(first) = args.first() {
        match first.as_str() {
            "--restore" => {
                let paths = library::Paths::new();
                let ok = engine::defaults::restore(&paths.data, engine::pw::SOURCE_NAME);
                if let Some(i) = games::cs2::find_install() {
                    let _ = games::cs2::write_state(&i, false);
                }
                println!("{}", if ok { "default source restored" } else { "restore failed" });
                std::process::exit(if ok { 0 } else { 1 });
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ => {
                // CLI mode: forward to the running instance.
                match rt.block_on(services::send_to_running(&args.join(" "))) {
                    Ok(reply) => {
                        println!("{reply}");
                        std::process::exit(if reply.starts_with("error") { 1 } else { 0 });
                    }
                    Err(e) => {
                        eprintln!("pipewire-dj is not running ({e})");
                        std::process::exit(2);
                    }
                }
            }
        }
    }

    // Single instance: if another one answers, ask it to show its window.
    if rt.block_on(services::send_to_running("show")).is_ok() {
        eprintln!("pipewire-dj is already running");
        return;
    }

    let app = match rt.block_on(async { App::new() }) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("pipewire-dj: failed to start audio engine: {e}");
            std::process::exit(1);
        }
    };
    app.write_game_cfgs();

    let _guard = rt.enter();
    tauri::async_runtime::set(rt.handle().clone());
    ctrlc_restore(app.clone());

    // Restore the default source even if the UI panics (e.g. no display).
    let app_for_panic = app.clone();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        app_for_panic.shutdown();
        let _ = std::fs::remove_file(services::socket_path());
        prev_hook(info);
    }));

    let app_for_exit = app.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(app.clone())
        .setup(move |tapp| {
            let handle = tapp.handle().clone();
            let a = app.clone();
            a.ui_ready.store(true, std::sync::atomic::Ordering::SeqCst);
            a.apply_settings();
            tauri::async_runtime::spawn(services::cs2_watch(a.clone()));
            tauri::async_runtime::spawn(services::control_socket(a.clone(), move || {
                if let Some(w) = handle.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }));
            let b = a.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = services::shortcuts(b).await {
                    eprintln!("pipewire-dj: global shortcuts unavailable ({e}); use `pipewire-dj toggle` etc. in your compositor binds");
                }
            });
            // Re-match selected apps as streams come and go.
            let c = a.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    c.sync_apps();
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_settings,
            commands::set_settings,
            commands::get_devices,
            commands::get_library,
            commands::update_track,
            commands::delete_track,
            commands::move_track,
            commands::import_files,
            commands::import_youtube,
            commands::load_track,
            commands::play_track,
            commands::transport,
            commands::set_ptt,
            commands::waveform,
            commands::write_game_config,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |_, ev| {
            if let tauri::RunEvent::Exit = ev {
                app_for_exit.shutdown();
                let _ = std::fs::remove_file(services::socket_path());
            }
        });
}

use app::App;

/// Restore the default source on SIGINT/SIGTERM as well as on normal exit.
fn ctrlc_restore(app: Arc<App>) {
    tauri::async_runtime::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut a), Ok(mut b)) = (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) else { return };
        tokio::select! { _ = a.recv() => {}, _ = b.recv() => {} }
        app.shutdown();
        let _ = std::fs::remove_file(services::socket_path());
        std::process::exit(0);
    });
}
