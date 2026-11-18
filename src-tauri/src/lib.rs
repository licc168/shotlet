mod capture;
#[cfg(target_os = "macos")]
mod hotkey_mac;
mod commands;
mod history;
mod ocr;
mod record;
mod record_audio;
mod record_cursor;
mod scroll;
mod settings;
mod state;
mod stitch;
mod ui;
mod util;
mod windows_list;

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
            commands::pin_file,
            commands::pin_png_base64,
            commands::pin_path,
            commands::read_image_bytes,
            commands::ocr_png_base64,
            commands::open_editor_record,
            commands::current_editor,
            commands::scroll_capture,
            commands::ffmpeg_available,
            commands::start_recording,
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
            let preferences = settings::load(&data);
            app.manage(AppState::new(preferences.clone()));
            if let Err(error) = ui::install_tray(app.handle()) {
                eprintln!("tray: {error}");
            }
            #[cfg(target_os = "macos")]
            capture::prepare_capture_access();
            commands::shortcut_log(app.handle(), "setup complete");
            if let Err(error) = ui::prepare_overlay(app.handle()) {
                eprintln!("overlay: {error}");
            }
            ui::begin_follow(app.handle());
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
