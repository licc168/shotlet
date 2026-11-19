pub use shotlet_core::{capture, history, ocr, record, scroll, settings, state, util};

#[cfg(target_os = "macos")]
mod hotkey_mac;
mod commands;
mod ui;

use tauri::{Manager, RunEvent, WindowEvent};
use tauri_plugin_global_shortcut::ShortcutState;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    let state = match event.state() {
                        ShortcutState::Pressed => ShortcutState::Pressed,
                        ShortcutState::Released => ShortcutState::Released,
                    };
                    commands::on_shortcut(app, shortcut, state);
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::trace_mark,
            commands::start_capture,
            commands::overlay_ready,
            commands::set_overlay_follow,
            commands::aim_overlay,
            commands::copy_capture_region,
            commands::current_capture,
            commands::cancel_capture,
            commands::open_history,
            commands::open_settings,
            commands::get_settings,
            commands::save_settings,
            commands::list_history,
            commands::commit_history,
            commands::update_history,
            commands::set_favorite,
            commands::set_tags,
            commands::rename_capture,
            commands::delete_captures,
            commands::restore_captures,
            commands::purge_deleted,
            commands::export_captures,
            commands::copy_file,
            commands::write_png_base64,
            commands::copy_png_base64,
            commands::copy_annotated,
            commands::pin_file,
            commands::pin_png_base64,
            commands::pin_capture_region,
            commands::pin_annotated,
            commands::pin_path,
            commands::pin_ready,
            commands::pin_standby,
            commands::read_image_bytes,
            commands::preview_rgba,
            commands::ocr_png_base64,
            commands::open_editor_record,
            commands::current_editor,
            commands::scroll_capture,
            commands::scroll_finish,
            commands::scroll_cancel,
            commands::scroll_progress,
            commands::ffmpeg_available,
            commands::open_recording,
            commands::begin_recording,
            commands::set_recording_paused,
            commands::recording_status,
            commands::stop_recording,
            commands::storage_dir,
            commands::reveal_path,
        ])
        .setup(|app| {
            let data = app.path().app_data_dir().unwrap_or_else(|_| {
                dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("shotlet")
            });
            let _ = std::fs::create_dir_all(&data);
            shotlet_core::library::set_trace_file(data.join("shotlet.log"));
            let preferences = settings::load(&data);
            app.manage(AppState::new(preferences.clone()));
            if let Err(error) = ui::install_tray(app.handle()) {
                eprintln!("tray: {error}");
            }
            shotlet_core::capture_session::prepare_access();
            commands::shortcut_log(app.handle(), "setup complete");
            if let Err(error) = ui::prepare_overlay(app.handle()) {
                eprintln!("overlay: {error}");
            }
            ui::begin_follow(app.handle());
            ui::replenish_pin(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                WindowEvent::Destroyed if window.label() == "recorder" => {
                    let app = window.app_handle().clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = commands::stop_recording(app).await;
                    });
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Shotlet")
        .run(|app, event| {
            if let RunEvent::Ready = event {
                // Carbon hotkeys registered before NSApp is running come back as success
                // and then get dropped. Register only once the event loop is up.
                commands::register_shortcuts_when_ready(app);
            }
        });
}
