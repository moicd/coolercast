//! Tray icon, single-instance handling and the message loop.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, mem, ptr};

use crate::{autostart, icon, window};
use coolercast_core::config::{Mode, Unit};
use coolercast_core::ipc::{self, Status};
use coolercast_core::win::wide;
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW,
    GetCursorPos, GetMessageW, HICON, HMENU, KillTimer, MF_CHECKED, MF_GRAYED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem, SetTimer, TPM_BOTTOMALIGN,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP,
    WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

const CLASS: &str = "coolercast-app";
const WM_TRAY: u32 = WM_APP + 1;
/// Posted by a second instance: open the settings window.
const WM_OPEN_SETTINGS: u32 = WM_APP + 2;
const TIMER_ID: usize = 1;
const REFRESH_MS: u32 = 2000;
/// Faster polling while the settings window shows live values.
const REFRESH_MS_WINDOW: u32 = 1000;

const ID_SETTINGS: usize = 90;
const ID_MODE_TEMPERATURE: usize = 100;
const ID_MODE_USAGE: usize = 101;
const ID_MODE_AUTO: usize = 102;
const ID_MODE_CUSTOM: usize = 103;
const ID_MODE_POWER: usize = 104;
const ID_UNIT_CELSIUS: usize = 110;
const ID_UNIT_FAHRENHEIT: usize = 111;
const ID_ALARM: usize = 120;
const ID_AUTOSTART: usize = 130;
const ID_EXIT: usize = 199;

struct App {
    hwnd: HWND,
    /// `None` while the service is not reachable.
    status: Option<Status>,
    icon: HICON,
    icon_key: (String, icon::Rgb),
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Broadcast by Explorer when the taskbar is recreated; the icon must be added again.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

pub fn main() {
    let tray_only = env::args().skip(1).any(|arg| arg == "--tray");

    let mutex_name = wide(r"Local\coolercast-app");
    let _single_instance = unsafe { CreateMutexW(ptr::null(), 0, mutex_name.as_ptr()) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if !tray_only {
            show_running_instance();
        }
        return;
    }
    autostart::update();

    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            Ordering::Relaxed,
        );

        let instance = GetModuleHandleW(ptr::null());
        let class = wide(CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..mem::zeroed()
        };
        RegisterClassW(&wc);
        // A hidden top-level window: message-only windows miss the TaskbarCreated broadcast.
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }

        APP.with(|app| {
            *app.borrow_mut() = Some(App {
                hwnd,
                status: None,
                icon: ptr::null_mut(),
                icon_key: (String::new(), icon::OFFLINE),
            })
        });
        refresh(true);
        if !tray_only {
            window::open();
        }

        let mut msg: MSG = mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => refresh(false),
        WM_TRAY if lparam as u32 == WM_LBUTTONUP => window::open(),
        WM_TRAY if lparam as u32 == WM_RBUTTONUP => show_menu(hwnd),
        WM_OPEN_SETTINGS => window::open(),
        WM_DESTROY => {
            window::close();
            APP.with(|app| {
                if let Some(app) = app.borrow_mut().take() {
                    unsafe {
                        KillTimer(app.hwnd, TIMER_ID);
                        Shell_NotifyIconW(NIM_DELETE, &notify_data(&app));
                        DestroyIcon(app.icon);
                    }
                }
            });
            unsafe { PostQuitMessage(0) };
        }
        m if m == TASKBAR_CREATED.load(Ordering::Relaxed) && m != 0 => refresh(true),
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    0
}

fn notify_data(app: &App) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = app.hwnd;
    data.uID = 1;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = app.icon;
    let tip = tooltip(app.status.as_ref());
    for (dst, src) in data.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
        *dst = src;
    }
    data
}

/// Asks the running instance to show its settings window.
fn show_running_instance() {
    let class = wide(CLASS);
    let hwnd = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
    if !hwnd.is_null() {
        unsafe {
            // This process was just started by the user, so it may hand over the foreground.
            AllowSetForegroundWindow(ASFW_ANY);
            PostMessageW(hwnd, WM_OPEN_SETTINGS, 0, 0);
        }
    }
}

/// The last status received from the service.
pub fn current_status() -> Option<Status> {
    APP.with(|app| app.borrow().as_ref().and_then(|a| a.status.clone()))
}

/// Polls the service and updates the icon and the settings window. `add` (re)creates the tray
/// entry.
pub fn refresh(add: bool) {
    let status = ipc::query_status().ok();
    let window_open = window::is_open();
    APP.with(|app| {
        let mut app = app.borrow_mut();
        let Some(app) = app.as_mut() else { return };
        app.status = status.clone();

        let key = icon_content(app.status.as_ref());
        if key != app.icon_key || app.icon.is_null() {
            let new_icon = icon::render(&key.0, key.1);
            if !app.icon.is_null() {
                unsafe { DestroyIcon(app.icon) };
            }
            app.icon = new_icon;
            app.icon_key = key;
        }
        let data = notify_data(app);
        let interval = if window_open {
            REFRESH_MS_WINDOW
        } else {
            REFRESH_MS
        };
        unsafe {
            Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &data);
            // Also restarts the timer, so the next poll is a full interval away.
            SetTimer(app.hwnd, TIMER_ID, interval, None);
        }
    });
    window::update(status.as_ref());
}

