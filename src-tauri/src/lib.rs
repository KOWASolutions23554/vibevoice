mod audio;
mod clipboard;
mod config;
mod focus;
mod format;
mod hotkey;
mod transcription;

use audio::AudioHandle;
use config::{load_config, save_config, AppConfig};
use format::format_transcript;
use hotkey::{set_record_hotkey, start_hotkey_listener, stop_hotkey_listener, HotkeyAction};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Listener, Manager, RunEvent, State, WebviewWindow,
};
use tauri_plugin_autostart::ManagerExt;

#[cfg(windows)]
fn set_window_icon_native(hwnd_raw: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, SendMessageW, WM_SETICON};

    let rgba = include_bytes!("../icons/window-48.rgba");
    let width: u32 = 48;
    let height: u32 = 48;

    // CreateIcon expects BGRA with pre-multiplied alpha + a bitmask
    let mut bgra = rgba.to_vec();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2); // RGBA → BGRA
    }
    let mask = vec![0u8; (width * height / 8) as usize];

    unsafe {
        let hicon = CreateIcon(
            None,
            width as i32,
            height as i32,
            1,
            32,
            mask.as_ptr(),
            bgra.as_ptr(),
        );
        if let Ok(icon) = hicon {
            let hwnd = HWND(hwnd_raw as *mut _);
            SendMessageW(
                hwnd,
                WM_SETICON,
                windows::Win32::Foundation::WPARAM(1),
                windows::Win32::Foundation::LPARAM(icon.0 as isize),
            );
            SendMessageW(
                hwnd,
                WM_SETICON,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(icon.0 as isize),
            );
        }
    }
}

struct AppState {
    config: Arc<Mutex<AppConfig>>,
    audio: AudioHandle,
}

