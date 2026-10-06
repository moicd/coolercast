//! Settings window, custom drawn with GDI+ in the style of the Windows 11 settings: a navigation
//! sidebar, an overview page with a live preview of the cooler display and the recent history,
//! and pages of setting cards.
//!
//! On Windows 11 (22H2 and later) the window uses the Acrylic system backdrop and draws its
//! cards as translucent glass on top; elsewhere it falls back to opaque colors. The blur is
//! composed by Windows, so it costs this process nothing.
//!
//! The window only exists while it is open; closing it releases GDI+ and every drawing resource,
//! so the tray icon alone stays as small as before.

use std::cell::{Cell, RefCell};
use std::sync::Mutex;
use std::{mem, ptr, thread};

use coolercast_core::config::{self, Bar, ClockTime, Config, Mode, Source, Symbol, Unit};
use coolercast_core::device::Component;
use coolercast_core::ipc::{DisplayOff, HISTORY_LEN, Shown, Status};
use coolercast_core::win::wide;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMSBT_TRANSIENTWINDOW, DWMWA_CAPTION_COLOR, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint,
    GetMonitorInfoW, HBITMAP, HDC, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, PAINTSTRUCT, SRCCOPY, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DOWN,
    VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos, IDC_ARROW,
    IsIconic, KillTimer, LoadCursorW, LoadIconW, PostMessageW, RegisterClassW, SW_RESTORE,
    SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetTimer, SetWindowPos,
    ShowWindow, WM_APP, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SETTINGCHANGE,
    WM_TIMER, WNDCLASSW, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
};

use crate::autostart;
use crate::gfx::{Align, Canvas, Color, Gdiplus, Rect, Weight};
use crate::preview::{self, Frame};
use crate::theme::{self, Theme};
use crate::update::{self, Check};

/// Posted by the update check thread when it has an answer.
const WM_UPDATE_CHECKED: u32 = WM_APP + 10;
/// `MK_SHIFT` in the `wParam` of mouse messages.
const MK_SHIFT: usize = 0x0004;

/// The answer of the last update check, handed from its thread to the window.
static UPDATE_RESULT: Mutex<Option<Check>> = Mutex::new(None);

const CLASS: &str = "coolercast-settings";
const STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

// Layout, in device-independent pixels: a navigation sidebar and the page on its right.
const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 620.0;
const SIDEBAR_W: f32 = 224.0;
const PAGE_X: f32 = SIDEBAR_W + 12.0;
const PAGE_W: f32 = WIDTH - PAGE_X - 28.0;
const PAGE_TOP: f32 = 84.0;
const NAV_TOP: f32 = 116.0;
const NAV_H: f32 = 40.0;
const SETTING_H: f32 = 64.0;
const SETTING_GAP: f32 = 6.0;
const RADIUS: f32 = 8.0;

/// Stepper changes are sent once the value stops changing for this long.
const TIMER_COMMIT: usize = 1;
const COMMIT_DELAY_MS: u32 = 400;
/// Holding a stepper button repeats it.
const TIMER_REPEAT: usize = 2;
const REPEAT_DELAY_MS: u32 = 400;
const REPEAT_RATE_MS: u32 = 60;
/// Backspace in the custom number field.
const BACKSPACE: char = '\u{8}';

/// The pages of the window, in the order of the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Overview,
    Display,
    Custom,
    TurnOff,
    Alarm,
    General,
}

const PAGES: [Page; 6] = [
    Page::Overview,
    Page::Display,
    Page::Custom,
    Page::TurnOff,
    Page::Alarm,
    Page::General,
];

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Display => "Display",
            Page::Custom => "Custom value",
            Page::TurnOff => "Display off",
            Page::Alarm => "Alarm",
            Page::General => "General",
        }
    }

    /// Glyph of the Windows icon font.
    fn icon(self) -> &'static str {
        match self {
            Page::Overview => "\u{E80F}",
            Page::Display => "\u{E7F4}",
            Page::Custom => "\u{E70F}",
            Page::TurnOff => "\u{E708}",
            Page::Alarm => "\u{EA8F}",
            Page::General => "\u{E713}",
        }
    }

    /// One line under the page title.
    fn subtitle(self) -> &'static str {
        match self {
            Page::Overview => "What the cooler shows right now",
            Page::Display => "What the display shows and how",
            Page::Custom => "A fixed number, shown when Show is set to Custom",
            Page::TurnOff => "Let the display go dark when nobody is looking",
            Page::Alarm => "A warning when the processor gets too hot",
            Page::General => "Startup and updates",
        }
    }

    /// The setting cards of the page, in focus order: control, title and description.
    fn settings(self) -> &'static [(Control, &'static str, &'static str)] {
        match self {
            Page::Overview => &[],
            Page::Display => &[
                (Control::Mode, "Show", "The value on the display"),
                (
                    Control::Source,
                    "Device",
                    "Processor, graphics card, both in turn, or the GPU while it is busy",
                ),
                (
                    Control::AutoInterval,
                    "Switch every",
                    "How long each value stays when alternating",
                ),
                (Control::Unit, "Unit", "Temperature unit"),
                (
                    Control::Bar,
                    "Bar shows the usage",
                    "Temperature on the digits and usage on the bar, at the same time",
                ),
                (
                    Control::Interval,
                    "Refresh every",
                    "How often the display is updated",
                ),
            ],
            Page::Custom => &[
                (
                    Control::UseCustom,
                    "Not shown right now",
                    "The display shows this value when Show is set to Custom",
                ),
                (
                    Control::CustomValue,
                    "Number",
                    "From 0 to 999: type it or use the buttons",
                ),
                (Control::CustomSymbol, "Symbol", "Lit next to the number"),
                (Control::CustomBar, "Bar", "Lit steps of the bar"),
            ],
            Page::TurnOff => &[
                (
                    Control::OffLocked,
                    "When the PC is locked",
                    "While Windows shows the lock screen",
                ),
                (
                    Control::OffScreen,
                    "When the screen turns off",
                    "Follows the monitors going to sleep",
                ),
                (
                    Control::OffNight,
                    "At night",
                    "Every day, between two times",
                ),
                (Control::NightStart, "From", "Local time"),
                (Control::NightEnd, "Until", "Local time"),
            ],
            Page::Alarm => &[
                (
                    Control::Alarm,
                    "Blink when hot",
                    "The display blinks when the processor reaches the threshold",
                ),
                (
                    Control::Threshold,
                    "Threshold",
                    "Processor temperature that sets off the alarm",
                ),
            ],
            Page::General => &[
                (
                    Control::Autostart,
                    "Start with Windows",
                    "Keeps the CoolerCast icon in the notification area",
                ),
                (
                    Control::Update,
                    concat!("Version ", env!("CARGO_PKG_VERSION")),
                    "Asks GitHub for the latest version, only when you click",
                ),
            ],
        }
    }
}

/// The interactive controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Nav(Page),
    Autostart,
    Update,
    /// Switches the display to the custom value.
    UseCustom,
    Mode,
    Source,
    AutoInterval,
    Unit,
    Bar,
    Interval,
    CustomValue,
    CustomSymbol,
    CustomBar,
    OffLocked,
    OffScreen,
    OffNight,
    NightStart,
    NightEnd,
    Alarm,
    Threshold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Segment(usize),
    Minus,
    Plus,
    Switch,
    /// The number of a stepper that also takes typed digits.
    Field,
    Button,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hit {
    control: Control,
    part: Part,
}

/// Keyboard focus order of a page: the sidebar, then the settings shown on the page.
fn focus_order(page: Page, config: &Config) -> Vec<Control> {
    PAGES
        .iter()
        .map(|&p| Control::Nav(p))
        .chain(
            page.settings()
                .iter()
                .map(|&(c, ..)| c)
                .filter(|c| c.visible(config)),
        )
        .collect()
}

impl Control {
    /// Whether the control can be used: the service settings need the service.
    fn enabled(self, _config: &Config, online: bool) -> bool {
        matches!(self, Control::Nav(_) | Control::Autostart | Control::Update) || online
    }

    /// Whether the card is shown: settings that only matter with another one are hidden
    /// instead of greyed out.
    fn visible(self, config: &Config) -> bool {
        match self {
            Control::AutoInterval => config.mode == Mode::Auto || config.source == Source::Auto,
            Control::NightStart | Control::NightEnd => config.off_at_night,
            Control::Threshold => config.alarm,
            Control::UseCustom => config.mode != Mode::Custom,
            _ => true,
        }
    }

