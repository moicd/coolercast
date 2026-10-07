//! Colors that follow the Windows light/dark app setting, opaque or translucent over the Mica
//! backdrop, and the accessibility settings that change them: high contrast, transparency
//! effects and text size.

use std::ptr;

use coolercast_core::win::wide;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW,
    COLOR_WINDOWTEXT, GetSysColor,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SystemParametersInfoW,
};

use crate::gfx::Color;

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    /// Translucent colors over the system backdrop instead of opaque ones.
    pub glass: bool,
    pub background: Color,
    /// What the window is cleared with: `background`, or a light tint over the backdrop.
    pub backdrop: Color,
    /// Flat fill of cards and the language list, and their hairline outline.
    pub card: Color,
    pub edge: Color,
    /// Outline of the switch track while off.
    pub border: Color,
    pub text: Color,
    pub text_dim: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub control: Color,
    pub control_hover: Color,
    /// Outline of buttons and fields: only high contrast draws one.
    pub outline: Color,
    pub focus: Color,
    pub ok: Color,
    pub warn: Color,
    pub alarm: Color,
    /// Temperature in the chart: warm, and distinct from the accent of selected controls.
    pub temp_line: Color,
    pub usage_line: Color,
    pub grid: Color,
    /// The fill under the item the mouse is over.
    pub hover: Color,
    /// The fill of the selected sidebar page, next to its accent mark.
    pub selection: Color,
    /// The selected option of a segmented control, and the text on it.
    pub thumb: Color,
    pub on_thumb: Color,
}

impl Theme {
    pub fn current(glass: bool) -> Self {
        if high_contrast() {
            return Self::high_contrast();
        }
        let theme = if apps_use_dark_theme() {
            Self::dark()
        } else {
            Self::light()
        };
        if glass { theme.glass() } else { theme }
    }

    fn dark() -> Self {
        Self {
            dark: true,
            glass: false,
            background: Color::rgb(0x20, 0x20, 0x20),
            backdrop: Color::rgb(0x20, 0x20, 0x20),
            card: Color::rgb(0x2B, 0x2B, 0x2B),
            edge: Color::rgb(0x1D, 0x1D, 0x1D),
            border: Color::rgb(0x9A, 0x9A, 0x9A),
            text: Color::rgb(0xF2, 0xF2, 0xF2),
            text_dim: Color::rgb(0xA8, 0xA8, 0xA8),
            accent: Color::rgb(0x2D, 0xD4, 0xBF),
            on_accent: Color::rgb(0x04, 0x2F, 0x2E),
            control: Color::rgb(0x38, 0x38, 0x38),
            control_hover: Color::rgb(0x44, 0x44, 0x44),
            outline: Color::rgb(0, 0, 0).alpha(0),
            focus: Color::rgb(0xFF, 0xFF, 0xFF),
            ok: Color::rgb(0x4A, 0xDE, 0x80),
            warn: Color::rgb(0xFB, 0xBF, 0x24),
            alarm: Color::rgb(0xF8, 0x71, 0x71),
            temp_line: Color::rgb(0xFB, 0xA9, 0x4C),
            usage_line: Color::rgb(0x93, 0xC5, 0xFD),
            grid: Color::rgb(0x36, 0x36, 0x36),
            hover: Color::rgb(0xFF, 0xFF, 0xFF).alpha(0x0F),
            selection: Color::rgb(0xFF, 0xFF, 0xFF).alpha(0x15),
            thumb: Color::rgb(0x50, 0x50, 0x50),
            on_thumb: Color::rgb(0xF2, 0xF2, 0xF2),
        }
    }

    fn light() -> Self {
        Self {
            dark: false,
            glass: false,
            background: Color::rgb(0xF3, 0xF3, 0xF3),
            backdrop: Color::rgb(0xF3, 0xF3, 0xF3),
            card: Color::rgb(0xFF, 0xFF, 0xFF),
            edge: Color::rgb(0xE5, 0xE5, 0xE5),
            border: Color::rgb(0x8A, 0x8A, 0x8A),
            text: Color::rgb(0x1A, 0x1A, 0x1A),
            text_dim: Color::rgb(0x61, 0x61, 0x61),
            accent: Color::rgb(0x0F, 0x76, 0x6E),
            on_accent: Color::rgb(0xFF, 0xFF, 0xFF),
            control: Color::rgb(0xEE, 0xEE, 0xEE),
            control_hover: Color::rgb(0xE2, 0xE2, 0xE2),
            outline: Color::rgb(0, 0, 0).alpha(0),
            focus: Color::rgb(0x1A, 0x1A, 0x1A),
            ok: Color::rgb(0x16, 0xA3, 0x4A),
            warn: Color::rgb(0xD9, 0x77, 0x06),
            alarm: Color::rgb(0xDC, 0x26, 0x26),
            temp_line: Color::rgb(0xC2, 0x41, 0x0C),
            usage_line: Color::rgb(0x25, 0x63, 0xEB),
            grid: Color::rgb(0xEC, 0xEC, 0xEC),
            hover: Color::rgb(0, 0, 0).alpha(0x09),
            selection: Color::rgb(0, 0, 0).alpha(0x0C),
            thumb: Color::rgb(0xFF, 0xFF, 0xFF),
            on_thumb: Color::rgb(0x1A, 0x1A, 0x1A),
        }
    }

