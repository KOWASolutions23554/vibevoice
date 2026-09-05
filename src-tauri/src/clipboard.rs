use crate::focus::{prepare_target_for_input, InjectMethod};
use arboard::Clipboard;
use std::mem::size_of;
use std::thread;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY, VK_CONTROL, VK_INSERT, VK_RETURN, VK_SHIFT, VK_TAB, VK_V,
};

pub fn inject_text(text: &str) -> Result<(), String> {
    thread::sleep(Duration::from_millis(120));

    let method = prepare_target_for_input()?;
    thread::sleep(Duration::from_millis(80));

    match method {
        InjectMethod::UnicodeType => type_unicode(text),
        InjectMethod::ShiftInsertPaste | InjectMethod::CtrlVPaste => {
            paste_via_clipboard(text, method)
        }
    }
}

fn paste_via_clipboard(text: &str, method: InjectMethod) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    let backup = clipboard.get_text().ok();

    clipboard.set_text(text).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(80));

    match method {
        InjectMethod::ShiftInsertPaste => send_shift_insert()?,
        InjectMethod::CtrlVPaste => send_ctrl_v()?,
        InjectMethod::UnicodeType => unreachable!(),
    }

    // The target reads the clipboard asynchronously after the paste keystroke;
    // restoring too early makes it paste the OLD clipboard content instead.
    // Restore in the background so the main thread is not blocked.
    if let Some(original) = backup {
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1500));
            if let Ok(mut clipboard) = Clipboard::new() {
                let _ = clipboard.set_text(&original);
            }
        });
    }

    Ok(())
}

fn type_unicode(text: &str) -> Result<(), String> {
    let mut inputs = Vec::with_capacity(text.len() * 2);

    for ch in text.chars() {
        match ch {
            '\n' => {
                inputs.push(key_event(VK_RETURN, Default::default()));
                inputs.push(key_event(VK_RETURN, KEYEVENTF_KEYUP));
            }
            '\t' => {
                inputs.push(key_event(VK_TAB, Default::default()));
                inputs.push(key_event(VK_TAB, KEYEVENTF_KEYUP));
            }
            _ => {
                push_unicode_char(&mut inputs, ch);
            }
        }
    }

    send_inputs_in_chunks(&inputs)
}

fn push_unicode_char(inputs: &mut Vec<INPUT>, ch: char) {
    let mut buffer = [0u16; 2];
    let encoded = ch.encode_utf16(&mut buffer);
    for unit in encoded.iter().copied() {
        inputs.push(unicode_event(unit, Default::default()));
        inputs.push(unicode_event(unit, KEYEVENTF_KEYUP));
    }
}

fn unicode_event(
    unit: u16,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: flags | KEYEVENTF_UNICODE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_ctrl_v() -> Result<(), String> {
    send_inputs(&[
        key_event(VK_CONTROL, Default::default()),
        key_event(VK_V, Default::default()),
        key_event(VK_V, KEYEVENTF_KEYUP),
        key_event(VK_CONTROL, KEYEVENTF_KEYUP),
    ])
}

fn send_shift_insert() -> Result<(), String> {
    send_inputs(&[
        key_event(VK_SHIFT, Default::default()),
        key_event(VK_INSERT, Default::default()),
        key_event(VK_INSERT, KEYEVENTF_KEYUP),
        key_event(VK_SHIFT, KEYEVENTF_KEYUP),
    ])
}

fn key_event(
    virtual_key: VIRTUAL_KEY,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_inputs_in_chunks(inputs: &[INPUT]) -> Result<(), String> {
    const CHUNK_SIZE: usize = 120;

    for chunk in inputs.chunks(CHUNK_SIZE) {
        send_inputs(chunk)?;
        thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}

fn send_inputs(inputs: &[INPUT]) -> Result<(), String> {
    unsafe {
        let sent = SendInput(inputs, size_of::<INPUT>() as i32);
        if sent as usize != inputs.len() {
            return Err("Failed to simulate keystrokes".to_string());
        }
    }

    Ok(())
}