    fn segments(self) -> &'static [&'static str] {
        match self {
            Control::Mode => &["Temperature", "Usage", "Alternate", "Custom"],
            Control::Source => &["CPU", "GPU", "Alternate", "Smart"],
            Control::Unit => &["°C", "°F"],
            Control::CustomSymbol => &["°C", "°F", "%"],
            _ => &[],
        }
    }

    fn selected_segment(self, config: &Config) -> usize {
        match self {
            Control::Mode => match config.mode {
                Mode::Temperature => 0,
                Mode::Usage => 1,
                Mode::Auto => 2,
                Mode::Custom => 3,
                // No segment (it would not fit): the power mode, for LS displays, is chosen
                // from the tray menu or the CLI.
                Mode::Power => 4,
            },
            Control::Source => match config.source {
                Source::Cpu => 0,
                Source::Gpu => 1,
                Source::Auto => 2,
                Source::Smart => 3,
            },
            Control::CustomSymbol => match config.custom_symbol {
                Symbol::Celsius => 0,
                Symbol::Fahrenheit => 1,
                Symbol::Percent => 2,
            },
            Control::Unit => match config.unit {
                Unit::Celsius => 0,
                Unit::Fahrenheit => 1,
            },
            _ => 0,
        }
    }

    fn with_segment(self, config: &Config, index: usize) -> Config {
        let mut config = config.clone();
        match self {
            Control::Mode => {
                const MODES: [Mode; 4] = [Mode::Temperature, Mode::Usage, Mode::Auto, Mode::Custom];
                config.mode = MODES[index.min(3)];
            }
            Control::Source => {
                const SOURCES: [Source; 4] =
                    [Source::Cpu, Source::Gpu, Source::Auto, Source::Smart];
                config.source = SOURCES[index.min(3)];
            }
            Control::Unit => config.unit = [Unit::Celsius, Unit::Fahrenheit][index.min(1)],
            Control::CustomSymbol => {
                const SYMBOLS: [Symbol; 3] = [Symbol::Celsius, Symbol::Fahrenheit, Symbol::Percent];
                config.custom_symbol = SYMBOLS[index.min(2)];
            }
            _ => {}
        }
        config
    }

    /// Whether a switch control is on.
    fn is_on(self, config: &Config) -> bool {
        match self {
            Control::Bar => config.bar == Bar::Usage,
            Control::OffLocked => config.off_when_locked,
            Control::OffScreen => config.off_when_screen_off,
            Control::OffNight => config.off_at_night,
            Control::Alarm => config.alarm,
            _ => false,
        }
    }

    /// The config after flipping a switch control.
    fn toggled(self, config: &Config) -> Option<Config> {
        let mut next = config.clone();
        match self {
            Control::Bar => {
                next.bar = if config.bar == Bar::Usage {
                    Bar::Value
                } else {
                    Bar::Usage
                }
            }
            Control::OffLocked => next.off_when_locked = !config.off_when_locked,
            Control::OffScreen => next.off_when_screen_off = !config.off_when_screen_off,
            Control::OffNight => next.off_at_night = !config.off_at_night,
            Control::Alarm => next.alarm = !config.alarm,
            _ => return None,
        }
        Some(next)
    }

    /// The config after one stepper click, or `None` at the end of the range.
    fn stepped(self, config: &Config, up: bool) -> Option<Config> {
        let mut next = config.clone();
        match self {
            Control::AutoInterval => {
                const STEPS: &[(u32, u32)] = &[(10, 1), (60, 5), (600, 30), (u32::MAX, 60)];
                next.auto_interval_s =
                    step(config.auto_interval_s, up, STEPS, config::AUTO_INTERVAL_S);
            }
            Control::Interval => {
                const STEPS: &[(u32, u32)] = &[(2000, 250), (u32::MAX, 1000)];
                next.interval_ms = step(config.interval_ms, up, STEPS, config::INTERVAL_MS);
            }
            Control::Threshold => {
                let (min, max) = config::ALARM_THRESHOLD;
                let value = step(
                    config.alarm_threshold.into(),
                    up,
                    &[(u32::MAX, 1)],
                    (min.into(), max.into()),
                );
                next.alarm_threshold = u8::try_from(value).unwrap_or(max);
            }
            Control::CustomValue => {
                let (min, max) = config::CUSTOM_VALUE;
                let value = step(
                    config.custom_value.into(),
                    up,
                    &[(u32::MAX, 1)],
                    (min.into(), max.into()),
                );
                next.custom_value = u16::try_from(value).unwrap_or(max);
            }
            Control::CustomBar => {
                let (min, max) = config::CUSTOM_BAR;
                let value = step(
                    config.custom_bar.into(),
                    up,
                    &[(u32::MAX, 1)],
                    (min.into(), max.into()),
                );
                next.custom_bar = u8::try_from(value).unwrap_or(max);
            }
            Control::NightStart => next.night_start = step_time(config.night_start, up),
            Control::NightEnd => next.night_end = step_time(config.night_end, up),
            _ => return None,
        }
        (next != *config).then_some(next)
    }

    /// The setting this control changes, in the text form accepted by the service.
    fn setting(self, config: &Config) -> Option<(&'static str, String)> {
        Some(match self {
            Control::Mode => ("mode", config.mode.to_string()),
            Control::Source => ("source", config.source.to_string()),
            Control::AutoInterval => ("auto_interval_s", config.auto_interval_s.to_string()),
            Control::Unit => ("unit", config.unit.to_string()),
            Control::Bar => ("bar", config.bar.to_string()),
            Control::Interval => ("interval_ms", config.interval_ms.to_string()),
            Control::OffLocked => ("off_when_locked", config.off_when_locked.to_string()),
            Control::OffScreen => (
                "off_when_screen_off",
                config.off_when_screen_off.to_string(),
            ),
            Control::OffNight => ("off_at_night", config.off_at_night.to_string()),
            Control::NightStart => ("night_start", config.night_start.to_string()),
            Control::NightEnd => ("night_end", config.night_end.to_string()),
            Control::Alarm => ("alarm", config.alarm.to_string()),
            Control::Threshold => ("alarm_threshold", config.alarm_threshold.to_string()),
            Control::CustomValue => ("custom_value", config.custom_value.to_string()),
            Control::CustomSymbol => ("custom_symbol", config.custom_symbol.to_string()),
            Control::CustomBar => ("custom_bar", config.custom_bar.to_string()),
            Control::Nav(_) | Control::Autostart | Control::Update | Control::UseCustom => {
                return None;
            }
        })
    }
}

/// A time of day 15 minutes later or earlier, snapped to the quarter and wrapping at midnight.
fn step_time(time: ClockTime, up: bool) -> ClockTime {
    const DAY: u16 = 24 * 60;
    let m = time.minutes();
    let next = if up {
        (m / 15 + 1) * 15
    } else {
        (m + DAY - 1) % DAY / 15 * 15
    };
    ClockTime::from_minutes(next % DAY)
}

/// The custom number after typing `key` into it. `fresh` starts a new number instead of
/// appending; digits that would go past three are ignored.
fn typed(value: u16, key: char, fresh: bool) -> Option<u16> {
    match key {
        BACKSPACE => Some(if fresh { 0 } else { value / 10 }),
        '0'..='9' => {
            let digit = key as u16 - '0' as u16;
            let next = if fresh { digit } else { value * 10 + digit };
            (next <= config::CUSTOM_VALUE.1).then_some(next)
        }
        _ => None,
    }
}

/// Moves `value` one step up or down, snapping to multiples of the step. `steps` lists
/// `(below, step)` pairs: values below `below` use `step`.
fn step(value: u32, up: bool, steps: &[(u32, u32)], (min, max): (u32, u32)) -> u32 {
    let step_for = |v: u32| {
        steps
            .iter()
            .find(|&&(below, _)| v < below)
            .map_or(1, |&(_, s)| s)
    };
    let next = if up {
        let s = step_for(value);
        (value / s + 1) * s
    } else {
        let below = value.saturating_sub(1);
        let s = step_for(below);
        below / s * s
    };
    next.clamp(min, max)
}

/// Next enabled control of `order`, wrapping around.
fn next_focus(
    order: &[Control],
    current: Option<Control>,
    forward: bool,
    enabled: impl Fn(Control) -> bool,
) -> Option<Control> {
    let n = order.len();
    let start = current.and_then(|c| order.iter().position(|&x| x == c));
    (1..=n)
        .map(|offset| match (start, forward) {
            (Some(i), true) => (i + offset) % n,
            (Some(i), false) => (i + n - offset) % n,
            (None, true) => offset - 1,
            (None, false) => n - offset,
        })
        .map(|i| order[i])
        .find(|&c| enabled(c))
}