fn apply_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let autolaunch = app.autolaunch();
    if enabled {
        autolaunch.enable().map_err(|e| e.to_string())
    } else {
        autolaunch.disable().map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn save_config_cmd(
    config: AppConfig,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    apply_autostart(&app, config.autostart)?;
    save_config(&config)?;
    set_record_hotkey(&config.hotkey);
    let _ = app.emit("language-changed", config.language.clone());
    *state.config.lock().unwrap() = config;
    Ok(())
}

#[tauri::command]
async fn test_api_key(api_key: String) -> Result<(), String> {
    transcription::test_api_key(&api_key).await
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn position_overlay(overlay: &WebviewWindow, app: &AppHandle) {
    let monitor = if let Some((x, y)) = focus::target_monitor_center() {
        app.monitor_from_point(x as f64, y as f64).ok().flatten()
    } else {
        None
    }
    .or_else(|| overlay.current_monitor().ok().flatten());

    if let Some(monitor) = monitor {
        // Work area = monitor minus taskbar, so the overlay never hides behind it.
        let work = monitor.work_area();
        let window = overlay.outer_size().unwrap_or_default();
        let x = work.position.x + (work.size.width as i32 - window.width as i32) / 2;
        let bottom_margin = 16;
        let y = work.position.y + work.size.height as i32 - window.height as i32 - bottom_margin;
        let _ = overlay.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

fn show_overlay(app: &AppHandle) {
    if let Some(overlay) = app.get_webview_window("overlay") {
        position_overlay(&overlay, app);
        let _ = overlay.show();
    }
}

fn hide_overlay(app: &AppHandle) {
    let _ = app.emit("overlay-hide", ());
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.hide();
    }
}

// Ctrl+Alt flips between speaking German and having it translated to English.
// The overlay doubles as the confirmation, so the new mode is visible without
// opening settings.
fn toggle_language(app: &AppHandle) {
    let language = {
        let state = app.state::<AppState>();
        let mut config = state.config.lock().unwrap();
        config.language = if transcription::is_translate_language(&config.language) {
            "de".to_string()
        } else {
            "de-en".to_string()
        };
        if let Err(error) = save_config(&config) {
            eprintln!("Failed to save language: {error}");
        }
        config.language.clone()
    };

    let _ = app.emit("language-changed", language);

    if !hotkey::is_recording() {
        show_overlay(app);
    }
}

fn emit_pipeline_error(app: &AppHandle, message: &str) {
    eprintln!("{message}");
    let _ = app.emit("pipeline-error", message.to_string());
}

async fn process_recording(app: AppHandle) {
    let recording = {
        let state = app.state::<AppState>();
        match state.audio.stop_recording() {
            Ok(recording) => recording,
            Err(error) => {
                emit_pipeline_error(&app, &error);
                return;
            }
        }
    };

    if !recording.has_speech() {
        emit_pipeline_error(
            &app,
            "No speech detected. Hold the keys longer and speak closer to the mic.",
        );
        return;
    }

    let duration_ms = recording.duration_ms;
    let rms = recording.rms;
    let wav_data = recording.wav_data;

    let config = app.state::<AppState>().config.lock().unwrap().clone();
    if config.api_key.is_empty() {
        emit_pipeline_error(&app, "No Groq API key configured. Open Settings from the tray.");
        return;
    }

    let transcription_result = if transcription::is_translate_language(&config.language) {
        transcription::translate(wav_data, &config.api_key).await
    } else {
        transcription::transcribe(wav_data, &config.language, &config.api_key).await
    };

    match transcription_result {
        Ok(text) => {
            let text = format_transcript(&text);
            if transcription::should_insert_transcript(&text, duration_ms, rms) {
                hide_overlay(&app);

                let text_to_insert = text;
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                if let Err(error) = app.run_on_main_thread(move || {
                    let result = clipboard::inject_text(&text_to_insert);
                    let _ = tx.send(result);
                }) {
                    emit_pipeline_error(&app, &format!("Failed to schedule paste: {error}"));
                } else if let Ok(result) = rx.recv() {
                    if let Err(error) = result {
                        emit_pipeline_error(&app, &format!("Text injection failed: {error}"));
                    }
                }
            } else {
                emit_pipeline_error(
                    &app,
                    "Speech was filtered. Try speaking a full sentence for at least half a second.",
                );
            }
        }
        Err(error) => emit_pipeline_error(&app, &format!("Transcription failed: {error}")),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (action_tx, action_rx) = std::sync::mpsc::channel::<HotkeyAction>();

    tauri::Builder::default()
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Vibe Voice Tool")
                .build(),
        )
        .manage(AppState {
            config: Arc::new(Mutex::new(load_config())),
            audio: AudioHandle::spawn(),
        })
        .setup(move |app| {
            let startup_config = app.state::<AppState>().config.lock().unwrap().clone();
            if let Err(error) = apply_autostart(app.handle(), startup_config.autostart) {
                eprintln!("Failed to apply autostart setting: {error}");
            }
            set_record_hotkey(&startup_config.hotkey);

            #[cfg(windows)]
            if let Some(main_win) = app.get_webview_window("main") {
                if let Ok(hwnd) = main_win.hwnd() {
                    set_window_icon_native(hwnd.0 as isize);
                }
            }

            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.set_ignore_cursor_events(true);
                position_overlay(&overlay, app.handle());
                #[cfg(windows)]
                if let Ok(raw_hwnd) = overlay.hwnd() {
                    focus::configure_overlay_no_activate(raw_hwnd.0 as isize);
                }
            }

            let settings_item = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_item, &quit_item])?;

            let tray_icon = {
                let rgba = include_bytes!("../icons/tray-128.rgba");
                tauri::image::Image::new_owned(rgba.to_vec(), 128, 128)
            };

            let tray_builder = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Vibe Voice Tool")
                .icon(tray_icon);

            tray_builder
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "settings" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            let app_handle = app.handle().clone();
            app.listen("recording-start", move |_event| {
                show_overlay(&app_handle);
            });

            start_hotkey_listener(app.handle().clone(), action_tx);

            let pipeline_app = app.handle().clone();
            std::thread::spawn(move || {
                while let Ok(action) = action_rx.recv() {
                    match action {
                        HotkeyAction::StartRecording => {
                            let state = pipeline_app.state::<AppState>();
                            if let Err(error) = state.audio.start_recording() {
                                emit_pipeline_error(
                                    &pipeline_app,
                                    &format!("Recording failed: {error}"),
                                );
                            }
                        }
                        HotkeyAction::StopRecording => {
                            let app = pipeline_app.clone();
                            tauri::async_runtime::spawn(async move {
                                process_recording(app).await;
                            });
                        }
                        HotkeyAction::ToggleLanguage => {
                            toggle_language(&pipeline_app);
                        }
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }

            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config_cmd,
            test_api_key,
            open_url
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app_handle, event| {
            if matches!(event, RunEvent::Exit) {
                stop_hotkey_listener();
            }
        });
}