    /// Translucent variant: flat cards and controls over the Mica backdrop, with the fills of the
    /// Windows 11 controls.
    fn glass(self) -> Self {
        let white = Color::rgb(0xFF, 0xFF, 0xFF);
        let black = Color::rgb(0, 0, 0);
        if self.dark {
            Self {
                glass: true,
                backdrop: Color::rgb(0x10, 0x10, 0x10).alpha(0x48),
                card: white.alpha(0x10),
                edge: black.alpha(0x50),
                border: white.alpha(0x8B),
                control: white.alpha(0x14),
                control_hover: white.alpha(0x26),
                grid: white.alpha(0x14),
                hover: white.alpha(0x0F),
                selection: white.alpha(0x15),
                thumb: white.alpha(0x30),
                ..self
            }
        } else {
            Self {
                glass: true,
                backdrop: white.alpha(0x38),
                card: white.alpha(0xB3),
                edge: black.alpha(0x12),
                border: black.alpha(0x72),
                control: black.alpha(0x0C),
                control_hover: black.alpha(0x18),
                grid: black.alpha(0x10),
                hover: black.alpha(0x09),
                selection: black.alpha(0x0C),
                thumb: white.alpha(0xF2),
                ..self
            }
        }
    }

    /// The Windows high contrast colors, opaque and outlined.
    fn high_contrast() -> Self {
        let sys = |index: i32| {
            let c = unsafe { GetSysColor(index) };
            Color::rgb(c as u8, (c >> 8) as u8, (c >> 16) as u8)
        };
        let (window, text) = (sys(COLOR_WINDOW), sys(COLOR_WINDOWTEXT));
        let (highlight, on_highlight) = (sys(COLOR_HIGHLIGHT), sys(COLOR_HIGHLIGHTTEXT));
        let link = sys(COLOR_HOTLIGHT);
        let r = (window.0 >> 16) & 0xFF;
        Self {
            dark: r < 0x80,
            glass: false,
            background: window,
            backdrop: window,
            card: window,
            edge: text,
            border: text,
            text,
            text_dim: text,
            accent: highlight,
            on_accent: on_highlight,
            control: window,
            control_hover: highlight.alpha(0x60),
            outline: text,
            focus: text,
            ok: text,
            warn: text,
            alarm: link,
            temp_line: highlight,
            usage_line: link,
            grid: sys(COLOR_GRAYTEXT),
            // No translucency: shapes are outlined instead.
            hover: highlight.alpha(0x60),
            selection: highlight,
            thumb: highlight,
            on_thumb: on_highlight,
        }
    }

    /// The window background as a GDI `COLORREF` (0x00BBGGRR).
    pub fn background_colorref(&self) -> u32 {
        let c = self.background.0;
        ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
    }
}

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

fn apps_use_dark_theme() -> bool {
    registry_dword(PERSONALIZE, "AppsUseLightTheme") == Some(0)
}

/// Whether the window may use the translucent backdrop: not with high contrast, nor with
/// transparency effects turned off in Settings › Accessibility › Visual effects.
pub fn transparency_allowed() -> bool {
    !high_contrast() && registry_dword(PERSONALIZE, "EnableTransparency") != Some(0)
}

/// Settings › Accessibility › Text size, as a factor from 1.0 to 2.25.
pub fn text_scale() -> f32 {
    registry_dword(r"Software\Microsoft\Accessibility", "TextScaleFactor")
        .map_or(1.0, |percent| percent.clamp(100, 225) as f32 / 100.0)
}

/// Settings › Accessibility › Visual effects › Animation effects.
pub fn animations_enabled() -> bool {
    let mut on = 1i32;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut on as *mut i32).cast(),
            0,
        )
    };
    ok == 0 || on != 0
}

fn high_contrast() -> bool {
    let mut hc = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            (&mut hc as *mut HIGHCONTRASTW).cast(),
            0,
        )
    };
    ok != 0 && hc.dwFlags & HCF_HIGHCONTRASTON != 0
}

fn registry_dword(key: &str, name: &str) -> Option<u32> {
    let (key, name) = (wide(key), wide(name));
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}
