//! Colors that follow the Windows light/dark app setting.

use std::ptr;

use coolercast_core::win::wide;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

use crate::gfx::Color;

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub background: Color,
    pub card: Color,
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
    pub fn current() -> Self {
        if apps_use_dark_theme() {
            Self::dark()
        } else {
            Self::light()
        }
    }

    fn dark() -> Self {
        Self {
            dark: true,
            background: Color::rgb(0x20, 0x20, 0x20),
            card: Color::rgb(0x2B, 0x2B, 0x2B),
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
            background: Color::rgb(0xF3, 0xF3, 0xF3),
            card: Color::rgb(0xFF, 0xFF, 0xFF),
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