fn format_seconds(s: u32) -> String {
    match (s / 60, s % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
}

fn format_ms(ms: u32) -> String {
    let s = ms as f32 / 1000.0;
    if ms.is_multiple_of(1000) {
        format!("{} s", ms / 1000)
    } else if ms.is_multiple_of(100) {
        format!("{s:.1} s")
    } else {
        format!("{s:.2} s")
    }
}

fn format_temp(celsius: f32, unit: Unit) -> String {
    format!("{:.0} {}", unit.from_celsius(celsius), unit.symbol())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Ok,
    Warn,
    Error,
}

/// The service state in two short lines for the sidebar.
fn status_lines(status: Option<&Status>) -> (Level, String, String) {
    let Some(s) = status else {
        return (
            Level::Error,
            "Service not running".into(),
            "Reinstall CoolerCast to start it".into(),
        );
    };
    let devices = if s.devices.is_empty() {
        "No cooler connected".to_owned()
    } else {
        s.devices.join(", ")
    };
    match (&s.temp_error, s.devices.is_empty()) {
        (Some(error), _) => (
            Level::Warn,
            "Temperature unavailable".into(),
            error.split(';').next().unwrap_or(error).to_owned(),
        ),
        (None, true) => (Level::Warn, "Waiting for a cooler".into(), devices),
        (None, false) => (Level::Ok, "Running".into(), devices),
    }
}

/// The app icon, as `assets/make-icon.ps1` draws it: a teal badge with a white "°C" mark.
fn draw_app_icon(c: &Canvas, r: Rect) {
    let size = r.w;
    c.fill_round_rect_v(
        r,
        size * 0.22,
        Color::rgb(0x14, 0xB8, 0xA6),
        Color::rgb(0x0F, 0x76, 0x6E),
    );
    let white = Color::rgb(0xFF, 0xFF, 0xFF);
    let center = (r.x + size * 0.56, r.y + size * 0.55);
    c.stroke_arc(center, size * 0.25, (45.0, 270.0), size * 0.13, white);
    let ring = (r.x + size * 0.24, r.y + size * 0.27);
    c.stroke_arc(ring, size * 0.085, (0.0, 360.0), size * 0.07, white);
}

/// One line saying what the display shows right now.
fn showing(status: Option<&Status>) -> String {
    let Some(s) = status else {
        return "Not connected to the service".into();
    };
    if let Some(off) = s.display_off {
        return match off {
            DisplayOff::Locked => "Display off while the PC is locked",
            DisplayOff::ScreenOff => "Display off while the screen is off",
            DisplayOff::Night => "Display off for the night",
        }
        .into();
    }
    if s.devices.is_empty() {
        return "No cooler connected".into();
    }
    let device = if s.component == Some(Component::Gpu) {
        "GPU"
    } else {
        "CPU"
    };
    let shown = match s.shown {
        Some(Shown::Temperature) => format!("Showing the {device} temperature"),
        Some(Shown::Usage) => format!("Showing the {device} usage"),
        Some(Shown::Power) => format!("Showing the {device} power"),
        Some(Shown::Custom) => "Showing the custom value".into(),
        None => "Starting…".into(),
    };
    if s.alarm_active {
        format!("{shown} · Alarm: too hot")
    } else {
        shown
    }
}

/// What a click or key press asks for, run once the window state is no longer borrowed:
/// both talk to the service and refresh the window synchronously.
enum Effect {
    None,
    Send(Vec<(&'static str, String)>),
    Autostart(bool),
    CheckUpdates,
    Open(String),
}

/// Where the "Check for updates" button is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum UpdateState {
    Idle,
    Checking,
    Done(Check),
}

impl UpdateState {
    fn label(&self) -> String {
        match self {
            UpdateState::Idle => "Check for updates".into(),
            UpdateState::Checking => "Checking…".into(),
            UpdateState::Done(Check::UpToDate) => {
                format!("Up to date · v{}", env!("CARGO_PKG_VERSION"))
            }
            UpdateState::Done(Check::Available { version, .. }) => {
                format!("Download v{version}")
            }
            UpdateState::Done(Check::Failed(_)) => "Couldn't check · Retry".into(),
        }
    }
}

struct Settings {
    hwnd: HWND,
    /// Device pixels per DIP: the monitor DPI times `zoom`.
    scale: f32,
    /// The Windows text size setting, as far as the window still fits the screen.
    zoom: f32,
    theme: Theme,
    status: Option<Status>,
    /// The settings as shown; ahead of `status` while stepper changes are pending.
    config: Config,
    /// Stepper changes not sent to the service yet.
    unsent: Vec<Control>,
    autostart: bool,
    update: UpdateState,
    page: Page,
    /// Clickable areas from the last paint.
    hits: Vec<(Hit, Rect)>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    focus: Option<Control>,
    /// The focus ring only shows after keyboard use, as in the rest of Windows.
    focus_visible: bool,
    /// The next digit typed into the custom number appends to it instead of replacing it.
    typing: bool,
    tracking_mouse: bool,
    // Last: shut down after everything else is released.
    _gdiplus: Gdiplus,
}

thread_local! {
    static SETTINGS: RefCell<Option<Settings>> = const { RefCell::new(None) };
    /// The page shown when the window was last closed: it opens there again.
    static LAST_PAGE: Cell<Page> = const { Cell::new(Page::Overview) };
}

/// Runs `f` on the window state, unless the window is closed or the state is already borrowed
/// (a re-entrant message).
fn with<R>(f: impl FnOnce(&mut Settings) -> R) -> Option<R> {
    SETTINGS.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn is_open() -> bool {
    SETTINGS.with(|s| s.try_borrow().map_or(true, |s| s.is_some()))
}

/// Opens the window, or brings it to the front if it is already open.
pub fn open() {
    if let Some(hwnd) = with(|s| s.hwnd) {
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
        return;
    }
    let Some(gdiplus) = Gdiplus::start() else {
        return;
    };

    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let class = wide(CLASS);
    let wc = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        // Resource 1 is the application icon embedded by build.rs.
        hIcon: unsafe { LoadIconW(instance, 1 as _) },
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        ..unsafe { mem::zeroed() }
    };
    // Fails harmlessly when the class is already registered by an earlier opening.
    unsafe { RegisterClassW(&wc) };

    // Center on the monitor with the cursor (where the tray icon was clicked).
    let (x, y, w, h, zoom) = unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..mem::zeroed()
        };
        GetMonitorInfoW(monitor, &mut info);
        let (mut dpi_x, mut dpi_y) = (96, 96);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let work = info.rcWork;
        // Larger text enlarges the whole window, as long as it fits the screen.
        let fit = |screen: i32, size: f32| screen as f32 / (size * dpi_x as f32 / 96.0);
        let room = fit(work.right - work.left, WIDTH).min(fit(work.bottom - work.top, HEIGHT));
        let zoom = theme::text_scale().min(room * 0.95).max(1.0);
        let (w, h) = window_size(dpi_x, zoom);
        let x = work.left + (work.right - work.left - w).max(0) / 2;
        let y = work.top + (work.bottom - work.top - h).max(0) / 2;
        (x, y, w, h, zoom)
    };

    let title = wide("CoolerCast");
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            STYLE,
            x,
            y,
            w,
            h,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        return;
    }

    let glass = theme::transparency_allowed() && enable_backdrop(hwnd);
    let theme = Theme::current(glass);
    let status = crate::tray::current_status();
    let state = Settings {
        hwnd,
        scale: unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0 * zoom,
        zoom,
        config: status
            .as_ref()
            .map(|s| s.config.clone())
            .unwrap_or_default(),
        status,
        theme,
        unsent: Vec::new(),
        autostart: autostart::enabled(),
        update: UpdateState::Idle,
        page: LAST_PAGE.get(),
        hits: Vec::new(),
        hover: None,
        pressed: None,
        focus: None,
        focus_visible: false,
        typing: false,
        tracking_mouse: false,
        _gdiplus: gdiplus,
    };
    style_title_bar(hwnd, &state.theme);
    SETTINGS.with(|s| *s.borrow_mut() = Some(state));
    crate::tray::set_interactive(true);
    unsafe {
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
    }
    // Poll faster while the window is open.
    crate::tray::refresh(false);
}

pub fn close() {
    if let Some(hwnd) = with(|s| s.hwnd) {
        unsafe { DestroyWindow(hwnd) };
    }
}

/// Shows a new status from the service.
pub fn update(status: Option<&Status>) {
    with(|s| {
        s.status = status.cloned();
        if s.unsent.is_empty()
            && let Some(status) = status
        {
            s.config = status.config.clone();
        }
        s.invalidate();
    });
}

/// Window size, frame included, for a monitor DPI and zoom.
fn window_size(dpi: u32, zoom: f32) -> (i32, i32) {
    let scale = dpi as f32 / 96.0 * zoom;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: (WIDTH * scale).round() as i32,
        bottom: (HEIGHT * scale).round() as i32,
    };
    unsafe { AdjustWindowRectExForDpi(&mut rect, STYLE, 0, 0, dpi) };
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// Turns on the Acrylic backdrop behind the whole window. Fails before Windows 11 22H2.
fn enable_backdrop(hwnd: HWND) -> bool {
    let kind = DWMSBT_TRANSIENTWINDOW;
    let set = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&kind as *const i32).cast(),
            size_of::<i32>() as u32,
        )
    };
    if set < 0 {
        return false;
    }
    // The backdrop shows wherever the client area is transparent.
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) >= 0 }
}

/// Dark title bar in dark mode. Without the backdrop, Windows 11 also paints the caption with
/// the window background.
fn style_title_bar(hwnd: HWND, theme: &Theme) {
    let dark = i32::from(theme.dark);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark as *const i32).cast(),
            size_of::<i32>() as u32,
        );
    }
    if !theme.glass {
        let caption = theme.background_colorref();
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_CAPTION_COLOR as u32,
                (&caption as *const u32).cast(),
                size_of::<u32>() as u32,
            );
        }
    }
}

