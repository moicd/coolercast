//! Colors that follow the Windows light/dark app setting, opaque or as glass over the window
//! backdrop.

use std::ptr;

use coolercast_core::win::wide;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

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
    pub usage_line: Color,
    pub grid: Color,
}

impl Theme {
    pub fn current(glass: bool) -> Self {
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
            usage_line: Color::rgb(0x93, 0xC5, 0xFD),
            grid: Color::rgb(0x36, 0x36, 0x36),
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
            usage_line: Color::rgb(0x25, 0x63, 0xEB),
            grid: Color::rgb(0xEC, 0xEC, 0xEC),
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
                card_top: Color::rgb(0x50, 0x50, 0x50).alpha(0x8C),
                card_bottom: Color::rgb(0x2C, 0x2C, 0x2C).alpha(0x78),
                edge_top: white.alpha(0x5C),
                edge_bottom: white.alpha(0x12),
                border: white.alpha(0x16),
                control: white.alpha(0x14),
                control_hover: white.alpha(0x26),
                grid: white.alpha(0x14),
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
                ..self
            }
        }
    }

    /// The window background as a GDI `COLORREF` (0x00BBGGRR).
    pub fn background_colorref(&self) -> u32 {
        let c = self.background.0;
        ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
    }
}

fn apps_use_dark_theme() -> bool {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut value = 1u32;
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
    status == ERROR_SUCCESS && value == 0
}
