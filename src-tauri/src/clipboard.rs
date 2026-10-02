use crate::focus::{prepare_target_for_input, InjectMethod};
use arboard::Clipboard;
use std::mem::size_of;
use std::thread;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, VkKeyScanW, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    KEYEVENTF_UNICODE, MAPVK_VK_TO_CHAR, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_CONTROL, VK_INSERT,
    VK_LCONTROL, VK_LSHIFT, VK_RETURN, VK_RMENU, VK_SHIFT, VK_SPACE, VK_TAB, VK_V,
};

pub fn inject_text(text: &str, remote_typing: bool) -> Result<(), String> {
    thread::sleep(Duration::from_millis(120));

    let detected = prepare_target_for_input()?;
    let method = if remote_typing {
        InjectMethod::ScancodeType
    } else {
        detected
    };
    thread::sleep(Duration::from_millis(80));

    match method {
        InjectMethod::UnicodeType => type_unicode(text),
        InjectMethod::ScancodeType => type_scancodes(text),
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
        InjectMethod::UnicodeType | InjectMethod::ScancodeType => unreachable!(),
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
        if !push_control_key(&mut inputs, ch) {
            push_unicode_char(&mut inputs, ch);
        }
    }

    send_inputs_in_chunks(&inputs)
}

fn push_control_key(inputs: &mut Vec<INPUT>, ch: char) -> bool {
    let virtual_key = match ch {
        '\n' => VK_RETURN,
        '\t' => VK_TAB,
        _ => return false,
    };
    inputs.push(key_event(virtual_key, Default::default()));
    inputs.push(key_event(virtual_key, KEYEVENTF_KEYUP));
    true
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

// Remote desktop clients like Parsec forward physical key presses, but drop
// Unicode packets and do not reliably sync the clipboard. So every character
// is typed as the real key (scancode) of the current keyboard layout.
fn type_scancodes(text: &str) -> Result<(), String> {
    let mut inputs = Vec::with_capacity(text.len() * 4);
    for ch in text.chars() {
        if !push_control_key(&mut inputs, ch) {
            push_scancode_char(&mut inputs, ch);
        }
    }
    send_inputs_in_chunks(&inputs)
}

fn push_scancode_char(inputs: &mut Vec<INPUT>, ch: char) {
    let scan = u16::try_from(ch as u32).map_or(-1, |unit| unsafe { VkKeyScanW(unit) });

    // Only plain, Shift and AltGr keys are typeable; anything else (e.g. emoji)
    // falls back to a Unicode packet. AltGr goes out as Ctrl + RIGHT Alt, so the
    // Ctrl + left Alt mode toggle never fires while typing @, € or \.
    let modifiers: &[VIRTUAL_KEY] = match (scan >> 8) & 0xFF {
        0 => &[],
        1 => &[VK_LSHIFT],
        6 => &[VK_LCONTROL, VK_RMENU],
        7 => &[VK_LCONTROL, VK_RMENU, VK_LSHIFT],
        _ => return push_unicode_char(inputs, ch),
    };
    let vk = VIRTUAL_KEY((scan & 0xFF) as u16);

    for &modifier in modifiers {
        inputs.push(scancode_event(modifier, Default::default()));
    }
    push_scancode_tap(inputs, vk);
    for &modifier in modifiers.iter().rev() {
        inputs.push(scancode_event(modifier, KEYEVENTF_KEYUP));
    }

    // Dead keys (^ ` ´ on German layouts) only print after a following space.
    if unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_CHAR) } & 0x8000_0000 != 0 {
        push_scancode_tap(inputs, VK_SPACE);
    }
}

fn push_scancode_tap(inputs: &mut Vec<INPUT>, virtual_key: VIRTUAL_KEY) {
    inputs.push(scancode_event(virtual_key, Default::default()));
    inputs.push(scancode_event(virtual_key, KEYEVENTF_KEYUP));
}

fn scancode_event(virtual_key: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(virtual_key.0 as u32, MAPVK_VK_TO_VSC) } as u16;
    let extended = if virtual_key == VK_RMENU {
        KEYEVENTF_EXTENDEDKEY
    } else {
        Default::default()
    };

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan,
                dwFlags: flags | extended | KEYEVENTF_SCANCODE,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scancode_inputs(ch: char) -> Vec<INPUT> {
        let mut inputs = Vec::new();
        push_scancode_char(&mut inputs, ch);
        inputs
    }

    #[test]
    fn plain_letter_is_one_key_press() {
        assert_eq!(scancode_inputs('a').len(), 2);
    }

    #[test]
    fn capital_letter_adds_shift() {
        assert_eq!(scancode_inputs('A').len(), 4);
    }

    #[test]
    fn emoji_falls_back_to_unicode() {
        let inputs = scancode_inputs('🎤');
        assert!(inputs
            .iter()
            .all(|input| unsafe { input.Anonymous.ki.dwFlags.contains(KEYEVENTF_UNICODE) }));
    }
}