/// Sends settings changes to the service, then refreshes the tray icon and the window.
fn run(effect: Effect) {
    match effect {
        Effect::None => {}
        Effect::Send(changes) => {
            for (key, value) in changes {
                let _ = coolercast_core::ipc::set(key, &value);
            }
            crate::tray::refresh(false);
        }
        Effect::Autostart(enable) => {
            autostart::set(enable);
            with(|s| {
                s.autostart = autostart::enabled();
                s.invalidate();
            });
        }
        Effect::CheckUpdates => {
            // HWND is not Send; the thread only posts a message back to it.
            let Some(hwnd) = with(|s| s.hwnd as usize) else {
                return;
            };
            thread::spawn(move || {
                let result = update::check();
                *UPDATE_RESULT.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                unsafe { PostMessageW(hwnd as HWND, WM_UPDATE_CHECKED, 0, 0) };
            });
        }
        Effect::Open(url) => {
            let (verb, url) = (wide("open"), wide(&url));
            unsafe {
                ShellExecuteW(
                    ptr::null_mut(),
                    verb.as_ptr(),
                    url.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    SW_SHOWNORMAL,
                )
            };
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
        WM_PAINT => paint(hwnd),
        // Everything is painted in WM_PAINT; skipping the erase avoids flicker.
        WM_ERASEBKGND => return 1,
        WM_MOUSEMOVE => {
            let (x, y) = mouse_pos(lparam);
            with(|s| s.mouse_move(x, y));
        }
        WM_MOUSELEAVE => {
            with(|s| {
                s.tracking_mouse = false;
                if s.hover.take().is_some() {
                    s.invalidate();
                }
            });
        }
        WM_LBUTTONDOWN => {
            let (x, y) = mouse_pos(lparam);
            let shift = wparam & MK_SHIFT != 0;
            unsafe { SetCapture(hwnd) };
            let effect = with(|s| s.mouse_down(x, y, shift)).unwrap_or(Effect::None);
            run(effect);
        }
        WM_LBUTTONUP => {
            unsafe { ReleaseCapture() };
            let (x, y) = mouse_pos(lparam);
            let effect = with(|s| s.mouse_up(x, y)).unwrap_or(Effect::None);
            run(effect);
        }
        WM_KEYDOWN => {
            if wparam as u16 == VK_ESCAPE {
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            } else {
                let shift = unsafe { GetKeyState(VK_SHIFT.into()) } < 0;
                let effect = with(|s| s.key(wparam as u16, shift)).unwrap_or(Effect::None);
                run(effect);
            }
        }
        WM_CHAR => {
            let effect = char::from_u32(wparam as u32)
                .and_then(|ch| with(|s| s.typed(ch)))
                .unwrap_or(Effect::None);
            run(effect);
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam >> 16) as u16 as i16;
            with(|s| s.wheel(delta > 0));
        }
        WM_TIMER => {
            let effect = with(|s| s.timer(wparam)).unwrap_or(Effect::None);
            run(effect);
        }
        WM_UPDATE_CHECKED => {
            let result = UPDATE_RESULT
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(result) = result {
                with(|s| {
                    s.update = UpdateState::Done(result);
                    s.invalidate();
                });
            }
        }
        WM_DPICHANGED => {
            let dpi = u32::from(wparam as u16);
            with(|s| {
                s.scale = dpi as f32 / 96.0 * s.zoom;
                s.invalidate();
            });
            // The suggested rectangle keeps the window under the cursor while dragging.
            let r = unsafe { &*(lparam as *const RECT) };
            unsafe {
                SetWindowPos(
                    hwnd,
                    ptr::null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
        }
        WM_SETTINGCHANGE if is_color_change(lparam) => {
            let glass = with(|s| s.theme.glass).unwrap_or(false);
            let theme = Theme::current(glass);
            style_title_bar(hwnd, &theme);
            with(|s| {
                s.theme = theme;
                s.invalidate();
            });
        }
        WM_DESTROY => {
            // Take the state out first: dropping it shuts GDI+ down.
            let state = SETTINGS.with(|s| s.try_borrow_mut().ok()?.take());
            if let Some(mut state) = state {
                unsafe {
                    KillTimer(hwnd, TIMER_COMMIT);
                    KillTimer(hwnd, TIMER_REPEAT);
                }
                let changes = state.take_unsent();
                drop(state);
                if !changes.is_empty() {
                    run(Effect::Send(changes));
                }
            }
            crate::tray::set_interactive(false);
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    0
}

/// Mouse position in device-independent pixels.
fn mouse_pos(lparam: LPARAM) -> (f32, f32) {
    let x = f32::from(lparam as u16 as i16);
    let y = f32::from((lparam >> 16) as u16 as i16);
    with(|s| (x / s.scale, y / s.scale)).unwrap_or((x, y))
}

/// `WM_SETTINGCHANGE` with "ImmersiveColorSet": the light/dark setting may have changed.
fn is_color_change(lparam: LPARAM) -> bool {
    if lparam == 0 {
        return false;
    }
    let expected = wide("ImmersiveColorSet");
    let text = lparam as *const u16;
    // Compare up to and including the terminating null.
    expected
        .iter()
        .enumerate()
        .all(|(i, &c)| unsafe { *text.add(i) } == c)
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client: RECT = mem::zeroed();
        GetClientRect(hwnd, &mut client);
        let (w, h) = (client.right, client.bottom);
        // Draw off-screen and copy once, so nothing flickers.
        let mem_dc = CreateCompatibleDC(hdc);
        let glass = with(|s| s.theme.glass).unwrap_or(false);
        let (bitmap, bits) = if glass {
            pixel_bitmap(hdc, w, h)
        } else {
            (CreateCompatibleBitmap(hdc, w, h), ptr::null_mut())
        };
        let previous = SelectObject(mem_dc, bitmap);
        with(|s| {
            // Glass needs per-pixel alpha, which only drawing on the pixels directly keeps.
            let canvas = if bits.is_null() {
                Canvas::new(mem_dc, s.scale)
            } else {
                Canvas::on_pixels(bits, w, h, s.scale)
            };
            if let Some(canvas) = canvas {
                s.draw(&canvas);
            }
        });
        BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
        SelectObject(mem_dc, previous);
        DeleteObject(bitmap);
        DeleteDC(mem_dc);
        EndPaint(hwnd, &ps);
    }
}

/// A 32-bit top-down DIB and its pixels.
fn pixel_bitmap(hdc: HDC, w: i32, h: i32) -> (HBITMAP, *mut u8) {
    let mut bmi: BITMAPINFO = unsafe { mem::zeroed() };
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits = ptr::null_mut();
    let bitmap =
        unsafe { CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0) };
    (bitmap, bits.cast())
}

impl Settings {
    fn invalidate(&self) {
        unsafe { InvalidateRect(self.hwnd, ptr::null(), 0) };
    }

    fn online(&self) -> bool {
        self.status.is_some()
    }

    fn enabled(&self, control: Control) -> bool {
        control.enabled(&self.config, self.online())
    }

    fn hit_test(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(hit, r)| r.contains(x, y) && self.enabled(hit.control))
            .map(|&(hit, _)| hit)
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        if !self.tracking_mouse {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe { TrackMouseEvent(&mut tme) };
            self.tracking_mouse = true;
        }
        let hover = self.hit_test(x, y);
        if hover != self.hover {
            self.hover = hover;
            self.invalidate();
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32, shift: bool) -> Effect {
        let Some(hit) = self.hit_test(x, y) else {
            return Effect::None;
        };
        self.pressed = Some(hit);
        self.set_focus(Some(hit.control));
        self.focus_visible = false;
        // A click on the number or its buttons makes the next digit start a new number.
        self.typing = false;
        self.invalidate();
        match hit.part {
            Part::Minus | Part::Plus => {
                unsafe { SetTimer(self.hwnd, TIMER_REPEAT, REPEAT_DELAY_MS, None) };
                // Shift-click takes ten steps at once, for the wide ranges.
                for _ in 0..if shift { 10 } else { 1 } {
                    self.step(hit.control, hit.part == Part::Plus);
                }
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn mouse_up(&mut self, x: f32, y: f32) -> Effect {
        let pressed = self.pressed.take();
        unsafe { KillTimer(self.hwnd, TIMER_REPEAT) };
        self.invalidate();
        match pressed {
            Some(hit) if self.hit_test(x, y) == Some(hit) => self.activate(hit),
            _ => Effect::None,
        }
    }

    fn timer(&mut self, id: usize) -> Effect {
        match id {
            TIMER_COMMIT => {
                unsafe { KillTimer(self.hwnd, TIMER_COMMIT) };
                Effect::Send(self.take_unsent())
            }
            TIMER_REPEAT => {
                match self.pressed {
                    Some(
                        hit @ Hit {
                            part: Part::Minus | Part::Plus,
                            ..
                        },
                    ) if self.hover == Some(hit) => {
                        unsafe { SetTimer(self.hwnd, TIMER_REPEAT, REPEAT_RATE_MS, None) };
                        self.step(hit.control, hit.part == Part::Plus);
                    }
                    Some(_) => {}
                    None => unsafe {
                        KillTimer(self.hwnd, TIMER_REPEAT);
                    },
                }
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn key(&mut self, key: u16, shift: bool) -> Effect {
        self.focus_visible = true;
        self.invalidate();
        if key == VK_TAB {
            let (config, online) = (self.config.clone(), self.online());
            let order = focus_order(self.page, &config);
            self.set_focus(next_focus(&order, self.focus, !shift, |c| {
                c.enabled(&config, online)
            }));
            return Effect::None;
        }
        let Some(control) = self.focus.filter(|&c| self.enabled(c)) else {
            return Effect::None;
        };
        let forward = match key {
            VK_RIGHT | VK_DOWN if matches!(control, Control::Nav(_)) => true,
            VK_LEFT | VK_UP if matches!(control, Control::Nav(_)) => false,
            VK_RIGHT | VK_UP => true,
            VK_LEFT | VK_DOWN => false,
            VK_SPACE | VK_RETURN => {
                let part = match control {
                    Control::Update | Control::UseCustom => Part::Button,
                    // Enter confirms a typed number right away.
                    Control::CustomValue if key == VK_RETURN => {
                        self.typing = false;
                        unsafe { KillTimer(self.hwnd, TIMER_COMMIT) };
                        return Effect::Send(self.take_unsent());
                    }
                    _ => Part::Switch,
                };
                return self.activate(Hit { control, part });
            }
            _ => return Effect::None,
        };
        self.typing = false;
        match control {
            // The arrows move through the sidebar.
            Control::Nav(page) => {
                let i = PAGES.iter().position(|&p| p == page).unwrap_or(0);
                let next = if forward {
                    (i + 1).min(PAGES.len() - 1)
                } else {
                    i.saturating_sub(1)
                };
                self.page = PAGES[next];
                LAST_PAGE.set(self.page);
                self.set_focus(Some(Control::Nav(self.page)));
                Effect::None
            }
            Control::Mode | Control::Source | Control::Unit | Control::CustomSymbol => {
                let count = control.segments().len();
                let current = control.selected_segment(&self.config);
                let index = if forward {
                    (current + 1).min(count - 1)
                } else {
                    current.saturating_sub(1)
                };
                if index == current {
                    return Effect::None;
                }
                self.activate(Hit {
                    control,
                    part: Part::Segment(index),
                })
            }
            Control::AutoInterval
            | Control::Interval
            | Control::Threshold
            | Control::CustomValue
            | Control::CustomBar
            | Control::NightStart
            | Control::NightEnd => {
                self.step(control, forward);
                Effect::None
            }
            _ => Effect::None,
        }
    }

    /// A character typed while the custom number has the focus.
    fn typed(&mut self, key: char) -> Effect {
        let control = Control::CustomValue;
        if self.focus != Some(control) || !self.enabled(control) {
            return Effect::None;
        }
        if let Some(value) = typed(self.config.custom_value, key, !self.typing) {
            let config = Config {
                custom_value: value,
                ..self.config.clone()
            };
            self.set_local(control, config);
            self.typing = true;
        }
        Effect::None
    }

    /// The mouse wheel steps the stepper under the cursor.
    fn wheel(&mut self, up: bool) {
        if let Some(hit) = self.hover
            && matches!(hit.part, Part::Minus | Part::Plus | Part::Field)
        {
            self.typing = false;
            self.step(hit.control, up);
        }
    }

    fn set_focus(&mut self, focus: Option<Control>) {
        if focus != self.focus {
            self.focus = focus;
            self.typing = false;
        }
    }

    /// Applies a click on a segment, a switch, a button or the sidebar. Stepper buttons act on
    /// mouse down instead.
    fn activate(&mut self, hit: Hit) -> Effect {
        match (hit.control, hit.part) {
            (Control::Nav(page), _) => {
                self.page = page;
                LAST_PAGE.set(page);
                self.invalidate();
                Effect::None
            }
            (Control::UseCustom, _) => {
                self.config.mode = Mode::Custom;
                self.send_with(Control::Mode)
            }
            (Control::Autostart, _) => Effect::Autostart(!self.autostart),
            (Control::Update, _) => match &self.update {
                UpdateState::Checking => Effect::None,
                UpdateState::Done(Check::Available { url, .. }) => Effect::Open(url.clone()),
                _ => {
                    self.update = UpdateState::Checking;
                    self.invalidate();
                    Effect::CheckUpdates
                }
            },
            (control, Part::Segment(index)) => {
                self.config = control.with_segment(&self.config, index);
                self.send_with(control)
            }
            (control, Part::Switch) => match control.toggled(&self.config) {
                Some(config) => {
                    self.config = config;
                    self.send_with(control)
                }
                None => Effect::None,
            },
            _ => Effect::None,
        }
    }

    fn step(&mut self, control: Control, up: bool) {
        if let Some(config) = control.stepped(&self.config, up) {
            self.set_local(control, config);
        }
    }

    /// Shows a changed value at once and sends it when it settles.
    fn set_local(&mut self, control: Control, config: Config) {
        self.config = config;
        if !self.unsent.contains(&control) {
            self.unsent.push(control);
        }
        unsafe { SetTimer(self.hwnd, TIMER_COMMIT, COMMIT_DELAY_MS, None) };
        self.invalidate();
    }

    /// Sends `control` now, along with any pending stepper changes.
    fn send_with(&mut self, control: Control) -> Effect {
        unsafe { KillTimer(self.hwnd, TIMER_COMMIT) };
        if !self.unsent.contains(&control) {
            self.unsent.push(control);
        }
        self.invalidate();
        Effect::Send(self.take_unsent())
    }

    fn take_unsent(&mut self) -> Vec<(&'static str, String)> {
        let config = &self.config;
        self.unsent
            .drain(..)
            .filter_map(|c| c.setting(config))
            .collect()
    }

    // ---- Drawing ----

    fn draw(&mut self, c: &Canvas) {
        let t = self.theme;
        c.clear(t.backdrop);
        self.hits.clear();
        self.draw_sidebar(c);

        let page = self.page;
        c.text(
            page.title(),
            Rect::new(PAGE_X, 22.0, PAGE_W, 36.0),
            26.0,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        c.text(
            page.subtitle(),
            Rect::new(PAGE_X + 1.0, 56.0, PAGE_W, 18.0),
            13.0,
            Weight::Regular,
            t.text_dim,
            Align::Left,
        );
        match page {
            Page::Overview => self.draw_overview(c),
            _ => self.draw_settings(c, page),
        }
    }

    fn draw_sidebar(&mut self, c: &Canvas) {
        let t = self.theme;
        draw_app_icon(c, Rect::new(20.0, 24.0, 40.0, 40.0));
        c.text(
            "CoolerCast",
            Rect::new(72.0, 22.0, SIDEBAR_W - 80.0, 22.0),
            16.0,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        // The service state, where it is always visible.
        let (level, title, detail) = status_lines(self.status.as_ref());
        let dot = match level {
            Level::Ok => t.ok,
            Level::Warn => t.warn,
            Level::Error => t.alarm,
        };
        c.fill_circle(76.0, 53.0, 4.0, dot);
        c.text(
            &title,
            Rect::new(86.0, 44.0, SIDEBAR_W - 94.0, 18.0),
            12.5,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        c.text(
            &detail,
            Rect::new(72.0, 63.0, SIDEBAR_W - 80.0, 18.0),
            12.0,
            Weight::Regular,
            t.text_dim,
            Align::Left,
        );

        for (i, &page) in PAGES.iter().enumerate() {
            let r = Rect::new(
                10.0,
                NAV_TOP + i as f32 * (NAV_H + 2.0),
                SIDEBAR_W - 20.0,
                NAV_H,
            );
            let hit = Hit {
                control: Control::Nav(page),
                part: Part::Button,
            };
            let (hovered, pressed) = self.state(hit);
            let selected = self.page == page;
            if selected || hovered {
                let fill = if pressed {
                    t.control_hover
                } else if selected {
                    t.control
                } else {
                    t.control.alpha(0x99)
                };
                c.fill_round_rect(r, 6.0, fill);
            }
            if selected {
                let bar = Rect::new(r.x, r.y + 11.0, 3.0, r.h - 22.0);
                c.fill_round_rect(bar, 1.5, t.accent);
            }
            c.text(
                page.icon(),
                Rect::new(r.x + 14.0, r.y, 20.0, r.h),
                16.0,
                Weight::Icon,
                t.text,
                Align::Center,
            );
            c.text(
                page.title(),
                Rect::new(r.x + 46.0, r.y, r.w - 52.0, r.h),
                14.0,
                if selected {
                    Weight::Semibold
                } else {
                    Weight::Regular
                },
                t.text,
                Align::Left,
            );
            if self.focus_visible && self.focus == Some(hit.control) {
                c.stroke_round_rect(r.inset(-2.0, -2.0), 8.0, 2.0, t.focus);
            }
            self.hits.push((hit, r));
        }
    }

    /// The overview: the display (or every value, for displays that show several), what it
    /// shows, and the history.
    fn draw_overview(&mut self, c: &Canvas) {
        let t = self.theme;
        let hero = Rect::new(PAGE_X, PAGE_TOP, PAGE_W, 214.0);
        self.draw_card(c, hero);
        self.draw_readout(c, hero);
        let chart = Rect::new(
            PAGE_X,
            hero.bottom() + 12.0,
            PAGE_W,
            HEIGHT - hero.bottom() - 12.0 - 20.0,
        );
        self.draw_chart(c, chart);
        let _ = t;
    }

    /// A page of setting cards.
    fn draw_settings(&mut self, c: &Canvas, page: Page) {
        let mut y = PAGE_TOP;
        for &(control, title, description) in page.settings() {
            if !control.visible(&self.config) {
                continue;
            }
            let row = Rect::new(PAGE_X, y, PAGE_W, SETTING_H);
            self.draw_setting(c, row, control, title, description);
            y = row.bottom() + SETTING_GAP;
        }
    }

    fn draw_card(&self, c: &Canvas, r: Rect) {
        let t = &self.theme;
        c.fill_round_rect_v(r, RADIUS, t.card_top, t.card_bottom);
        c.stroke_round_rect_v(r, RADIUS, 1.0, t.edge_top, t.edge_bottom);
    }

    /// The glassy sheen on accent-filled controls: a highlight fading out over the top half.
    fn gloss(&self, c: &Canvas, r: Rect, radius: f32) {
        if self.theme.glass {
            let top = Rect::new(r.x, r.y, r.w, r.h * 0.55);
            let white = Color::rgb(0xFF, 0xFF, 0xFF);
            c.fill_round_rect_v(top, radius, white.alpha(0x40), white.alpha(0x00));
        }
    }

    /// Inside the hero card: the AK preview and what it shows on the left, the values on the
    /// right; or only the values, in three columns, for displays the preview does not match.
    fn draw_readout(&self, c: &Canvas, card: Rect) {
        let t = self.theme;
        let status = self.status.as_ref();
        let preview = status.is_none_or(|s| preview::applies_to(&s.devices));
        let mut values_x = card.x + 24.0;
        if preview {
            // In custom mode the preview follows the settings as they are edited.
            let frame = match status {
                Some(s) if self.config.mode == Mode::Custom => {
                    Some(Frame::custom(&self.config, s.alarm_active))
                }
                _ => status.and_then(Frame::from_status),
            };
            let area = Rect::new(card.x + 20.0, card.y + 20.0, 276.0, 165.0);
            preview::draw(
                c,
                area,
                frame.filter(|_| status.is_none_or(|s| s.display_off.is_none())),
            );
            c.text(
                &showing(status),
                Rect::new(area.x, area.bottom() + 6.0, area.w, 18.0),
                12.0,
                Weight::Regular,
                t.text_dim,
                Align::Center,
            );
            values_x = area.right() + 28.0;
        } else if let Some(s) = status {
            c.text(
                &showing(Some(s)),
                Rect::new(card.x + 24.0, card.bottom() - 30.0, card.w - 48.0, 18.0),
                12.0,
                Weight::Regular,
                t.text_dim,
                Align::Left,
            );
        }

        let unit = self.config.unit;
        let text = |value: Option<f32>, format: &dyn Fn(f32) -> String| {
            value.map_or_else(|| "–".to_owned(), format)
        };
        let temp = |value| text(value, &|v| format_temp(v, unit));
        let percent = |value| text(value, &|u| format!("{u:.0} %"));
        let alarm = status.is_some_and(|s| s.alarm_active);
        let mut values = vec![
            (
                "CPU temperature",
                temp(status.and_then(|s| s.cpu_temp)),
                if alarm { t.alarm } else { t.text },
            ),
            (
                "CPU usage",
                percent(status.and_then(|s| s.cpu_usage)),
                t.text,
            ),
        ];
        if let Some(s) = status.filter(|s| s.gpu.temp.is_some() || s.gpu.usage.is_some()) {
            values.push(("GPU temperature", temp(s.gpu.temp), t.text));
            values.push(("GPU usage", percent(s.gpu.usage), t.text));
        }
        if let Some(w) = status.and_then(|s| s.cpu_power) {
            values.push(("CPU power", format!("{w:.0} W"), t.text));
        }
        if let Some(f) = status.and_then(|s| s.cpu_freq) {
            values.push(("CPU clock", format!("{f:.0} MHz"), t.text));
        }
        // With only the CPU measured, the peaks of the chart are worth a glance too.
        if values.len() == 2
            && let Some(s) = status.filter(|s| !s.history.is_empty())
        {
            let peak_temp = s.history.iter().filter_map(|h| h.cpu_temp).reduce(f32::max);
            let peak_usage = s.history.iter().map(|h| h.cpu_usage).reduce(f32::max);
            values.push(("Highest temperature", temp(peak_temp), t.text));
            values.push(("Highest usage", percent(peak_usage), t.text));
        }
        let columns = if preview { 2 } else { 3 };
        let rows = values.len().div_ceil(columns).max(1);
        let w = (card.right() - 20.0 - values_x) / columns as f32;
        let row_h = ((card.h - 40.0) / rows as f32).min(62.0);
        for (i, (label, value, color)) in values.into_iter().enumerate() {
            let x = values_x + (i % columns) as f32 * w;
            let y = card.y + 22.0 + (i / columns) as f32 * row_h;
            c.text(
                label,
                Rect::new(x, y, w - 8.0, 16.0),
                12.0,
                Weight::Regular,
                t.text_dim,
                Align::Left,
            );
            c.text(
                &value,
                Rect::new(x, y + 17.0, w - 8.0, 32.0),
                24.0,
                Weight::Semibold,
                color,
                Align::Left,
            );
        }
    }

    fn draw_chart(&self, c: &Canvas, card: Rect) {
        let t = self.theme;
        self.draw_card(c, card);
        let span = HISTORY_LEN as u32 * self.config.interval_ms / 1000;
        c.text(
            &format!("Last {}", format_seconds(span)),
            Rect::new(card.x + 16.0, card.y + 12.0, 160.0, 20.0),
            13.0,
            Weight::Semibold,
            t.text,
            Align::Left,
        );

        // Legend, right-aligned.
        let mut lx = card.right() - 16.0;
        for (label, color) in [("Usage", t.usage_line), ("Temperature", t.temp_line)] {
            let w = c.measure(label, 12.0, Weight::Regular);
            lx -= w;
            c.text(
                label,
                Rect::new(lx, card.y + 12.0, w + 2.0, 20.0),
                12.0,
                Weight::Regular,
                t.text_dim,
                Align::Left,
            );
            lx -= 18.0;
            c.line(lx, card.y + 22.0, lx + 12.0, card.y + 22.0, 2.5, color);
            lx -= 14.0;
        }

        // Temperature (°C) and usage (%) share one 0-100 grid: temperature labels on the left,
        // usage labels on the right.
        let plot = Rect::new(
            card.x + 48.0,
            card.y + 48.0,
            card.w - 48.0 - 44.0,
            card.h - 48.0 - 40.0,
        );
        let y_of = |v: f32| plot.bottom() - v.clamp(0.0, 100.0) / 100.0 * plot.h;
        let unit = self.config.unit;
        for v in [0.0f32, 25.0, 50.0, 75.0, 100.0] {
            let y = y_of(v);
            c.line(plot.x, y, plot.right(), y, 1.0, t.grid);
            c.text(
                &format!("{:.0}°", unit.from_celsius(v)),
                Rect::new(card.x + 8.0, y - 8.0, 32.0, 16.0),
                11.0,
                Weight::Regular,
                t.text_dim,
                Align::Right,
            );
            c.text(
                &format!("{v:.0}%"),
                Rect::new(plot.right() + 6.0, y - 8.0, 36.0, 16.0),
                11.0,
                Weight::Regular,
                t.text_dim,
                Align::Left,
            );
        }
        c.text(
            &format!("-{}", format_seconds(span)),
            Rect::new(plot.x, plot.bottom() + 6.0, 80.0, 16.0),
            11.0,
            Weight::Regular,
            t.text_dim,
            Align::Left,
        );
        c.text(
            "now",
            Rect::new(plot.right() - 80.0, plot.bottom() + 6.0, 80.0, 16.0),
            11.0,
            Weight::Regular,
            t.text_dim,
            Align::Right,
        );

        if self.config.alarm && self.online() {
            let y = y_of(f32::from(self.config.alarm_threshold));
            c.dashed_line(plot.x, y, plot.right(), y, 1.0, t.alarm.alpha(0xB0));
            c.text(
                "Alarm",
                Rect::new(plot.right() - 60.0, y - 17.0, 56.0, 14.0),
                11.0,
                Weight::Regular,
                t.text_dim,
                Align::Right,
            );
        }

        let history = self.status.as_ref().map_or(&[][..], |s| &s.history[..]);
        if history.len() < 2 {
            c.text(
                if self.online() {
                    "Collecting data…"
                } else {
                    "No data"
                },
                plot,
                12.0,
                Weight::Regular,
                t.text_dim,
                Align::Center,
            );
            return;
        }
        // Newest sample on the right edge.
        let dx = plot.w / (HISTORY_LEN - 1) as f32;
        let x_of = |i: usize| plot.right() - (history.len() - 1 - i) as f32 * dx;

        let usage: Vec<(f32, f32)> = history
            .iter()
            .enumerate()
            .map(|(i, s)| (x_of(i), y_of(s.cpu_usage)))
            .collect();
        c.polyline(&usage, 1.5, t.usage_line);

        // Temperature: one filled line per run of samples that have a value.
        let mut run: Vec<(f32, f32)> = Vec::new();
        for (i, sample) in history.iter().enumerate() {
            if let Some(temp) = sample.cpu_temp {
                run.push((x_of(i), y_of(temp)));
            }
            if sample.cpu_temp.is_none() || i == history.len() - 1 {
                self.draw_temp_run(c, &run, plot.bottom());
                run.clear();
            }
        }
    }

    fn draw_temp_run(&self, c: &Canvas, points: &[(f32, f32)], baseline: f32) {
        let (Some(&(first, _)), Some(&(last, _))) = (points.first(), points.last()) else {
            return;
        };
        let mut area = points.to_vec();
        area.push((last, baseline));
        area.push((first, baseline));
        c.fill_polygon(&area, self.theme.temp_line.alpha(0x30));
        c.polyline(points, 2.0, self.theme.temp_line);
    }

    /// One setting card: title and description on the left, the control on the right.
    fn draw_setting(
        &mut self,
        c: &Canvas,
        row: Rect,
        control: Control,
        title: &str,
        description: &str,
    ) {
        let t = self.theme;
        self.draw_card(c, row);
        let enabled = self.enabled(control);
        let right = row.right() - 18.0;
        let cy = row.y + row.h / 2.0;
        let config = self.config.clone();
        let area = match control {
            Control::Mode | Control::Source | Control::Unit | Control::CustomSymbol => {
                self.draw_segmented(c, control, right, cy, enabled)
            }
            Control::Autostart => self.draw_switch(c, control, self.autostart, right, cy, true),
            Control::Bar
            | Control::OffLocked
            | Control::OffScreen
            | Control::OffNight
            | Control::Alarm => {
                self.draw_switch(c, control, control.is_on(&config), right, cy, enabled)
            }
            Control::Update => self.draw_update_button(c, right, cy),
            Control::UseCustom => self.draw_button(c, control, "Show it", right, cy, enabled),
            Control::CustomValue => {
                let text = config.custom_value.to_string();
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::CustomBar => {
                let text = format!("{} / {}", config.custom_bar, config::CUSTOM_BAR.1);
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::AutoInterval => {
                let text = format_seconds(config.auto_interval_s);
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::Interval => {
                let text = format_ms(config.interval_ms);
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::Threshold => {
                let text = format_temp(config.alarm_threshold.into(), config.unit);
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::NightStart => {
                let text = config.night_start.to_string();
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::NightEnd => {
                let text = config.night_end.to_string();
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::Nav(_) => return,
        };
        let text_w = area.x - row.x - 36.0;
        c.text(
            title,
            Rect::new(row.x + 18.0, row.y + 12.0, text_w, 20.0),
            14.0,
            Weight::Regular,
            fade(t.text, enabled),
            Align::Left,
        );
        c.text(
            description,
            Rect::new(row.x + 18.0, row.y + 33.0, text_w, 18.0),
            12.0,
            Weight::Regular,
            fade(t.text_dim, enabled),
            Align::Left,
        );
        if self.focus_visible && self.focus == Some(control) {
            c.stroke_round_rect(area.inset(-3.0, -3.0), 8.0, 2.0, t.focus);
        }
    }

    fn state(&self, hit: Hit) -> (bool, bool) {
        let hovered = self.hover == Some(hit);
        let pressed = self.pressed == Some(hit) && hovered;
        (hovered, pressed)
    }

    fn draw_segmented(
        &mut self,
        c: &Canvas,
        control: Control,
        right: f32,
        cy: f32,
        enabled: bool,
    ) -> Rect {
        let t = self.theme;
        let options = control.segments();
        let widths: Vec<f32> = options
            .iter()
            .map(|o| {
                (c.measure(o, 12.5, Weight::Semibold) + 24.0)
                    .max(44.0)
                    .round()
            })
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + 4.0;
        let track = Rect::new(right - total, cy - 16.0, total, 32.0);
        c.fill_round_rect(track, 6.0, fade(t.control, enabled));

        let selected = control.selected_segment(&self.config);
        let mut x = track.x + 2.0;
        for (i, (&option, &w)) in options.iter().zip(&widths).enumerate() {
            let r = Rect::new(x, track.y + 2.0, w, 28.0);
            let hit = Hit {
                control,
                part: Part::Segment(i),
            };
            let (hovered, _) = self.state(hit);
            let (fill, text, weight) = if i == selected {
                (Some(t.accent), t.on_accent, Weight::Semibold)
            } else if hovered {
                (Some(t.control_hover), t.text, Weight::Regular)
            } else {
                (None, t.text, Weight::Regular)
            };
            if let Some(fill) = fill {
                c.fill_round_rect(r, 4.0, fade(fill, enabled));
                if i == selected && enabled {
                    self.gloss(c, r, 4.0);
                }
            }
            c.text(option, r, 12.5, weight, fade(text, enabled), Align::Center);
            self.hits.push((hit, r));
            x += w;
        }
        track
    }

    fn draw_switch(
        &mut self,
        c: &Canvas,
        control: Control,
        on: bool,
        right: f32,
        cy: f32,
        enabled: bool,
    ) -> Rect {
        let t = self.theme;
        let hit = Hit {
            control,
            part: Part::Switch,
        };
        let (hovered, _) = self.state(hit);
        let track = Rect::new(right - 40.0, cy - 10.0, 40.0, 20.0);
        if on {
            let fill = if hovered {
                t.accent.alpha(0xD8)
            } else {
                t.accent
            };
            c.fill_round_rect(track, 10.0, fade(fill, enabled));
            if enabled {
                self.gloss(c, track, 10.0);
            }
            c.fill_circle(track.right() - 10.0, cy, 6.0, fade(t.on_accent, enabled));
        } else {
            if hovered {
                c.fill_round_rect(track, 10.0, t.control_hover);
            }
            c.stroke_round_rect(track, 10.0, 1.5, fade(t.text_dim, enabled));
            c.fill_circle(track.x + 10.0, cy, 5.0, fade(t.text_dim, enabled));
        }
        let label = Rect::new(track.x - 44.0, cy - 10.0, 34.0, 20.0);
        c.text(
            if on { "On" } else { "Off" },
            label,
            12.5,
            Weight::Regular,
            fade(t.text_dim, enabled),
            Align::Right,
        );
        // The label is clickable too, as in the Windows settings.
        let area = Rect::new(label.x, track.y, track.right() - label.x, track.h);
        self.hits.push((hit, area.inset(0.0, -6.0)));
        track
    }

    /// A plain push button, right-aligned at `right`.
    fn draw_button(
        &mut self,
        c: &Canvas,
        control: Control,
        label: &str,
        right: f32,
        cy: f32,
        enabled: bool,
    ) -> Rect {
        let t = self.theme;
        let w = (c.measure(label, 13.0, Weight::Regular) + 32.0).round();
        let r = Rect::new(right - w, cy - 15.0, w, 30.0);
        let hit = Hit {
            control,
            part: Part::Button,
        };
        let (hovered, pressed) = self.state(hit);
        let fill = if pressed {
            t.border
        } else if hovered && enabled {
            t.control_hover
        } else {
            t.control
        };
        c.fill_round_rect(r, 6.0, fade(fill, enabled));
        c.stroke_round_rect(r, 6.0, 1.0, fade(t.border, enabled));
        c.text(
            label,
            r,
            13.0,
            Weight::Regular,
            fade(t.text, enabled),
            Align::Center,
        );
        self.hits.push((hit, r));
        r
    }

    /// The update button, right-aligned at `right`; filled with the accent when a new version
    /// is waiting.
    fn draw_update_button(&mut self, c: &Canvas, right: f32, cy: f32) -> Rect {
        let t = self.theme;
        let label = self.update.label();
        let available = matches!(self.update, UpdateState::Done(Check::Available { .. }));
        let weight = if available {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        let w = (c.measure(&label, 12.5, weight) + 24.0).round();
        let r = Rect::new(right - w, cy - 13.0, w, 26.0);
        let hit = Hit {
            control: Control::Update,
            part: Part::Button,
        };
        let (hovered, pressed) = self.state(hit);
        let busy = self.update == UpdateState::Checking;
        let (fill, text) = if available {
            let fill = if hovered {
                t.accent.alpha(0xD8)
            } else {
                t.accent
            };
            (fill, t.on_accent)
        } else if pressed {
            (t.border, t.text)
        } else if hovered && !busy {
            (t.control_hover, t.text)
        } else {
            (t.control, if busy { t.text_dim } else { t.text })
        };
        c.fill_round_rect(r, 13.0, fill);
        if available {
            self.gloss(c, r, 13.0);
        }
        c.text(&label, r, 12.5, weight, text, Align::Center);
        if self.focus_visible && self.focus == Some(Control::Update) {
            c.stroke_round_rect(r.inset(-3.0, -3.0), 16.0, 2.0, t.focus);
        }
        self.hits.push((hit, r));
        r
    }

    fn draw_stepper(
        &mut self,
        c: &Canvas,
        control: Control,
        value: &str,
        right: f32,
        cy: f32,
        enabled: bool,
    ) -> Rect {
        let t = self.theme;
        let area = Rect::new(right - 128.0, cy - 14.0, 128.0, 28.0);
        let buttons = [
            (Part::Minus, Rect::new(area.x, area.y, 28.0, 28.0)),
            (
                Part::Plus,
                Rect::new(area.right() - 28.0, area.y, 28.0, 28.0),
            ),
        ];
        for (part, r) in buttons {
            let hit = Hit { control, part };
            let (hovered, pressed) = self.state(hit);
            let can_step = control.stepped(&self.config, part == Part::Plus).is_some();
            let active = enabled && can_step;
            let fill = if pressed {
                t.border
            } else if hovered && active {
                t.control_hover
            } else {
                t.control
            };
            c.fill_round_rect(r, 6.0, fade(fill, active));
            let (mx, my) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let glyph = fade(t.text, active);
            c.line(mx - 5.0, my, mx + 5.0, my, 1.5, glyph);
            if part == Part::Plus {
                c.line(mx, my - 5.0, mx, my + 5.0, 1.5, glyph);
            }
            if can_step {
                self.hits.push((hit, r));
            }
        }
        let field = Rect::new(area.x + 32.0, area.y, area.w - 64.0, area.h);
        if control == Control::CustomValue {
            // A text box look: this number also takes typed digits.
            let hovered = self
                .state(Hit {
                    control,
                    part: Part::Field,
                })
                .0;
            let fill = if hovered && enabled {
                t.control_hover
            } else {
                t.control
            };
            c.fill_round_rect(field, 4.0, fade(fill, enabled));
            if enabled && self.focus == Some(control) {
                let y = field.bottom() - 1.0;
                c.line(field.x + 3.0, y, field.right() - 3.0, y, 2.0, t.accent);
            }
        }
        c.text(
            value,
            field,
            13.0,
            Weight::Semibold,
            fade(t.text, enabled),
            Align::Center,
        );
        // The whole stepper takes focus on click and steps with the mouse wheel.
        self.hits.push((
            Hit {
                control,
                part: Part::Field,
            },
            area,
        ));
        area
    }
}

/// Dims colors of disabled controls, keeping translucent colors proportionally translucent.
fn fade(color: Color, enabled: bool) -> Color {
    if enabled {
        color
    } else {
        let alpha = (color.0 >> 24) * 0x60 / 0xFF;
        color.alpha(alpha as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTO_STEPS: &[(u32, u32)] = &[(10, 1), (60, 5), (600, 30), (u32::MAX, 60)];

    #[test]
    fn step_snaps_to_the_step_of_each_range() {
        let up = |v| step(v, true, AUTO_STEPS, (1, 3600));
        let down = |v| step(v, false, AUTO_STEPS, (1, 3600));
        assert_eq!(
            [up(7), up(9), up(10), up(12), up(600)],
            [8, 10, 15, 15, 660]
        );
        assert_eq!(
            [down(10), down(12), down(60), down(61), down(600)],
            [9, 10, 55, 60, 570]
        );
    }

    #[test]
    fn step_stays_in_range() {
        assert_eq!(step(1, false, AUTO_STEPS, (1, 3600)), 1);
        assert_eq!(step(3600, true, AUTO_STEPS, (1, 3600)), 3600);
    }

    #[test]
    fn stepped_returns_none_at_the_limits() {
        let config = Config {
            alarm_threshold: config::ALARM_THRESHOLD.1,
            interval_ms: config::INTERVAL_MS.0,
            ..Config::default()
        };
        assert_eq!(Control::Threshold.stepped(&config, true), None);
        assert_eq!(Control::Interval.stepped(&config, false), None);
        let next = Control::Interval.stepped(&config, true).unwrap();
        assert_eq!(next.interval_ms, 500);
    }

    #[test]
    fn interval_steps_coarser_above_two_seconds() {
        let config = Config {
            interval_ms: 2000,
            ..Config::default()
        };
        assert_eq!(
            Control::Interval
                .stepped(&config, true)
                .unwrap()
                .interval_ms,
            3000
        );
        assert_eq!(
            Control::Interval
                .stepped(&config, false)
                .unwrap()
                .interval_ms,
            1750
        );
    }

    #[test]
    fn focus_goes_through_the_sidebar_then_the_page() {
        let config = Config::default();
        let order = focus_order(Page::Display, &config);
        assert_eq!(order[0], Control::Nav(Page::Overview));
        assert_eq!(order[PAGES.len()], Control::Mode);
        // Hidden settings are skipped: no alternation, so no "Switch every".
        assert!(!order.contains(&Control::AutoInterval));
        let all = |_| true;
        assert_eq!(
            next_focus(&order, Some(Control::Mode), true, all),
            Some(Control::Source)
        );
        assert_eq!(
            next_focus(&order, Some(Control::Interval), true, all),
            Some(Control::Nav(Page::Overview))
        );
        assert_eq!(
            next_focus(&order, None, false, all),
            Some(Control::Interval)
        );
        // Offline, only the sidebar is reachable.
        let online = |c: Control| c.enabled(&config, false);
        assert_eq!(
            next_focus(&order, Some(Control::Nav(Page::General)), true, online),
            Some(Control::Nav(Page::Overview))
        );
        assert_eq!(next_focus(&order, None, true, |_| false), None);
    }

    #[test]
    fn settings_show_only_when_they_matter() {
        let config = Config::default();
        assert!(!Control::AutoInterval.visible(&config));
        assert!(!Control::NightStart.visible(&config));
        assert!(Control::Threshold.visible(&config));
        let auto = Control::Source.with_segment(&config, 2);
        assert!(Control::AutoInterval.visible(&auto));
        let night = Control::OffNight.toggled(&config).unwrap();
        assert!(Control::NightStart.visible(&night) && Control::NightEnd.visible(&night));
        let quiet = Control::Alarm.toggled(&config).unwrap();
        assert!(!Control::Threshold.visible(&quiet));
    }

    #[test]
    fn switches_flip_their_setting() {
        let config = Config::default();
        let usage = Control::Bar.toggled(&config).unwrap();
        assert_eq!(usage.bar, Bar::Usage);
        assert!(Control::Bar.is_on(&usage));
        assert!(Control::OffLocked.toggled(&config).unwrap().off_when_locked);
        assert!(
            Control::OffScreen
                .toggled(&config)
                .unwrap()
                .off_when_screen_off
        );
        assert_eq!(Control::Mode.toggled(&config), None);
    }

    #[test]
    fn night_times_step_by_quarters_and_wrap() {
        let t = ClockTime::new;
        assert_eq!(step_time(t(23, 0), true), t(23, 15));
        assert_eq!(step_time(t(23, 45), true), t(0, 0));
        assert_eq!(step_time(t(0, 0), false), t(23, 45));
        assert_eq!(step_time(t(7, 10), false), t(7, 0));
        assert_eq!(step_time(t(7, 10), true), t(7, 15));
    }

    #[test]
    fn custom_segments() {
        let config = Config::default();
        let custom = Control::Mode.with_segment(&config, 3);
        assert_eq!(custom.mode, Mode::Custom);
        let percent = Control::CustomSymbol.with_segment(&custom, 2);
        assert_eq!(percent.custom_symbol, Symbol::Percent);
        assert_eq!(Control::CustomSymbol.selected_segment(&percent), 2);
        let smart = Control::Source.with_segment(&config, 3);
        assert_eq!(smart.source, Source::Smart);
    }

    #[test]
    fn custom_steppers_stay_in_range() {
        let config = Config {
            custom_value: 999,
            custom_bar: 1,
            ..Config::default()
        };
        assert_eq!(Control::CustomValue.stepped(&config, true), None);
        assert_eq!(Control::CustomBar.stepped(&config, false), None);
        assert_eq!(
            Control::CustomValue
                .stepped(&config, false)
                .unwrap()
                .custom_value,
            998
        );
        assert_eq!(
            Control::CustomBar
                .stepped(&config, true)
                .unwrap()
                .custom_bar,
            2
        );
    }

    #[test]
    fn typing_builds_up_to_three_digits() {
        assert_eq!(typed(42, '7', true), Some(7));
        assert_eq!(typed(4, '2', false), Some(42));
        assert_eq!(typed(42, '5', false), Some(425));
        assert_eq!(typed(425, '1', false), None);
        assert_eq!(typed(425, BACKSPACE, false), Some(42));
        assert_eq!(typed(425, BACKSPACE, true), Some(0));
        assert_eq!(typed(42, 'x', false), None);
    }

    #[test]
    fn controls_need_the_service() {
        let config = Config::default();
        assert!(!Control::Mode.enabled(&config, false));
        assert!(Control::Mode.enabled(&config, true));
        assert!(Control::Autostart.enabled(&config, false));
        assert!(Control::Update.enabled(&config, false));
        assert!(Control::Nav(Page::Alarm).enabled(&config, false));
        let auto = Control::Mode.with_segment(&config, 2);
        assert_eq!(auto.mode, Mode::Auto);
        assert_eq!(Control::Mode.selected_segment(&auto), 2);
    }

    #[test]
    fn settings_use_the_service_text_form() {
        let config = Config::default();
        assert_eq!(
            Control::Mode.setting(&config),
            Some(("mode", "temperature".into()))
        );
        assert_eq!(
            Control::Alarm.setting(&config),
            Some(("alarm", "true".into()))
        );
        assert_eq!(Control::Autostart.setting(&config), None);
        for page in PAGES {
            for &(c, ..) in page.settings() {
                if let Some((key, value)) = c.setting(&config) {
                    assert!(Config::default().set(key, &value).is_ok(), "{key}={value}");
                }
            }
        }
    }

    #[test]
    fn fade_dims_translucent_colors_too() {
        let alpha = |c: Color| c.0 >> 24;
        assert_eq!(alpha(fade(Color::rgb(1, 2, 3), false)), 0x60);
        assert_eq!(alpha(fade(Color::rgb(0, 0, 0).alpha(0x0C), false)), 0x04);
        assert_eq!(fade(Color::rgb(1, 2, 3), true), Color::rgb(1, 2, 3));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(format_seconds(5), "5 s");
        assert_eq!(format_seconds(120), "2 min");
        assert_eq!(format_seconds(90), "1 min 30 s");
        assert_eq!(format_ms(250), "0.25 s");
        assert_eq!(format_ms(1500), "1.5 s");
        assert_eq!(format_ms(1000), "1 s");
    }

    #[test]
    fn sidebar_reports_the_worst_problem() {
        assert_eq!(status_lines(None).0, Level::Error);
        let mut status = Status::default();
        assert_eq!(
            status_lines(Some(&status)),
            (
                Level::Warn,
                "Waiting for a cooler".to_owned(),
                "No cooler connected".to_owned()
            )
        );
        status.devices = vec!["AK400 DIGITAL".into()];
        assert_eq!(status_lines(Some(&status)).0, Level::Ok);
        status.temp_error = Some("PawnIO is not installed".into());
        assert_eq!(status_lines(Some(&status)).0, Level::Warn);
    }

    #[test]
    fn showing_says_what_the_display_shows() {
        let mut status = Status {
            devices: vec!["AK400 DIGITAL".into()],
            shown: Some(Shown::Usage),
            component: Some(Component::Gpu),
            ..Status::default()
        };
        assert_eq!(showing(Some(&status)), "Showing the GPU usage");
        status.display_off = Some(DisplayOff::Night);
        assert_eq!(showing(Some(&status)), "Display off for the night");
        assert_eq!(showing(None), "Not connected to the service");
    }
}
