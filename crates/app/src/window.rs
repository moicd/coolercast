//! Settings window, custom drawn with GDI+ in a liquid glass style: a floating sidebar whose
//! selection slides between pages, an overview page with a live preview of the cooler display and
//! the recent history, and pages of rounded setting cards.
//!
//! On Windows 11 (22H2 and later) the window uses the Acrylic system backdrop and draws its
//! surfaces as translucent glass on top; elsewhere it falls back to opaque colors. The blur is
//! composed by Windows, so it costs this process nothing. Motion follows the Windows animation
//! effects setting, and the window only repaints while something moves.
//!
//! The window only exists while it is open; closing it releases GDI+ and every drawing resource,
//! so the tray icon alone stays as small as before.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::sync::Mutex;
use std::time::Instant;
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
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
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

use crate::anim::{Span, Tween};
use crate::autostart;
use crate::gfx::{Align, Canvas, Color, Gdiplus, Rect, Weight, pixel_bitmap};
use crate::i18n::{self, LANGS, Lang, Strings, fill};
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
/// The sidebar floats as a glass panel this far from the window edges.
const INSET: f32 = 12.0;
const SIDEBAR_W: f32 = 220.0;
const PAGE_X: f32 = INSET + SIDEBAR_W + 20.0;
const PAGE_W: f32 = WIDTH - PAGE_X - 24.0;
const PAGE_TOP: f32 = 84.0;
const NAV_TOP: f32 = 116.0;
const NAV_H: f32 = 40.0;
const SETTING_H: f32 = 64.0;
/// Each extra line of a wrapped description.
const LINE_H: f32 = 16.0;
const SETTING_GAP: f32 = 8.0;
const CARD_RADIUS: f32 = 18.0;
/// Height of the capsule controls.
const CONTROL_H: f32 = 32.0;
const SWITCH_W: f32 = 46.0;
const SWITCH_H: f32 = 28.0;
const STEPPER_W: f32 = 136.0;
const MENU_ITEM_H: f32 = 32.0;
const MENU_PAD: f32 = 6.0;

// Glyphs of the Windows icon font.
const CHEVRON_DOWN: &str = "\u{E70D}";
const CHECK_MARK: &str = "\u{E73E}";

/// Stepper changes are sent once the value stops changing for this long.
const TIMER_COMMIT: usize = 1;
const COMMIT_DELAY_MS: u32 = 400;
/// Holding a stepper button repeats it.
const TIMER_REPEAT: usize = 2;
const REPEAT_DELAY_MS: u32 = 400;
const REPEAT_RATE_MS: u32 = 60;
/// Repaints while something animates: about one frame.
const TIMER_ANIM: usize = 3;
const ANIM_FRAME_MS: u32 = 10;
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
    fn title(self, s: &Strings) -> &'static str {
        match self {
            Page::Overview => s.overview,
            Page::Display => s.display,
            Page::Custom => s.custom_value,
            Page::TurnOff => s.display_off,
            Page::Alarm => s.alarm,
            Page::General => s.general,
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
    fn subtitle(self, s: &Strings) -> &'static str {
        match self {
            Page::Overview => s.overview_about,
            Page::Display => s.display_about,
            Page::Custom => s.custom_about,
            Page::TurnOff => s.display_off_about,
            Page::Alarm => s.alarm_about,
            Page::General => s.general_about,
        }
    }

    /// The setting cards of the page, in focus order.
    fn settings(self) -> &'static [Control] {
        match self {
            Page::Overview => &[],
            Page::Display => &[
                Control::Mode,
                Control::Source,
                Control::AutoInterval,
                Control::Unit,
                Control::Bar,
                Control::Interval,
            ],
            Page::Custom => &[
                Control::UseCustom,
                Control::CustomValue,
                Control::CustomSymbol,
                Control::CustomBar,
            ],
            Page::TurnOff => &[
                Control::OffLocked,
                Control::OffScreen,
                Control::OffNight,
                Control::NightStart,
                Control::NightEnd,
            ],
            Page::Alarm => &[Control::Alarm, Control::Threshold],
            Page::General => &[Control::Language, Control::Autostart, Control::Update],
        }
    }
}

/// The interactive controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Nav(Page),
    Language,
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
                .copied()
                .filter(|c| c.visible(config)),
        )
        .collect()
}

impl Control {
    /// Whether the control can be used: the service settings need the service.
    fn enabled(self, _config: &Config, online: bool) -> bool {
        self.local() || online
    }

    /// Controls that work without the service: the window and the app settings.
    fn local(self) -> bool {
        matches!(
            self,
            Control::Nav(_) | Control::Language | Control::Autostart | Control::Update
        )
    }

