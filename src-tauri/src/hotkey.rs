use crate::focus::capture_target_window;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const STOP_DEBOUNCE_MS: u64 = 400;

// Modifiers live in a bitmask so the record combo can be reconfigured at runtime.
const MOD_CTRL: u8 = 1;
const MOD_ALT: u8 = 2;
const MOD_SHIFT: u8 = 4;
const MOD_WIN: u8 = 8;

const DEFAULT_RECORD_MODS: u8 = MOD_CTRL | MOD_WIN;

// Ctrl + left Alt toggles the language mode. Right Alt is excluded on purpose:
// on a German keyboard AltGr sends Ctrl + right Alt, so typing @ or \ must not
// flip the mode.
const MODE_MODS: u8 = MOD_CTRL | MOD_ALT;

static MOD_STATE: AtomicU8 = AtomicU8::new(0);
static RECORD_MODS: AtomicU8 = AtomicU8::new(DEFAULT_RECORD_MODS);
static LEFT_ALT_DOWN: AtomicBool = AtomicBool::new(false);
static MODE_ARMED: AtomicBool = AtomicBool::new(false);

static RECORDING: AtomicBool = AtomicBool::new(false);
static LOCKED: AtomicBool = AtomicBool::new(false);
static LAST_COMBO_RELEASE: AtomicU64 = AtomicU64::new(0);
static STOP_GENERATION: AtomicU64 = AtomicU64::new(0);

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
static ACTION_TX: OnceLock<Sender<HotkeyAction>> = OnceLock::new();
static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);

pub enum HotkeyAction {
    StartRecording,
    StopRecording,
    ToggleLanguage,
}

#[derive(Serialize, Clone)]
struct RecordingStartPayload {
    locked: bool,
}

/// Turns a config string like "Ctrl+Win" into a modifier bitmask.
/// Unknown or empty specs fall back to the built-in default.
fn parse_modifiers(spec: &str, fallback: u8) -> u8 {
    let mut mask = 0u8;
    for part in spec.split('+') {
        mask |= match part.trim().to_lowercase().as_str() {
            "ctrl" => MOD_CTRL,
            "alt" => MOD_ALT,
            "shift" => MOD_SHIFT,
            "win" => MOD_WIN,
            _ => 0,
        };
    }

    if mask == 0 {
        fallback
    } else {
        mask
    }
}

pub fn set_record_hotkey(record_combo: &str) {
    RECORD_MODS.store(
        parse_modifiers(record_combo, DEFAULT_RECORD_MODS),
        Ordering::SeqCst,
    );
}

pub fn is_recording() -> bool {
    RECORDING.load(Ordering::SeqCst)
}

pub fn start_hotkey_listener(app: AppHandle, tx: Sender<HotkeyAction>) {
    let _ = APP_HANDLE.set(app);

    if ACTION_TX.set(tx).is_err() {
        return;
    }

    thread::spawn(|| unsafe {
        let hook =
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), HINSTANCE::default(), 0) {
                Ok(hook) => hook,
                Err(error) => {
                    eprintln!("Failed to install keyboard hook: {error}");
                    return;
                }
            };

        HOOK_HANDLE.store(hook.0 as isize, Ordering::SeqCst);

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        let _ = UnhookWindowsHookEx(hook);
    });
}

pub fn stop_hotkey_listener() {
    let raw = HOOK_HANDLE.load(Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(raw as *mut _));
        }
        HOOK_HANDLE.store(0, Ordering::SeqCst);
    }
}

