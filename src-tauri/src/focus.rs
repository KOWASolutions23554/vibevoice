use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::RECT;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsWindow, SetForegroundWindow, ShowWindow, SW_SHOW,
};

static TARGET_HWND: AtomicIsize = AtomicIsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectMethod {
    /// Classic console windows (conhost)
    ShiftInsertPaste,
    /// Browsers, most editors
    CtrlVPaste,
    /// Direct keystrokes — works reliably in Cursor / VS Code
    UnicodeType,
}

pub fn capture_target_window() {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return;
        }
        TARGET_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    }
}

pub fn inject_method_for_target() -> InjectMethod {
    let Some(hwnd) = target_hwnd() else {
        return InjectMethod::CtrlVPaste;
    };

    let class = window_class_name(hwnd);
    let title = window_title(hwnd).to_lowercase();

    // Windows Terminal pastes via its Ctrl+V keybinding; Shift+Insert is not
    // bound in every setup (verified missing on this machine).
    if class == "CASCADIA_HOSTING_WINDOW_CLASS" {
        return InjectMethod::CtrlVPaste;
    }

    if class == "ConsoleWindowClass" {
        return InjectMethod::ShiftInsertPaste;
    }

    if class == "Chrome_WidgetWin_1" {
        if title.contains("cursor")
            || title.contains("visual studio code")
            || title.contains("code")
        {
            return InjectMethod::UnicodeType;
        }
        return InjectMethod::CtrlVPaste;
    }

    InjectMethod::CtrlVPaste
}

pub fn prepare_target_for_input() -> Result<InjectMethod, String> {
    let method = inject_method_for_target();

    let Some(target) = target_hwnd() else {
        return Ok(method);
    };

    unsafe {
        if !IsWindow(target).as_bool() {
            return Ok(method);
        }

        let foreground = GetForegroundWindow();
        if foreground == target {
            return Ok(method);
        }

        if is_same_process(foreground, target) {
            return Ok(method);
        }

        let _ = ShowWindow(target, SW_SHOW);
        if !set_foreground_window(target) {
            return Err("Could not restore keyboard focus".to_string());
        }
    }

    Ok(method)
}

pub fn target_monitor_center() -> Option<(i32, i32)> {
    let target = target_hwnd()?;

    unsafe {
        if !IsWindow(target).as_bool() {
            return None;
        }

        let mut rect = RECT::default();
        if GetWindowRect(target, &mut rect).is_err() {
            return None;
        }

        Some(((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2))
    }
}
fn target_hwnd() -> Option<HWND> {
    let raw = TARGET_HWND.load(Ordering::SeqCst);
    if raw == 0 {
        return None;
    }
    Some(HWND(raw as *mut _))
}

fn window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    unsafe {
        let length = GetClassNameW(hwnd, &mut buffer);
        if length == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..length as usize])
    }
}

fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    unsafe {
        let length = GetWindowTextW(hwnd, &mut buffer);
        if length == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..length as usize])
    }
}

unsafe fn is_same_process(a: HWND, b: HWND) -> bool {
    let mut pid_a = 0u32;
    let mut pid_b = 0u32;
    GetWindowThreadProcessId(a, Some(&mut pid_a));
    GetWindowThreadProcessId(b, Some(&mut pid_b));
    pid_a != 0 && pid_a == pid_b
}

unsafe fn set_foreground_window(hwnd: HWND) -> bool {
    let foreground = GetForegroundWindow();
    let current_thread = GetCurrentThreadId();
    let foreground_thread = GetWindowThreadProcessId(foreground, None);
    let target_thread = GetWindowThreadProcessId(hwnd, None);

    let attached_foreground = foreground_thread != current_thread
        && AttachThreadInput(foreground_thread, current_thread, true).as_bool();
    let attached_target = target_thread != current_thread
        && AttachThreadInput(target_thread, current_thread, true).as_bool();

    let result = SetForegroundWindow(hwnd).as_bool();

    if attached_target {
        let _ = AttachThreadInput(target_thread, current_thread, false);
    }
    if attached_foreground {
        let _ = AttachThreadInput(foreground_thread, current_thread, false);
    }

    result
}

#[cfg(windows)]
pub fn configure_overlay_no_activate(raw_hwnd: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE,
    };

    unsafe {
        let hwnd = HWND(raw_hwnd as *mut _);
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (style | WS_EX_NOACTIVATE.0) as isize);
    }
}