    /// Title and description of the setting card.
    fn label(self, s: &Strings) -> (Cow<'static, str>, &'static str) {
        let (title, about) = match self {
            Control::Mode => (s.show, s.show_about),
            Control::Source => (s.device, s.device_about),
            Control::AutoInterval => (s.switch_every, s.switch_every_about),
            Control::Unit => (s.unit, s.unit_about),
            Control::Bar => (s.usage_bar, s.usage_bar_about),
            Control::Interval => (s.refresh_every, s.refresh_every_about),
            Control::UseCustom => (s.not_shown, s.not_shown_about),
            Control::CustomValue => (s.number, s.number_about),
            Control::CustomSymbol => (s.symbol, s.symbol_about),
            Control::CustomBar => (s.bar, s.bar_about),
            Control::OffLocked => (s.when_locked, s.when_locked_about),
            Control::OffScreen => (s.when_screen_off, s.when_screen_off_about),
            Control::OffNight => (s.at_night, s.at_night_about),
            Control::NightStart => (s.from, s.local_time),
            Control::NightEnd => (s.until, s.local_time),
            Control::Alarm => (s.blink, s.blink_about),
            Control::Threshold => (s.threshold, s.threshold_about),
            Control::Language => (s.language, s.language_about),
            Control::Autostart => (s.start_with_windows, s.start_with_windows_about),
            Control::Update => {
                let version = fill(s.version, &[env!("CARGO_PKG_VERSION")]);
                return (Cow::Owned(version), s.version_about);
            }
            Control::Nav(page) => (page.title(s), ""),
        };
        (Cow::Borrowed(title), about)
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

    fn segments(self, s: &Strings) -> Vec<&'static str> {
        match self {
            Control::Mode => vec![s.temperature, s.usage, s.alternate, s.custom],
            Control::Source => vec!["CPU", "GPU", s.alternate, s.smart],
            Control::Unit => vec!["°C", "°F"],
            Control::CustomSymbol => vec!["°C", "°F", "%"],
            _ => Vec::new(),
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
            Control::Nav(_)
            | Control::Language
            | Control::Autostart
            | Control::Update
            | Control::UseCustom => return None,
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

fn format_seconds(t: &Strings, secs: u32) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => fill(t.seconds, &[&s.to_string()]),
        (m, 0) => fill(t.minutes, &[&m.to_string()]),
        (m, s) => fill(t.minutes_seconds, &[&m.to_string(), &s.to_string()]),
    }
}

fn format_ms(t: &Strings, ms: u32) -> String {
    let s = ms as f32 / 1000.0;
    let number = if ms.is_multiple_of(1000) {
        (ms / 1000).to_string()
    } else if ms.is_multiple_of(100) {
        format!("{s:.1}")
    } else {
        format!("{s:.2}")
    };
    fill(t.seconds, &[&number.replace('.', t.decimal)])
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
fn status_lines(status: Option<&Status>, t: &Strings) -> (Level, String, String) {
    let Some(s) = status else {
        return (
            Level::Error,
            t.service_down.into(),
            t.service_down_detail.into(),
        );
    };
    let devices = if s.devices.is_empty() {
        t.no_cooler.to_owned()
    } else {
        s.devices.join(", ")
    };
    match (&s.temp_error, s.devices.is_empty()) {
        (Some(error), _) => (
            Level::Warn,
            t.temperature_unavailable.into(),
            error.split(';').next().unwrap_or(error).to_owned(),
        ),
        (None, true) => (Level::Warn, t.waiting_for_cooler.into(), devices),
        (None, false) => (Level::Ok, t.running.into(), devices),
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
fn showing(status: Option<&Status>, t: &Strings) -> String {
    let Some(s) = status else {
        return t.not_connected.into();
    };
    if let Some(off) = s.display_off {
        return match off {
            DisplayOff::Locked => t.off_while_locked,
            DisplayOff::ScreenOff => t.off_while_screen_off,
            DisplayOff::Night => t.off_for_the_night,
        }
        .into();
    }
    if s.devices.is_empty() {
        return t.no_cooler.into();
    }
    let device = if s.component == Some(Component::Gpu) {
        "GPU"
    } else {
        "CPU"
    };
    let shown = match s.shown {
        Some(Shown::Temperature) => fill(t.showing_temperature, &[device]),
        Some(Shown::Usage) => fill(t.showing_usage, &[device]),
        Some(Shown::Power) => fill(t.showing_power, &[device]),
        Some(Shown::Custom) => t.showing_custom.into(),
        None => t.starting.into(),
    };
    if s.alarm_active {
        format!("{shown} · {}", t.too_hot)
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
    /// `None` follows the Windows language.
    Language(Option<Lang>),
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
    fn label(&self, t: &Strings) -> String {
        match self {
            UpdateState::Idle => t.check_updates.into(),
            UpdateState::Checking => t.checking.into(),
            UpdateState::Done(Check::UpToDate) => fill(t.up_to_date, &[env!("CARGO_PKG_VERSION")]),
            UpdateState::Done(Check::Available { version, .. }) => fill(t.download, &[version]),
            UpdateState::Done(Check::Failed(_)) => t.check_failed.into(),
        }
    }
}

/// The entries of the language list: the Windows language, then every translation.
fn language_choices() -> impl Iterator<Item = Option<Lang>> {
    std::iter::once(None).chain(LANGS.into_iter().map(Some))
}

/// How a language choice reads in the list and on its button.
fn language_label(choice: Option<Lang>, t: &Strings) -> String {
    match choice {
        Some(lang) => lang.name().into(),
        None => format!("{} ({})", t.windows_language, i18n::system().name()),
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
    /// The open language list and its highlighted entry.
    menu: Option<usize>,
    /// The entries of the open list, from the last paint.
    menu_hits: Vec<(usize, Rect)>,
    /// Whether things glide (Windows animation effects).
    motion: bool,
    /// When the frame being painted happens.
    now: Instant,
    /// Something is still moving: keep repainting.
    animating: bool,
    /// The sidebar pills: the selected page, and the item under the mouse with its opacity.
    nav_pill: Option<Span>,
    hover_pill: Option<Span>,
    hover_alpha: Tween,
    /// Per control: the sliding thumb of segmented controls, the knob of switches (0 off, 1 on)
    /// and the hover glow of setting cards.
    thumbs: Vec<(Control, Span)>,
    knobs: Vec<(Control, Tween)>,
    glows: Vec<(Control, Tween)>,
    /// The highlight of the language list.
    menu_pill: Option<Span>,
    /// Setting cards from the last paint, and the one under the mouse.
    rows: Vec<(Control, Rect)>,
    hover_row: Option<Control>,
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
        menu: None,
        menu_hits: Vec::new(),
        motion: theme::animations_enabled(),
        now: Instant::now(),
        animating: false,
        nav_pill: None,
        hover_pill: None,
        hover_alpha: Tween::new(0.0, Instant::now()),
        thumbs: Vec::new(),
        knobs: Vec::new(),
        glows: Vec::new(),
        menu_pill: None,
        rows: Vec::new(),
        hover_row: None,
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
        Effect::Language(choice) => {
            i18n::choose(choice);
            // Repaints the window too, and the tray tooltip follows the new language.
            crate::tray::refresh(false);
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
                if s.hover.take().is_some() | s.hover_row.take().is_some() {
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
                // Escape closes the language list first, then the window.
                if !with(Settings::close_menu).unwrap_or(false) {
                    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
                }
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
                    KillTimer(hwnd, TIMER_ANIM);
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
        // The open list takes the mouse for itself.
        if self.menu.is_some() {
            if let Some(item) = self.menu_hit(x, y)
                && self.menu != Some(item)
            {
                self.menu = Some(item);
                self.invalidate();
            }
            return;
        }
        let hover = self.hit_test(x, y);
        let row = self
            .rows
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|&(c, _)| c);
        if hover != self.hover || row != self.hover_row {
            self.hover = hover;
            self.hover_row = row;
            self.invalidate();
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32, shift: bool) -> Effect {
        if self.menu.is_some() {
            // A click outside the list closes it; a click on an entry picks it on release.
            if self.menu_hit(x, y).is_none() {
                self.close_menu();
            }
            return Effect::None;
        }
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
        if self.menu.is_some() {
            return match self.menu_hit(x, y) {
                Some(item) => self.pick_language(item),
                None => Effect::None,
            };
        }
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
            // The next paint decides whether to keep going.
            TIMER_ANIM => {
                self.invalidate();
                Effect::None
            }
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
        if let Some(item) = self.menu {
            let last = language_choices().count() - 1;
            match key {
                VK_UP => self.menu = Some(item.saturating_sub(1)),
                VK_DOWN => self.menu = Some((item + 1).min(last)),
                VK_RETURN | VK_SPACE => return self.pick_language(item),
                VK_TAB => {
                    self.close_menu();
                }
                _ => {}
            }
            return Effect::None;
        }
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
                    Control::Update | Control::UseCustom | Control::Language => Part::Button,
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
                let count = control.segments(i18n::text()).len();
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
        if self.menu.is_some() {
            return;
        }
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

    /// The language list entry at a point.
    fn menu_hit(&self, x: f32, y: f32) -> Option<usize> {
        self.menu_hits
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|&(i, _)| i)
    }

    /// Closes the language list; `false` if it was not open.
    fn close_menu(&mut self) -> bool {
        let open = self.menu.take().is_some();
        if open {
            self.menu_hits.clear();
            self.menu_pill = None;
            self.invalidate();
        }
        open
    }

    fn pick_language(&mut self, item: usize) -> Effect {
        self.close_menu();
        Effect::Language(language_choices().nth(item).unwrap_or(None))
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
            (Control::Language, _) => {
                let chosen = i18n::choice();
                self.menu = Some(language_choices().position(|c| c == chosen).unwrap_or(0));
                self.hover = None;
                self.invalidate();
                Effect::None
            }
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
        let tx = i18n::text();
        self.now = Instant::now();
        self.animating = false;
        c.clear(t.backdrop);
        self.hits.clear();
        self.menu_hits.clear();
        self.rows.clear();
        self.draw_sidebar(c);

        let page = self.page;
        c.text(
            page.title(tx),
            Rect::new(PAGE_X, 22.0, PAGE_W, 36.0),
            26.0,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        c.text(
            page.subtitle(tx),
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
        if self.menu.is_some() {
            self.draw_menu(c);
        }
        unsafe {
            if self.animating {
                SetTimer(self.hwnd, TIMER_ANIM, ANIM_FRAME_MS, None);
            } else {
                KillTimer(self.hwnd, TIMER_ANIM);
            }
        }
    }

    /// Moves `pill` toward `target` and returns where it is in this frame.
    fn glide(&mut self, pill: Option<Span>, target: (f32, f32)) -> (Span, (f32, f32)) {
        let now = self.now;
        let mut pill = pill.unwrap_or_else(|| Span::new(target, now));
        pill.go(target, self.motion, now);
        self.animating |= pill.running(now);
        (pill, pill.value(now))
    }

    /// Moves the value kept for `key` in one of the per-control lists toward `target`.
    fn tween(
        &mut self,
        list: fn(&mut Self) -> &mut Vec<(Control, Tween)>,
        key: Control,
        target: f32,
        ms: f32,
    ) -> f32 {
        let (now, ms) = (self.now, if self.motion { ms } else { 0.0 });
        let tween = keyed(list(self), key, || Tween::new(target, now));
        tween.go(target, ms, now);
        let (value, running) = (tween.value(now), tween.running(now));
        self.animating |= running;
        value
    }

    /// A floating glass surface: soft shadow, fill fading downwards and a lit rim.
    fn draw_glass(&self, c: &Canvas, r: Rect, radius: f32, depth: f32) {
        let t = &self.theme;
        c.shadow(r, radius, depth, t.shadow);
        c.fill_round_rect_v(r, radius, t.card_top, t.card_bottom);
        c.stroke_round_rect_v(r, radius, 1.0, t.edge_top, t.edge_bottom);
    }

    fn draw_sidebar(&mut self, c: &Canvas) {
        let t = self.theme;
        let tx = i18n::text();
        let panel = Rect::new(INSET, INSET, SIDEBAR_W, HEIGHT - 2.0 * INSET);
        self.draw_glass(c, panel, 22.0, 14.0);
        draw_app_icon(c, Rect::new(28.0, 30.0, 40.0, 40.0));
        let text_x = 80.0;
        let text_w = panel.right() - text_x - 12.0;
        c.text(
            "CoolerCast",
            Rect::new(text_x, 28.0, text_w, 22.0),
            16.0,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        // The service state, where it is always visible.
        let (level, title, detail) = status_lines(self.status.as_ref(), tx);
        let dot = match level {
            Level::Ok => t.ok,
            Level::Warn => t.warn,
            Level::Error => t.alarm,
        };
        c.fill_circle(text_x + 4.0, 59.0, 4.0, dot);
        c.text(
            &title,
            Rect::new(text_x + 14.0, 50.0, text_w - 14.0, 18.0),
            12.5,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        c.text(
            &detail,
            Rect::new(text_x, 69.0, text_w, 18.0),
            12.0,
            Weight::Regular,
            t.text_dim,
            Align::Left,
        );

        let item = |i: usize| {
            Rect::new(
                panel.x + 10.0,
                NAV_TOP + i as f32 * (NAV_H + 2.0),
                panel.w - 20.0,
                NAV_H,
            )
        };
        let selected = PAGES.iter().position(|&p| p == self.page).unwrap_or(0);
        let hovered = PAGES.iter().position(|&p| {
            self.hover
                == Some(Hit {
                    control: Control::Nav(p),
                    part: Part::Button,
                })
        });

        // The hover pill glides between items and fades in and out; it appears in place.
        let now = self.now;
        let showing_hover = hovered.filter(|&i| i != selected);
        self.hover_alpha.go(
            if showing_hover.is_some() { 1.0 } else { 0.0 },
            if self.motion { 160.0 } else { 0.0 },
            now,
        );
        let alpha = self.hover_alpha.value(now).clamp(0.0, 1.0);
        self.animating |= self.hover_alpha.running(now);
        if let Some(i) = showing_hover {
            let r = item(i);
            let pill = if alpha < 0.05 { None } else { self.hover_pill };
            let (pill, _) = self.glide(pill, (r.y, r.bottom()));
            self.hover_pill = Some(pill);
        }
        if let Some(pill) = self.hover_pill
            && alpha > 0.01
        {
            let (top, bottom) = pill.value(now);
            let r = Rect::new(item(0).x, top, item(0).w, bottom - top);
            c.fill_round_rect(r, NAV_H / 2.0, faded(t.hover, alpha));
        }

        // The selection pill slides to the chosen page, stretching on the way.
        let target = item(selected);
        let (pill, (top, bottom)) = self.glide(self.nav_pill, (target.y, target.bottom()));
        self.nav_pill = Some(pill);
        let pill = Rect::new(target.x, top, target.w, bottom - top);
        let radius = NAV_H / 2.0;
        c.fill_round_rect(pill, radius, t.selection);
        self.gloss(c, pill, radius);
        c.stroke_round_rect_v(pill, radius, 1.0, t.edge_top, t.edge_top.alpha(0));

        for (i, &page) in PAGES.iter().enumerate() {
            let r = item(i);
            let hit = Hit {
                control: Control::Nav(page),
                part: Part::Button,
            };
            let on = i == selected;
            // High contrast fills the selection with the highlight color.
            let text = if on && t.selection.0 >> 24 == 0xFF {
                t.on_thumb
            } else {
                t.text
            };
            c.text(
                page.icon(),
                Rect::new(r.x + 14.0, r.y, 20.0, r.h),
                16.0,
                Weight::Icon,
                if on && text == t.text { t.accent } else { text },
                Align::Center,
            );
            c.text(
                page.title(tx),
                Rect::new(r.x + 46.0, r.y, r.w - 52.0, r.h),
                14.0,
                if on {
                    Weight::Semibold
                } else {
                    Weight::Regular
                },
                text,
                Align::Left,
            );
            if self.focus_visible && self.focus == Some(hit.control) {
                c.stroke_round_rect(r.inset(-2.0, -2.0), radius + 2.0, 2.0, t.focus);
            }
            self.hits.push((hit, r));
        }
    }

    /// The overview: the display (or every value, for displays that show several), what it
    /// shows, and the history.
    fn draw_overview(&mut self, c: &Canvas) {
        let hero = Rect::new(PAGE_X, PAGE_TOP, PAGE_W, 214.0);
        self.draw_card(c, hero);
        self.draw_readout(c, hero);
        let chart = Rect::new(
            PAGE_X,
            hero.bottom() + 14.0,
            PAGE_W,
            HEIGHT - hero.bottom() - 14.0 - 2.0 * INSET,
        );
        self.draw_chart(c, chart);
    }

    /// A page of setting cards; a card grows when its description needs a second line.
    fn draw_settings(&mut self, c: &Canvas, page: Page) {
        let tx = i18n::text();
        let mut y = PAGE_TOP;
        for &control in page.settings() {
            if !control.visible(&self.config) {
                continue;
            }
            let text_w = PAGE_W - 36.0 - 18.0 - self.control_width(c, control);
            let (_, about) = control.label(tx);
            let lines = c.lines(about, 12.0, Weight::Regular, text_w).clamp(1, 2);
            let row = Rect::new(PAGE_X, y, PAGE_W, SETTING_H + (lines - 1) as f32 * LINE_H);
            self.draw_setting(c, row, control, text_w);
            y = row.bottom() + SETTING_GAP;
        }
    }

    fn draw_card(&self, c: &Canvas, r: Rect) {
        self.draw_glass(c, r, CARD_RADIUS, 10.0);
    }

    /// The glassy sheen on raised and accent-filled controls: a highlight fading out over the
    /// top half.
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
        let tx = i18n::text();
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
                &showing(status, tx),
                Rect::new(area.x, area.bottom() + 6.0, area.w, 18.0),
                12.0,
                Weight::Regular,
                t.text_dim,
                Align::Center,
            );
            values_x = area.right() + 28.0;
        } else if status.is_some() {
            c.text(
                &showing(status, tx),
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
                tx.cpu_temperature,
                temp(status.and_then(|s| s.cpu_temp)),
                if alarm { t.alarm } else { t.text },
            ),
            (
                tx.cpu_usage,
                percent(status.and_then(|s| s.cpu_usage)),
                t.text,
            ),
        ];
        if let Some(s) = status.filter(|s| s.gpu.temp.is_some() || s.gpu.usage.is_some()) {
            values.push((tx.gpu_temperature, temp(s.gpu.temp), t.text));
            values.push((tx.gpu_usage, percent(s.gpu.usage), t.text));
        }
        if let Some(w) = status.and_then(|s| s.cpu_power) {
            values.push((tx.cpu_power, format!("{w:.0} W"), t.text));
        }
        if let Some(f) = status.and_then(|s| s.cpu_freq) {
            values.push((tx.cpu_clock, format!("{f:.0} MHz"), t.text));
        }
        // With only the CPU measured, the peaks of the chart are worth a glance too.
        if values.len() == 2
            && let Some(s) = status.filter(|s| !s.history.is_empty())
        {
            let peak_temp = s.history.iter().filter_map(|h| h.cpu_temp).reduce(f32::max);
            let peak_usage = s.history.iter().map(|h| h.cpu_usage).reduce(f32::max);
            values.push((tx.highest_temperature, temp(peak_temp), t.text));
            values.push((tx.highest_usage, percent(peak_usage), t.text));
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
        let tx = i18n::text();
        self.draw_card(c, card);
        let span = HISTORY_LEN as u32 * self.config.interval_ms / 1000;
        let span_text = format_seconds(tx, span);
        c.text(
            &fill(tx.last, &[&span_text]),
            Rect::new(card.x + 16.0, card.y + 12.0, 200.0, 20.0),
            13.0,
            Weight::Semibold,
            t.text,
            Align::Left,
        );

        // Legend, right-aligned.
        let mut lx = card.right() - 16.0;
        for (label, color) in [(tx.usage, t.usage_line), (tx.temperature, t.temp_line)] {
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
            &format!("-{span_text}"),
            Rect::new(plot.x, plot.bottom() + 6.0, 80.0, 16.0),
            11.0,
            Weight::Regular,
            t.text_dim,
            Align::Left,
        );
        c.text(
            tx.now,
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
                tx.alarm,
                Rect::new(plot.right() - 120.0, y - 17.0, 116.0, 14.0),
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
                    tx.collecting_data
                } else {
                    tx.no_data
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
    fn draw_setting(&mut self, c: &Canvas, row: Rect, control: Control, text_w: f32) {
        let t = self.theme;
        let tx = i18n::text();
        let (title, description) = control.label(tx);
        self.draw_card(c, row);
        // The card under the mouse lights up a little.
        let lit = if self.hover_row == Some(control) {
            1.0
        } else {
            0.0
        };
        let glow = self.tween(|s| &mut s.glows, control, lit, 160.0);
        if glow > 0.01 {
            c.fill_round_rect(row, CARD_RADIUS, faded(t.hover, glow));
        }
        self.rows.push((control, row));

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
            Control::UseCustom => self.draw_button(c, control, tx.show_it, right, cy, enabled),
            Control::Language => {
                let label = language_label(i18n::choice(), tx);
                self.draw_dropdown(c, &label, right, cy)
            }
            Control::CustomValue => {
                let text = config.custom_value.to_string();
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::CustomBar => {
                let text = format!("{} / {}", config.custom_bar, config::CUSTOM_BAR.1);
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::AutoInterval => {
                let text = format_seconds(tx, config.auto_interval_s);
                self.draw_stepper(c, control, &text, right, cy, enabled)
            }
            Control::Interval => {
                let text = format_ms(tx, config.interval_ms);
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
        c.text(
            &title,
            Rect::new(row.x + 18.0, row.y + 12.0, text_w, 20.0),
            14.0,
            Weight::Regular,
            fade(t.text, enabled),
            Align::Left,
        );
        c.text_wrapped(
            description,
            Rect::new(row.x + 18.0, row.y + 34.0, text_w, row.h - 34.0 - 6.0),
            12.0,
            Weight::Regular,
            fade(t.text_dim, enabled),
        );
        if self.focus_visible && self.focus == Some(control) {
            self.focus_ring(c, area);
        }
    }

    /// The keyboard focus, as a capsule around the control.
    fn focus_ring(&self, c: &Canvas, r: Rect) {
        let r = r.inset(-3.0, -3.0);
        c.stroke_round_rect(r, r.h / 2.0, 2.0, self.theme.focus);
    }

    fn state(&self, hit: Hit) -> (bool, bool) {
        let hovered = self.hover == Some(hit);
        let pressed = self.pressed == Some(hit) && hovered;
        (hovered, pressed)
    }

    /// Width of a setting's control, as `draw_setting` draws it.
    fn control_width(&self, c: &Canvas, control: Control) -> f32 {
        let tx = i18n::text();
        match control {
            Control::Mode | Control::Source | Control::Unit | Control::CustomSymbol => {
                segment_widths(c, control, tx).iter().sum::<f32>() + 6.0
            }
            Control::Autostart
            | Control::Bar
            | Control::OffLocked
            | Control::OffScreen
            | Control::OffNight
            | Control::Alarm => SWITCH_W + 10.0 + switch_label_w(c, tx),
            Control::Update => update_w(c, &self.update, tx),
            Control::UseCustom => capsule_w(c, tx.show_it),
            Control::Language => dropdown_w(c, &language_label(i18n::choice(), tx)),
            Control::CustomValue
            | Control::CustomBar
            | Control::AutoInterval
            | Control::Interval
            | Control::Threshold
            | Control::NightStart
            | Control::NightEnd => STEPPER_W,
            Control::Nav(_) => 0.0,
        }
    }

    /// Options in a capsule, with a raised thumb that slides to the selected one.
    fn draw_segmented(
        &mut self,
        c: &Canvas,
        control: Control,
        right: f32,
        cy: f32,
        enabled: bool,
    ) -> Rect {
        let t = self.theme;
        let tx = i18n::text();
        let options = control.segments(tx);
        let widths = segment_widths(c, control, tx);
        let total = widths.iter().sum::<f32>() + 6.0;
        let track = Rect::new(right - total, cy - CONTROL_H / 2.0, total, CONTROL_H);
        c.fill_round_rect(track, CONTROL_H / 2.0, fade(t.control, enabled));
        let inner_h = CONTROL_H - 6.0;
        let radius = inner_h / 2.0;
        let mut rects = Vec::with_capacity(widths.len());
        let mut x = track.x + 3.0;
        for &w in &widths {
            rects.push(Rect::new(x, track.y + 3.0, w, inner_h));
            x += w;
        }

        let selected = control.selected_segment(&self.config);
        if let Some(&target) = rects.get(selected) {
            let (now, motion) = (self.now, self.motion);
            let extent = (target.x, target.right());
            let span = keyed(&mut self.thumbs, control, || Span::new(extent, now));
            span.go(extent, motion, now);
            let ((a, b), running) = (span.value(now), span.running(now));
            self.animating |= running;
            let thumb = Rect::new(a, target.y, b - a, target.h);
            if enabled {
                c.shadow(thumb, radius, 4.0, t.shadow);
            }
            c.fill_round_rect(thumb, radius, fade(t.thumb, enabled));
            if enabled {
                self.gloss(c, thumb, radius);
            }
            c.stroke_round_rect_v(
                thumb,
                radius,
                1.0,
                fade(t.edge_top, enabled),
                t.edge_top.alpha(0),
            );
        }
        for (i, (&option, &r)) in options.iter().zip(&rects).enumerate() {
            let hit = Hit {
                control,
                part: Part::Segment(i),
            };
            let (hovered, _) = self.state(hit);
            let on = i == selected;
            if hovered && !on {
                c.fill_round_rect(r, radius, t.hover);
            }
            let (color, weight) = if on {
                (t.on_thumb, Weight::Semibold)
            } else {
                (t.text, Weight::Regular)
            };
            c.text(option, r, 12.5, weight, fade(color, enabled), Align::Center);
            self.hits.push((hit, r));
        }
        track
    }

    /// A switch whose knob slides across while the track fills with the accent.
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
        let tx = i18n::text();
        let hit = Hit {
            control,
            part: Part::Switch,
        };
        let (hovered, _) = self.state(hit);
        let k = self.tween(|s| &mut s.knobs, control, f32::from(u8::from(on)), 260.0);
        let fill_k = k.clamp(0.0, 1.0);
        let track = Rect::new(right - SWITCH_W, cy - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
        let radius = SWITCH_H / 2.0;
        c.fill_round_rect(
            track,
            radius,
            fade(t.control.mix(t.accent, fill_k), enabled),
        );
        if fill_k < 1.0 {
            let outline = faded(t.border, 1.0 - fill_k);
            c.stroke_round_rect(track, radius, 1.0, fade(outline, enabled));
        }
        if enabled && fill_k > 0.5 {
            self.gloss(c, track, radius);
        }
        if hovered && enabled {
            c.fill_round_rect(track, radius, t.hover);
        }
        let knob_r = radius - 3.0;
        let cx = track.x + radius + k * (SWITCH_W - SWITCH_H);
        let knob = Rect::new(cx - knob_r, cy - knob_r, 2.0 * knob_r, 2.0 * knob_r);
        if enabled {
            c.shadow(knob, knob_r, 3.0, t.shadow);
        }
        c.fill_circle(cx, cy, knob_r, fade(t.knob, enabled));
        c.stroke_round_rect(knob, knob_r, 1.0, fade(faded(t.border, 0.8), enabled));

        let label_w = switch_label_w(c, tx);
        let label = Rect::new(track.x - 10.0 - label_w, cy - 10.0, label_w, 20.0);
        c.text(
            if on { tx.on } else { tx.off },
            label,
            12.5,
            Weight::Regular,
            fade(t.text_dim, enabled),
            Align::Right,
        );
        // The label is clickable too, as in the Windows settings.
        let area = Rect::new(label.x, track.y, track.right() - label.x, track.h);
        self.hits.push((hit, area.inset(0.0, -4.0)));
        track
    }

    /// A capsule push button, right-aligned at `right`.
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
        let w = capsule_w(c, label);
        let r = Rect::new(right - w, cy - CONTROL_H / 2.0, w, CONTROL_H);
        let hit = Hit {
            control,
            part: Part::Button,
        };
        let (hovered, pressed) = self.state(hit);
        self.draw_capsule(c, r, hovered && enabled, pressed, enabled);
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

    /// The raised capsule of buttons.
    fn draw_capsule(&self, c: &Canvas, r: Rect, hovered: bool, pressed: bool, enabled: bool) {
        let t = &self.theme;
        let radius = r.h / 2.0;
        c.fill_round_rect(r, radius, fade(t.control, enabled));
        if pressed {
            c.fill_round_rect(r, radius, t.control_hover);
        } else if hovered {
            c.fill_round_rect(r, radius, t.hover);
        }
        c.stroke_round_rect_v(
            r,
            radius,
            1.0,
            fade(t.edge_top, enabled),
            fade(t.border, enabled),
        );
    }

    /// The update button, right-aligned at `right`; filled with the accent when a new version
    /// is waiting.
    fn draw_update_button(&mut self, c: &Canvas, right: f32, cy: f32) -> Rect {
        let t = self.theme;
        let label = self.update.label(i18n::text());
        let available = matches!(self.update, UpdateState::Done(Check::Available { .. }));
        let w = update_w(c, &self.update, i18n::text());
        let r = Rect::new(right - w, cy - CONTROL_H / 2.0, w, CONTROL_H);
        let hit = Hit {
            control: Control::Update,
            part: Part::Button,
        };
        let (hovered, pressed) = self.state(hit);
        let busy = self.update == UpdateState::Checking;
        let (weight, text) = if available {
            let radius = r.h / 2.0;
            c.fill_round_rect(r, radius, t.accent);
            self.gloss(c, r, radius);
            if hovered {
                c.fill_round_rect(r, radius, t.hover);
            }
            (Weight::Semibold, t.on_accent)
        } else {
            self.draw_capsule(c, r, hovered && !busy, pressed, true);
            (Weight::Regular, if busy { t.text_dim } else { t.text })
        };
        c.text(&label, r, 12.5, weight, text, Align::Center);
        if self.focus_visible && self.focus == Some(Control::Update) {
            self.focus_ring(c, r);
        }
        self.hits.push((hit, r));
        r
    }

    /// A capsule that opens a list, right-aligned at `right`.
    fn draw_dropdown(&mut self, c: &Canvas, label: &str, right: f32, cy: f32) -> Rect {
        let t = self.theme;
        let w = dropdown_w(c, label);
        let r = Rect::new(right - w, cy - CONTROL_H / 2.0, w, CONTROL_H);
        let hit = Hit {
            control: Control::Language,
            part: Part::Button,
        };
        let (hovered, pressed) = self.state(hit);
        self.draw_capsule(c, r, hovered, pressed || self.menu.is_some(), true);
        c.text(
            label,
            Rect::new(r.x + 16.0, r.y, r.w - 46.0, r.h),
            13.0,
            Weight::Regular,
            t.text,
            Align::Left,
        );
        c.text(
            CHEVRON_DOWN,
            Rect::new(r.right() - 32.0, r.y, 18.0, r.h),
            10.0,
            Weight::Icon,
            t.text_dim,
            Align::Center,
        );
        self.hits.push((hit, r));
        r
    }

    /// The open language list, a glass panel under its button.
    fn draw_menu(&mut self, c: &Canvas) {
        let Some(&(_, anchor)) = self
            .hits
            .iter()
            .find(|(hit, _)| hit.control == Control::Language)
        else {
            self.menu = None;
            return;
        };
        let t = self.theme;
        let tx = i18n::text();
        let chosen = i18n::choice();
        let items: Vec<(Option<Lang>, String)> = language_choices()
            .map(|choice| (choice, language_label(choice, tx)))
            .collect();
        let text_w = items
            .iter()
            .map(|(_, label)| c.measure(label, 13.0, Weight::Regular))
            .fold(0.0, f32::max);
        let w = (text_w + 64.0).max(anchor.w).round();
        let h = items.len() as f32 * MENU_ITEM_H + 2.0 * MENU_PAD;
        let x = anchor.right() - w;
        let y = (anchor.bottom() + 6.0).min(HEIGHT - INSET - h).max(INSET);
        let panel = Rect::new(x, y, w, h);
        let radius = 16.0;
        // Nearly opaque, so the cards behind it do not get in the way of reading.
        c.shadow(panel, radius, 18.0, t.shadow);
        c.fill_round_rect(panel, radius, t.background.alpha(0xF0));
        c.fill_round_rect_v(panel, radius, t.card_top, t.card_bottom);
        c.stroke_round_rect_v(panel, radius, 1.0, t.edge_top, t.edge_bottom);
        let item = |i: usize| {
            Rect::new(
                x + MENU_PAD,
                y + MENU_PAD + i as f32 * MENU_ITEM_H,
                w - 2.0 * MENU_PAD,
                MENU_ITEM_H,
            )
        };
        if let Some(i) = self.menu {
            let r = item(i);
            let (pill, (top, bottom)) = self.glide(self.menu_pill, (r.y, r.bottom()));
            self.menu_pill = Some(pill);
            let pill = Rect::new(r.x, top, r.w, bottom - top);
            c.fill_round_rect(pill, MENU_ITEM_H / 2.0, t.control_hover);
        }
        for (i, (choice, label)) in items.iter().enumerate() {
            let r = item(i);
            if *choice == chosen {
                c.text(
                    CHECK_MARK,
                    Rect::new(r.x + 10.0, r.y, 18.0, r.h),
                    12.0,
                    Weight::Icon,
                    t.accent,
                    Align::Center,
                );
            }
            c.text(
                label,
                Rect::new(r.x + 36.0, r.y, r.w - 44.0, r.h),
                13.0,
                Weight::Regular,
                t.text,
                Align::Left,
            );
            self.menu_hits.push((i, r));
        }
    }

    /// A number between round − and + buttons, in one capsule.
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
        let area = Rect::new(
            right - STEPPER_W,
            cy - CONTROL_H / 2.0,
            STEPPER_W,
            CONTROL_H,
        );
        c.fill_round_rect(area, CONTROL_H / 2.0, fade(t.control, enabled));
        let d = CONTROL_H - 6.0;
        let buttons = [
            (Part::Minus, Rect::new(area.x + 3.0, area.y + 3.0, d, d)),
            (
                Part::Plus,
                Rect::new(area.right() - 3.0 - d, area.y + 3.0, d, d),
            ),
        ];
        for (part, r) in buttons {
            let hit = Hit { control, part };
            let (hovered, pressed) = self.state(hit);
            let can_step = control.stepped(&self.config, part == Part::Plus).is_some();
            let active = enabled && can_step;
            if active {
                c.shadow(r, d / 2.0, 3.0, t.shadow);
            }
            c.fill_round_rect(r, d / 2.0, fade(t.thumb, active));
            if pressed {
                c.fill_round_rect(r, d / 2.0, t.control_hover);
            } else if hovered && active {
                c.fill_round_rect(r, d / 2.0, t.hover);
            }
            let (mx, my) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let glyph = fade(t.on_thumb, active);
            c.line(mx - 5.0, my, mx + 5.0, my, 1.5, glyph);
            if part == Part::Plus {
                c.line(mx, my - 5.0, mx, my + 5.0, 1.5, glyph);
            }
            if can_step {
                self.hits.push((hit, r));
            }
        }
        let field = Rect::new(area.x + d + 6.0, area.y, area.w - 2.0 * (d + 6.0), area.h);
        if control == Control::CustomValue {
            // A text box look: this number also takes typed digits.
            let hovered = self
                .state(Hit {
                    control,
                    part: Part::Field,
                })
                .0;
            let inner = field.inset(0.0, 3.0);
            if hovered && enabled {
                c.fill_round_rect(inner, inner.h / 2.0, t.hover);
            }
            if enabled && self.focus == Some(control) {
                c.stroke_round_rect(inner, inner.h / 2.0, 1.5, t.accent);
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

/// Width of each option of a segmented control.
fn segment_widths(c: &Canvas, control: Control, tx: &Strings) -> Vec<f32> {
    control
        .segments(tx)
        .iter()
        .map(|o| {
            (c.measure(o, 12.5, Weight::Semibold) + 28.0)
                .max(48.0)
                .round()
        })
        .collect()
}

/// Room for the longer of "On" and "Off", so a switch does not move when it flips.
fn switch_label_w(c: &Canvas, tx: &Strings) -> f32 {
    c.measure(tx.on, 12.5, Weight::Regular)
        .max(c.measure(tx.off, 12.5, Weight::Regular))
        .ceil()
}

fn capsule_w(c: &Canvas, label: &str) -> f32 {
    (c.measure(label, 13.0, Weight::Regular) + 36.0).round()
}

fn dropdown_w(c: &Canvas, label: &str) -> f32 {
    (c.measure(label, 13.0, Weight::Regular) + 56.0).round()
}

fn update_w(c: &Canvas, update: &UpdateState, tx: &Strings) -> f32 {
    let weight = if matches!(update, UpdateState::Done(Check::Available { .. })) {
        Weight::Semibold
    } else {
        Weight::Regular
    };
    (c.measure(&update.label(tx), 12.5, weight) + 32.0).round()
}

/// The entry for `key` in a small per-control list, created by `make` the first time.
fn keyed<T>(list: &mut Vec<(Control, T)>, key: Control, make: impl FnOnce() -> T) -> &mut T {
    let i = match list.iter().position(|(k, _)| *k == key) {
        Some(i) => i,
        None => {
            list.push((key, make()));
            list.len() - 1
        }
    };
    &mut list[i].1
}

/// `color` with its opacity scaled by `k` (0 to 1).
fn faded(color: Color, k: f32) -> Color {
    let alpha = ((color.0 >> 24) as f32 * k.clamp(0.0, 1.0)).round();
    color.alpha(alpha as u8)
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

    fn en() -> &'static Strings {
        Lang::En.strings()
    }

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
            for &c in page.settings() {
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
        assert_eq!(format_seconds(en(), 5), "5 s");
        assert_eq!(format_seconds(en(), 120), "2 min");
        assert_eq!(format_seconds(en(), 90), "1 min 30 s");
        assert_eq!(format_ms(en(), 250), "0.25 s");
        assert_eq!(format_ms(en(), 1500), "1.5 s");
        assert_eq!(format_ms(Lang::Es.strings(), 1500), "1,5 s");
        assert_eq!(format_ms(en(), 1000), "1 s");
    }

    #[test]
    fn sidebar_reports_the_worst_problem() {
        assert_eq!(status_lines(None, en()).0, Level::Error);
        let mut status = Status::default();
        assert_eq!(
            status_lines(Some(&status), en()),
            (
                Level::Warn,
                "Waiting for a cooler".to_owned(),
                "No cooler connected".to_owned()
            )
        );
        status.devices = vec!["AK400 DIGITAL".into()];
        assert_eq!(status_lines(Some(&status), en()).0, Level::Ok);
        status.temp_error = Some("PawnIO is not installed".into());
        assert_eq!(status_lines(Some(&status), en()).0, Level::Warn);
    }

    #[test]
    fn showing_says_what_the_display_shows() {
        let mut status = Status {
            devices: vec!["AK400 DIGITAL".into()],
            shown: Some(Shown::Usage),
            component: Some(Component::Gpu),
            ..Status::default()
        };
        assert_eq!(showing(Some(&status), en()), "Showing the GPU usage");
        status.display_off = Some(DisplayOff::Night);
        assert_eq!(showing(Some(&status), en()), "Display off for the night");
        assert_eq!(showing(None, en()), "Not connected to the service");
    }

    #[test]
    fn language_list_starts_with_windows() {
        let choices: Vec<_> = language_choices().collect();
        assert_eq!(choices[0], None);
        assert_eq!(choices.len(), LANGS.len() + 1);
        assert_eq!(language_label(Some(Lang::De), en()), "Deutsch");
        assert!(language_label(None, en()).starts_with("Same as Windows ("));
    }

    #[test]
    fn every_setting_has_a_label() {
        for page in PAGES {
            for &c in page.settings() {
                let (title, about) = c.label(en());
                assert!(!title.is_empty() && !about.is_empty(), "{c:?}");
            }
        }
        assert_eq!(
            Control::Update.label(en()).0,
            format!("Version {}", env!("CARGO_PKG_VERSION"))
        );
    }
}