fn modifier_bit(virtual_key: u32) -> u8 {
    if virtual_key == VK_LCONTROL.0 as u32 || virtual_key == VK_RCONTROL.0 as u32 {
        MOD_CTRL
    } else if virtual_key == VK_LMENU.0 as u32 || virtual_key == VK_RMENU.0 as u32 {
        MOD_ALT
    } else if virtual_key == VK_LSHIFT.0 as u32 || virtual_key == VK_RSHIFT.0 as u32 {
        MOD_SHIFT
    } else if virtual_key == VK_LWIN.0 as u32 || virtual_key == VK_RWIN.0 as u32 {
        MOD_WIN
    } else {
        0
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let keyboard = *(lparam.0 as *const KBDLLHOOKSTRUCT);
    let virtual_key = keyboard.vkCode;
    let is_key_down = wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize;
    let is_key_up = wparam.0 == WM_KEYUP as usize || wparam.0 == WM_SYSKEYUP as usize;

    let bit = modifier_bit(virtual_key);
    if bit != 0 {
        if virtual_key == VK_LMENU.0 as u32 {
            LEFT_ALT_DOWN.store(is_key_down, Ordering::SeqCst);
        }

        if is_key_down {
            MOD_STATE.fetch_or(bit, Ordering::SeqCst);
            arm_mode_toggle();
            try_start_recording();
        } else if is_key_up {
            MOD_STATE.fetch_and(!bit, Ordering::SeqCst);
            if bit == MOD_CTRL || bit == MOD_ALT {
                fire_mode_toggle();
            }
            try_stop_recording();
        }

        // Swallow the Windows key while the rest of the record combo is held,
        // otherwise Windows opens the Start menu on release.
        if bit == MOD_WIN {
            let others = RECORD_MODS.load(Ordering::SeqCst) & !MOD_WIN;
            if others != 0 && MOD_STATE.load(Ordering::SeqCst) & others == others {
                return LRESULT(1);
            }
        }
    } else if is_key_down && MODE_ARMED.load(Ordering::SeqCst) {
        // Ctrl+Alt+<something> is somebody else's shortcut, not a mode switch.
        MODE_ARMED.store(false, Ordering::SeqCst);
    }

    CallNextHookEx(None, code, wparam, lparam)
}

// The toggle only arms while exactly Ctrl and the left Alt are held, and it
// only fires if they are released again without any other key in between.
fn arm_mode_toggle() {
    let armed =
        MOD_STATE.load(Ordering::SeqCst) == MODE_MODS && LEFT_ALT_DOWN.load(Ordering::SeqCst);
    MODE_ARMED.store(armed, Ordering::SeqCst);
}

fn fire_mode_toggle() {
    if !MODE_ARMED.swap(false, Ordering::SeqCst) {
        return;
    }

    if let Some(tx) = ACTION_TX.get() {
        let _ = tx.send(HotkeyAction::ToggleLanguage);
    }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn cancel_pending_stop() {
    STOP_GENERATION.fetch_add(1, Ordering::SeqCst);
}

fn schedule_stop() {
    let generation = STOP_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(STOP_DEBOUNCE_MS));
        if STOP_GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        if LOCKED.load(Ordering::SeqCst) || !RECORDING.load(Ordering::SeqCst) {
            return;
        }
        finish_recording();
    });
}

fn emit_recording_start(locked: bool) {
    if let Some(app) = APP_HANDLE.get() {
        let payload = RecordingStartPayload { locked };
        let _ = app.emit("recording-start", payload);
    }
}

fn emit_recording_locked() {
    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit("recording-locked", ());
    }
}

fn begin_recording(locked: bool) {
    if RECORDING.swap(true, Ordering::SeqCst) {
        return;
    }

    capture_target_window();
    emit_recording_start(locked);

    if let Some(tx) = ACTION_TX.get() {
        let _ = tx.send(HotkeyAction::StartRecording);
    }
}

fn finish_recording() {
    if !RECORDING.swap(false, Ordering::SeqCst) {
        return;
    }

    LOCKED.store(false, Ordering::SeqCst);
    capture_target_window();

    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit("recording-stop", ());
        let _ = app.emit("transcribing", ());
    }

    if let Some(tx) = ACTION_TX.get() {
        let _ = tx.send(HotkeyAction::StopRecording);
    }
}

fn record_combo_held() -> bool {
    let required = RECORD_MODS.load(Ordering::SeqCst);
    MOD_STATE.load(Ordering::SeqCst) & required == required
}

fn try_start_recording() {
    if !record_combo_held() {
        return;
    }

    cancel_pending_stop();

    if LOCKED.load(Ordering::SeqCst) {
        finish_recording();
        return;
    }

    let last_release = LAST_COMBO_RELEASE.load(Ordering::SeqCst);
    let now = now_ms();
    if last_release > 0 && now.saturating_sub(last_release) < STOP_DEBOUNCE_MS {
        LOCKED.store(true, Ordering::SeqCst);
        if RECORDING.load(Ordering::SeqCst) {
            emit_recording_locked();
        } else {
            begin_recording(true);
        }
        return;
    }

    begin_recording(false);
}

fn try_stop_recording() {
    if !RECORDING.load(Ordering::SeqCst) {
        return;
    }

    if record_combo_held() {
        return;
    }

    if LOCKED.load(Ordering::SeqCst) {
        return;
    }

    LAST_COMBO_RELEASE.store(now_ms(), Ordering::SeqCst);
    schedule_stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifier_combos() {
        assert_eq!(parse_modifiers("Ctrl+Win", 0), MOD_CTRL | MOD_WIN);
        assert_eq!(parse_modifiers("alt + shift", 0), MOD_ALT | MOD_SHIFT);
        assert_eq!(parse_modifiers("Ctrl", 0), MOD_CTRL);
    }

    #[test]
    fn falls_back_on_unknown_combos() {
        assert_eq!(
            parse_modifiers("", DEFAULT_RECORD_MODS),
            DEFAULT_RECORD_MODS
        );
        assert_eq!(
            parse_modifiers("Banana", DEFAULT_RECORD_MODS),
            DEFAULT_RECORD_MODS
        );
    }
}
