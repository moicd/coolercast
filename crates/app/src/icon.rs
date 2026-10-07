//! Renders the tray icon: a rounded badge with a number, drawn with GDI at runtime.

use std::{ptr, slice};

use coolercast_core::win::wide;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, CreateBitmap, CreateCompatibleDC, CreateFontW,
    DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC,
    DeleteObject, DrawTextW, FF_DONTCARE, FW_BOLD, GdiFlush, OUT_DEFAULT_PRECIS, SelectObject,
    SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON,
};

use crate::gfx::pixel_bitmap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const NORMAL: Rgb = Rgb(15, 118, 110);
pub const ALARM: Rgb = Rgb(220, 38, 38);
pub const OFFLINE: Rgb = Rgb(107, 114, 128);

/// Creates an icon showing `text` in white on a `background` badge. The caller owns the icon.
pub fn render(text: &str, background: Rgb) -> HICON {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    unsafe {
        let dc = CreateCompatibleDC(ptr::null_mut());

        let (color, bits) = pixel_bitmap(dc, size, size);
        let previous = SelectObject(dc, color);

        // Draw white anti-aliased text on black; the gray level becomes text coverage.
        let font_px = if text.chars().count() >= 3 {
            size * 11 / 20
        } else {
            size * 4 / 5
        };
        let face = wide("Segoe UI");
        let font = CreateFontW(
            -font_px,
            0,
            0,
            0,
            FW_BOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as _,
            OUT_DEFAULT_PRECIS as _,
            CLIP_DEFAULT_PRECIS as _,
            ANTIALIASED_QUALITY as _,
            (DEFAULT_PITCH | FF_DONTCARE) as _,
            face.as_ptr(),
        );
        SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, 0x00FF_FFFF);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };
        let text16: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(
            dc,
            text16.as_ptr(),
            text16.len() as i32,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        GdiFlush();

        let pixels = slice::from_raw_parts_mut(bits.cast::<u32>(), (size * size) as usize);
        compose(pixels, size as usize, background);

        SelectObject(dc, previous);
        DeleteObject(font);
        DeleteDC(dc);

        // Alpha comes from the color bitmap; the mask only has to exist.
        let stride = (size as usize).div_ceil(16) * 2;
        let mask_bits = vec![0u8; stride * size as usize];
        let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr().cast());
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        DeleteObject(mask);
        DeleteObject(color);
        icon
    }
}

/// Turns the rendered text coverage into a BGRA badge with straight alpha.
fn compose(pixels: &mut [u32], size: usize, Rgb(r, g, b): Rgb) {
    let radius = size as f32 / 5.0;
    let far = size as f32 - radius;
    for (i, pixel) in pixels.iter_mut().enumerate() {
        let (x, y) = ((i % size) as f32 + 0.5, (i / size) as f32 + 0.5);
        let (dx, dy) = (x - x.clamp(radius, far), y - y.clamp(radius, far));
        if dx * dx + dy * dy > radius * radius {
            *pixel = 0;
            continue;
        }
        let coverage = (*pixel & 0xFF)
            .max((*pixel >> 8) & 0xFF)
            .max((*pixel >> 16) & 0xFF);
        let mix = |channel: u8| (u32::from(channel) * (255 - coverage) + 255 * coverage) / 255;
        *pixel = 0xFF00_0000 | (mix(r) << 16) | (mix(g) << 8) | mix(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_has_transparent_corners_and_text() {
        let size = 16;
        let mut pixels = vec![0u32; size * size];
        pixels[8 * size + 8] = 0x00FF_FFFF; // fully covered text pixel
        compose(&mut pixels, size, NORMAL);
        assert_eq!(pixels[0], 0, "corner is transparent");
        assert_eq!(pixels[8 * size + 8], 0xFFFF_FFFF, "text is opaque white");
        assert_eq!(
            pixels[8 * size + 2],
            0xFF0F_766E,
            "background is the badge color"
        );
    }

    #[test]
    fn renders_an_icon() {
        assert!(!render("41", NORMAL).is_null());
    }
}
