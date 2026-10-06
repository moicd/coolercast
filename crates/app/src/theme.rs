//! Colors that follow the Windows light/dark app setting, opaque or as glass over the window
//! backdrop, and the accessibility settings that change them: high contrast, transparency
//! effects and text size.

use std::ptr;

use coolercast_core::win::wide;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Graphics::Gdi::GetSysColor;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
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
    /// Card fill, as a vertical gradient (equal when opaque).
    pub card_top: Color,
    pub card_bottom: Color,
    /// Card outline, as a vertical gradient: a lit top edge on glass.
    pub edge_top: Color,
    pub edge_bottom: Color,
    /// Dividers between rows.
    pub border: Color,
    pub text: Color,
    pub text_dim: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub control: Color,
    pub control_hover: Color,
    pub focus: Color,
    pub ok: Color,
    pub warn: Color,
    pub alarm: Color,
    /// Temperature in the chart: warm, and distinct from the accent of selected controls.
    pub temp_line: Color,
    pub usage_line: Color,
    pub grid: Color,
    /// Soft shadow under cards, panels and raised controls.
    pub shadow: Color,
    /// The pill under the item the mouse is over.
    pub hover: Color,
    /// The pill of the selected sidebar page, tinted with the accent.
    pub selection: Color,
    /// The raised thumb of a segmented control, and the text on it.
    pub thumb: Color,
    pub on_thumb: Color,
    /// The round knob of a switch.
    pub knob: Color,
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
            card_top: Color::rgb(0x2B, 0x2B, 0x2B),
            card_bottom: Color::rgb(0x2B, 0x2B, 0x2B),
            edge_top: Color::rgb(0x3A, 0x3A, 0x3A),
            edge_bottom: Color::rgb(0x3A, 0x3A, 0x3A),
            border: Color::rgb(0x3A, 0x3A, 0x3A),
            text: Color::rgb(0xF2, 0xF2, 0xF2),
            text_dim: Color::rgb(0xA8, 0xA8, 0xA8),
            accent: Color::rgb(0x2D, 0xD4, 0xBF),
            on_accent: Color::rgb(0x04, 0x2F, 0x2E),
            control: Color::rgb(0x38, 0x38, 0x38),
            control_hover: Color::rgb(0x44, 0x44, 0x44),
            focus: Color::rgb(0xFF, 0xFF, 0xFF),
            ok: Color::rgb(0x4A, 0xDE, 0x80),
            warn: Color::rgb(0xFB, 0xBF, 0x24),
            alarm: Color::rgb(0xF8, 0x71, 0x71),
            temp_line: Color::rgb(0xFB, 0xA9, 0x4C),
            usage_line: Color::rgb(0x93, 0xC5, 0xFD),
            grid: Color::rgb(0x36, 0x36, 0x36),
            shadow: Color::rgb(0, 0, 0).alpha(0x60),
            hover: Color::rgb(0xFF, 0xFF, 0xFF).alpha(0x12),
            selection: Color::rgb(0x2D, 0xD4, 0xBF).alpha(0x30),
            thumb: Color::rgb(0x5C, 0x5C, 0x5C),
            on_thumb: Color::rgb(0xF2, 0xF2, 0xF2),
            knob: Color::rgb(0xFF, 0xFF, 0xFF),
        }
    }

    fn light() -> Self {
        Self {
            dark: false,
            glass: false,
            background: Color::rgb(0xF3, 0xF3, 0xF3),
            backdrop: Color::rgb(0xF3, 0xF3, 0xF3),
            card_top: Color::rgb(0xFF, 0xFF, 0xFF),
            card_bottom: Color::rgb(0xFF, 0xFF, 0xFF),
            edge_top: Color::rgb(0xE5, 0xE5, 0xE5),
            edge_bottom: Color::rgb(0xE5, 0xE5, 0xE5),
            border: Color::rgb(0xE5, 0xE5, 0xE5),
            text: Color::rgb(0x1A, 0x1A, 0x1A),
            text_dim: Color::rgb(0x61, 0x61, 0x61),
            accent: Color::rgb(0x0F, 0x76, 0x6E),
            on_accent: Color::rgb(0xFF, 0xFF, 0xFF),
            control: Color::rgb(0xEE, 0xEE, 0xEE),
            control_hover: Color::rgb(0xE2, 0xE2, 0xE2),
            focus: Color::rgb(0x1A, 0x1A, 0x1A),
            ok: Color::rgb(0x16, 0xA3, 0x4A),
            warn: Color::rgb(0xD9, 0x77, 0x06),
            alarm: Color::rgb(0xDC, 0x26, 0x26),
            temp_line: Color::rgb(0xC2, 0x41, 0x0C),
            usage_line: Color::rgb(0x25, 0x63, 0xEB),
            grid: Color::rgb(0xEC, 0xEC, 0xEC),
            shadow: Color::rgb(0, 0, 0).alpha(0x24),
            hover: Color::rgb(0, 0, 0).alpha(0x0A),
            selection: Color::rgb(0x0F, 0x76, 0x6E).alpha(0x22),
            thumb: Color::rgb(0xFF, 0xFF, 0xFF),
            on_thumb: Color::rgb(0x1A, 0x1A, 0x1A),
            knob: Color::rgb(0xFF, 0xFF, 0xFF),
        }
    }

    /// Translucent variant: cards and controls let the blurred backdrop through, with a lit top
    /// edge and a soft top-to-bottom fade.
    fn glass(self) -> Self {
        let white = Color::rgb(0xFF, 0xFF, 0xFF);
        let black = Color::rgb(0, 0, 0);
        if self.dark {
            Self {
                glass: true,
                backdrop: Color::rgb(0x10, 0x10, 0x10).alpha(0x48),
                // #A8A8A8 reads at 3.7:1 on the lightest part of a glass card (#4B4B4B);
                // this one at 5.0:1.
                text_dim: Color::rgb(0xC4, 0xC4, 0xC4),
                card_top: Color::rgb(0x50, 0x50, 0x50).alpha(0x8C),
                card_bottom: Color::rgb(0x2C, 0x2C, 0x2C).alpha(0x78),
                edge_top: white.alpha(0x5C),
                edge_bottom: white.alpha(0x12),
                border: white.alpha(0x16),
                control: white.alpha(0x14),
                control_hover: white.alpha(0x26),
                grid: white.alpha(0x14),
                hover: white.alpha(0x16),
                selection: self.accent.alpha(0x3C),
                thumb: white.alpha(0x38),
                ..self
            }
        } else {
            Self {
                glass: true,
                backdrop: white.alpha(0x38),
                card_top: white.alpha(0xD0),
                card_bottom: white.alpha(0x94),
                edge_top: white,
                edge_bottom: black.alpha(0x16),
                border: black.alpha(0x12),
                control: black.alpha(0x0C),
                control_hover: black.alpha(0x18),
                grid: black.alpha(0x10),
                // Light glass is nearly white already: hovering darkens it a little.
                hover: black.alpha(0x0C),
                selection: self.accent.alpha(0x2A),
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
            card_top: window,
            card_bottom: window,
            edge_top: text,
            edge_bottom: text,
            border: text,
            text,
            text_dim: text,
            accent: highlight,
            on_accent: on_highlight,
            control: window,
            control_hover: highlight.alpha(0x60),
            focus: text,
            ok: text,
            warn: text,
            alarm: link,
            temp_line: highlight,
            usage_line: link,
            grid: sys(COLOR_GRAYTEXT),
            // No shadows or translucency: shapes are outlined instead.
            shadow: window.alpha(0),
            hover: highlight.alpha(0x60),
            selection: highlight,
            thumb: highlight,
            on_thumb: on_highlight,
            knob: on_highlight,
        }
    }

    /// The window background as a GDI `COLORREF` (0x00BBGGRR).
    pub fn background_colorref(&self) -> u32 {
        let c = self.background.0;
        ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
    }
}

const COLOR_WINDOW: i32 = 5;
const COLOR_WINDOWTEXT: i32 = 8;
const COLOR_HIGHLIGHT: i32 = 13;
const COLOR_HIGHLIGHTTEXT: i32 = 14;
const COLOR_GRAYTEXT: i32 = 17;
const COLOR_HOTLIGHT: i32 = 26;
const SPI_GETHIGHCONTRAST: u32 = 0x0042;
const HCF_HIGHCONTRASTON: u32 = 1;

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
    /// `HIGHCONTRASTW`.
    #[repr(C)]
    struct HighContrast {
        size: u32,
        flags: u32,
        scheme: *mut u16,
    }
    let mut hc = HighContrast {
        size: size_of::<HighContrast>() as u32,
        flags: 0,
        scheme: ptr::null_mut(),
    };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.size,
            (&mut hc as *mut HighContrast).cast(),
            0,
        )
    };
    ok != 0 && hc.flags & HCF_HIGHCONTRASTON != 0
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