fn icon_content(status: Option<&Status>) -> (String, icon::Rgb) {
    let Some(s) = status else {
        return ("-".into(), icon::OFFLINE);
    };
    let config = &s.config;
    match s.cpu_temp {
        Some(t) => {
            let alarm = config.alarm && t >= f32::from(config.alarm_threshold);
            let color = if alarm { icon::ALARM } else { icon::NORMAL };
            (format!("{:.0}", config.unit.from_celsius(t)), color)
        }
        None => (format!("{:.0}", s.cpu_usage.unwrap_or(0.0)), icon::NORMAL),
    }
}

fn tooltip(status: Option<&Status>) -> String {
    let Some(s) = status else {
        return "CoolerCast\nService not running".into();
    };
    let unit = s.config.unit;
    let temp = s.cpu_temp.map_or("--".into(), |t| {
        format!("{:.0} {}", unit.from_celsius(t), unit.symbol())
    });
    let usage = s.cpu_usage.map_or("--".into(), |u| format!("{u:.0} %"));
    let devices = if s.devices.is_empty() {
        "No cooler connected".into()
    } else {
        s.devices.join(", ")
    };
    format!("CoolerCast\nCPU {temp} · {usage}\n{devices}")
}

fn show_menu(hwnd: HWND) {
    // Copy what the menu needs: TrackPopupMenu runs a modal loop that re-enters window_proc.
    let status = APP.with(|app| app.borrow().as_ref().and_then(|a| a.status.clone()));
    let autostart = autostart::enabled();

    let command = unsafe {
        let menu = CreatePopupMenu();
        let item = |menu: HMENU, id: usize, text: &str, checked: bool, enabled: bool| {
            let mut flags = MF_STRING;
            if checked {
                flags |= MF_CHECKED;
            }
            if !enabled {
                flags |= MF_GRAYED;
            }
            AppendMenuW(menu, flags, id, wide(text).as_ptr());
        };

        let header = match &status {
            Some(s) => tooltip(Some(s))
                .lines()
                .nth(1)
                .unwrap_or_default()
                .to_owned(),
            None => "Service not running".into(),
        };
        item(menu, 0, &header, false, false);
        if let Some(error) = status.as_ref().and_then(|s| s.temp_error.as_ref()) {
            item(
                menu,
                0,
                &format!("Temperature unavailable: {error}"),
                false,
                false,
            );
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        item(menu, ID_SETTINGS, "Settings", false, true);
        SetMenuDefaultItem(menu, ID_SETTINGS as u32, 0);
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());

        let online = status.is_some();
        let config = status
            .as_ref()
            .map(|s| s.config.clone())
            .unwrap_or_default();

        let modes = CreatePopupMenu();
        item(
            modes,
            ID_MODE_TEMPERATURE,
            "Temperature",
            online && config.mode == Mode::Temperature,
            online,
        );
        item(
            modes,
            ID_MODE_USAGE,
            "Usage",
            online && config.mode == Mode::Usage,
            online,
        );
        item(
            modes,
            ID_MODE_AUTO,
            "Alternate",
            online && config.mode == Mode::Auto,
            online,
        );
        item(
            modes,
            ID_MODE_POWER,
            "Power (LS series)",
            online && config.mode == Mode::Power,
            online,
        );
        item(
            modes,
            ID_MODE_CUSTOM,
            "Custom value",
            online && config.mode == Mode::Custom,
            online,
        );
        AppendMenuW(menu, MF_POPUP, modes as usize, wide("Display").as_ptr());

        let units = CreatePopupMenu();
        item(
            units,
            ID_UNIT_CELSIUS,
            "Celsius (°C)",
            online && config.unit == Unit::Celsius,
            online,
        );
        item(
            units,
            ID_UNIT_FAHRENHEIT,
            "Fahrenheit (°F)",
            online && config.unit == Unit::Fahrenheit,
            online,
        );
        AppendMenuW(menu, MF_POPUP, units as usize, wide("Unit").as_ptr());

        let alarm_label = format!("Alarm at {} °C", config.alarm_threshold);
        item(menu, ID_ALARM, &alarm_label, online && config.alarm, online);
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        item(menu, ID_AUTOSTART, "Start with Windows", autostart, true);
        item(menu, ID_EXIT, "Exit", false, true);

        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            cursor.x,
            cursor.y,
            0,
            hwnd,
            ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu); // also destroys the submenus
        command as usize
    };

    let alarm = status.as_ref().is_some_and(|s| s.config.alarm);
    let setting = match command {
        ID_MODE_TEMPERATURE => Some(("mode", "temperature")),
        ID_MODE_USAGE => Some(("mode", "usage")),
        ID_MODE_AUTO => Some(("mode", "auto")),
        ID_MODE_CUSTOM => Some(("mode", "custom")),
        ID_MODE_POWER => Some(("mode", "power")),
        ID_UNIT_CELSIUS => Some(("unit", "celsius")),
        ID_UNIT_FAHRENHEIT => Some(("unit", "fahrenheit")),
        ID_ALARM => Some(("alarm", if alarm { "off" } else { "on" })),
        ID_SETTINGS => {
            window::open();
            None
        }
        ID_AUTOSTART => {
            autostart::set(!autostart);
            None
        }
        ID_EXIT => {
            unsafe { DestroyWindow(hwnd) };
            return;
        }
        _ => None,
    };
    if let Some((key, value)) = setting {
        let _ = ipc::set(key, value);
        refresh(false);
    }
}
