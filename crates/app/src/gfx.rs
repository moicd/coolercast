//! A small drawing layer over the GDI+ flat API: anti-aliased shapes and ClearType text in
//! device-independent pixels.

use std::cell::RefCell;
use std::{mem, ptr};

use coolercast_core::win::wide;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows_sys::Win32::Graphics::GdiPlus::*;

/// Keeps GDI+ initialized while alive.
pub struct Gdiplus(usize);

impl Gdiplus {
    pub fn start() -> Option<Self> {
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let mut token = 0usize;
        let status = unsafe { GdiplusStartup(&mut token, &input, ptr::null_mut()) };
        (status == Ok).then_some(Self(token))
    }
}

impl Drop for Gdiplus {
    fn drop(&mut self) {
        unsafe { GdiplusShutdown(self.0) };
    }
}

/// ARGB color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32)
    }

    pub const fn alpha(self, a: u8) -> Self {
        Self((self.0 & 0x00FF_FFFF) | (a as u32) << 24)
    }

    /// `self` blended toward `other` by `t` (0 keeps `self`, 1 gives `other`), alpha included.
    pub fn mix(self, other: Color, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let channel = |shift: u32| {
            let (a, b) = ((self.0 >> shift) & 0xFF, (other.0 >> shift) & 0xFF);
            ((a as f32 + (b as f32 - a as f32) * t).round() as u32) << shift
        };
        Self(channel(24) | channel(16) | channel(8) | channel(0))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        Self::new(
            self.x + dx,
            self.y + dy,
            self.w - 2.0 * dx,
            self.h - 2.0 * dy,
        )
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Font weight; maps to the regular and semibold faces of Segoe UI. `Icon` draws glyphs of the
/// Windows icon font (Segoe Fluent Icons, or Segoe MDL2 Assets before Windows 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Semibold,
    Icon,
}

struct Font {
    size: f32,
    weight: Weight,
    raw: *mut GpFont,
}

/// A 32-bit top-down DIB and its pixels.
pub fn pixel_bitmap(hdc: HDC, w: i32, h: i32) -> (HBITMAP, *mut u8) {
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

/// `PixelFormat32bppPARGB` from the GDI+ headers (a macro, missing from windows-sys).
const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x000E_200B;

pub struct Canvas {
    g: *mut GpGraphics,
    /// The bitmap `g` draws on when created with [`Canvas::on_pixels`].
    bitmap: *mut GpBitmap,
    scale: f32,
    families: [*mut GpFontFamily; 3],
    /// Font style of each family: semibold is synthesized from bold where no semibold face
    /// exists.
    styles: [i32; 3],
    format: *mut GpStringFormat,
    /// Like `format`, but wrapping at word boundaries and starting at the top.
    wrap_format: *mut GpStringFormat,
    fonts: RefCell<Vec<Font>>,
}

impl Canvas {
    /// Draws on `hdc`; `scale` converts device-independent pixels to device pixels.
    pub fn new(hdc: HDC, scale: f32) -> Option<Self> {
        let mut g = ptr::null_mut();
        if unsafe { GdipCreateFromHDC(hdc, &mut g) } != Ok {
            return None;
        }
        Some(Self::setup(
            g,
            ptr::null_mut(),
            scale,
            TextRenderingHintClearTypeGridFit,
        ))
    }

    /// Draws on 32-bit premultiplied ARGB pixels (top-down rows, `width * 4` bytes each) and
    /// keeps per-pixel alpha, so what is behind the window can show through.
    pub fn on_pixels(bits: *mut u8, width: i32, height: i32, scale: f32) -> Option<Self> {
        let mut bitmap = ptr::null_mut();
        let status = unsafe {
            GdipCreateBitmapFromScan0(
                width,
                height,
                width * 4,
                PIXEL_FORMAT_32BPP_PARGB,
                bits,
                &mut bitmap,
            )
        };
        if status != Ok {
            return None;
        }
        let mut g = ptr::null_mut();
        if unsafe { GdipGetImageGraphicsContext(bitmap.cast(), &mut g) } != Ok {
            unsafe { GdipDisposeImage(bitmap.cast()) };
            return None;
        }
        // ClearType needs an opaque background; grayscale anti-aliasing keeps the alpha right.
        Some(Self::setup(
            g,
            bitmap,
            scale,
            TextRenderingHintAntiAliasGridFit,
        ))
    }

    fn setup(
        g: *mut GpGraphics,
        bitmap: *mut GpBitmap,
        scale: f32,
        text_hint: TextRenderingHint,
    ) -> Self {
        unsafe {
            GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
            GdipSetTextRenderingHint(g, text_hint);
        }
        let family = |names: &[&str]| {
            names.iter().find_map(|name| {
                let mut f = ptr::null_mut();
                let ok = unsafe {
                    GdipCreateFontFamilyFromName(wide(name).as_ptr(), ptr::null_mut(), &mut f)
                };
                (ok == Ok).then_some(f)
            })
        };
        // Languages Segoe UI does not cover use their own Windows UI font, in bold.
        let script = crate::i18n::current().font();
        let regular = script
            .and_then(|name| family(&[name]))
            .or_else(|| family(&["Segoe UI", "Tahoma"]))
            .unwrap_or_else(|| {
                let mut f = ptr::null_mut();
                unsafe { GdipGetGenericFontFamilySansSerif(&mut f) };
                f
            });
        let semibold = match script {
            Some(_) => None,
            None => family(&["Segoe UI Semibold"]),
        };
        let semibold_style = if semibold.is_some() {
            FontStyleRegular
        } else {
            FontStyleBold
        };
        let semibold = semibold.unwrap_or(regular);
        let icons = family(&["Segoe Fluent Icons", "Segoe MDL2 Assets"]).unwrap_or(regular);
        // Typographic layout: no extra padding around the text.
        let mut generic = ptr::null_mut();
        let mut format = ptr::null_mut();
        unsafe {
            GdipStringFormatGetGenericTypographic(&mut generic);
            GdipCloneStringFormat(generic, &mut format);
            GdipSetStringFormatFlags(format, StringFormatFlagsNoWrap);
            GdipSetStringFormatLineAlign(format, StringAlignmentCenter);
            GdipSetStringFormatTrimming(format, StringTrimmingEllipsisCharacter);
        }
        let mut wrap_format = ptr::null_mut();
        unsafe {
            GdipCloneStringFormat(generic, &mut wrap_format);
            GdipSetStringFormatTrimming(wrap_format, StringTrimmingEllipsisWord);
        }
        Self {
            g,
            bitmap,
            scale,
            families: [regular, semibold, icons],
            styles: [FontStyleRegular, semibold_style, FontStyleRegular],
            format,
            wrap_format,
            fonts: RefCell::new(Vec::new()),
        }
    }

    fn s(&self, v: f32) -> f32 {
        v * self.scale
    }

    fn r(&self, r: Rect) -> Rect {
        Rect::new(self.s(r.x), self.s(r.y), self.s(r.w), self.s(r.h))
    }

    pub fn clear(&self, color: Color) {
        unsafe { GdipGraphicsClear(self.g, color.0) };
    }

    fn with_brush(&self, color: Color, f: impl FnOnce(*mut GpBrush)) {
        let mut brush: *mut GpSolidFill = ptr::null_mut();
        unsafe { GdipCreateSolidFill(color.0, &mut brush) };
        f(brush.cast());
        unsafe { GdipDeleteBrush(brush.cast()) };
    }

    fn with_pen(&self, color: Color, width: f32, f: impl FnOnce(*mut GpPen)) {
        let mut pen = ptr::null_mut();
        unsafe {
            GdipCreatePen1(color.0, self.s(width), UnitPixel, &mut pen);
            GdipSetPenStartCap(pen, LineCapRound);
            GdipSetPenEndCap(pen, LineCapRound);
            GdipSetPenLineJoin(pen, LineJoinRound);
        }
        f(pen);
        unsafe { GdipDeletePen(pen) };
    }

    fn rounded_path(&self, r: Rect, radius: f32) -> *mut GpPath {
        let mut path = ptr::null_mut();
        unsafe { GdipCreatePath(FillModeAlternate, &mut path) };
        self.add_rounded(path, r, radius);
        path
    }

    /// Adds a rounded rectangle to `path` as a closed figure.
    fn add_rounded(&self, path: *mut GpPath, r: Rect, radius: f32) {
        let r = self.r(r);
        let d = (self.s(radius) * 2.0).min(r.w).min(r.h);
        unsafe {
            if d <= 0.0 {
                GdipAddPathArc(path, r.x, r.y, 0.0, 0.0, 180.0, 90.0);
                GdipAddPathArc(path, r.right(), r.y, 0.0, 0.0, 270.0, 90.0);
                GdipAddPathArc(path, r.right(), r.bottom(), 0.0, 0.0, 0.0, 90.0);
                GdipAddPathArc(path, r.x, r.bottom(), 0.0, 0.0, 90.0, 90.0);
            } else {
                GdipAddPathArc(path, r.x, r.y, d, d, 180.0, 90.0);
                GdipAddPathArc(path, r.right() - d, r.y, d, d, 270.0, 90.0);
                GdipAddPathArc(path, r.right() - d, r.bottom() - d, d, d, 0.0, 90.0);
                GdipAddPathArc(path, r.x, r.bottom() - d, d, d, 90.0, 90.0);
            }
            GdipClosePathFigure(path);
        }
    }

    /// A soft shadow around a rounded rectangle, falling slightly downwards. GDI+ has no blur, so
    /// it stacks translucent rings; the rectangle itself is left untouched, so translucent glass
    /// on top does not get darker.
    pub fn shadow(&self, r: Rect, radius: f32, depth: f32, color: Color) {
        const STEPS: u32 = 4;
        let alpha = ((color.0 >> 24) / STEPS) as u8;
        for i in 1..=STEPS {
            let grow = depth * i as f32 / STEPS as f32;
            let outer = Rect::new(
                r.x - grow,
                r.y - grow + depth * 0.4,
                r.w + 2.0 * grow,
                r.h + 2.0 * grow,
            );
            let path = self.rounded_path(outer, radius + grow);
            self.add_rounded(path, r, radius);
            self.with_brush(color.alpha(alpha), |b| unsafe {
                GdipFillPath(self.g, b, path);
            });
            unsafe { GdipDeletePath(path) };
        }
    }

    pub fn fill_round_rect(&self, r: Rect, radius: f32, color: Color) {
        let path = self.rounded_path(r, radius);
        self.with_brush(color, |b| unsafe {
            GdipFillPath(self.g, b, path);
        });
        unsafe { GdipDeletePath(path) };
    }

    /// Calls `f` with a vertical gradient brush spanning `r` (device pixels).
    fn with_gradient(&self, r: Rect, top: Color, bottom: Color, f: impl FnOnce(*mut GpBrush)) {
        // One pixel of overscan keeps the wrapped gradient from bleeding into the edges.
        let rect = RectF {
            X: r.x,
            Y: r.y - 1.0,
            Width: r.w.max(1.0),
            Height: r.h + 2.0,
        };
        let mut brush: *mut GpLineGradient = ptr::null_mut();
        unsafe {
            GdipCreateLineBrushFromRect(
                &rect,
                top.0,
                bottom.0,
                LinearGradientModeVertical,
                WrapModeTileFlipXY,
                &mut brush,
            )
        };
        f(brush.cast());
        unsafe { GdipDeleteBrush(brush.cast()) };
    }

    /// Fills a rounded rectangle with a vertical gradient.
    pub fn fill_round_rect_v(&self, r: Rect, radius: f32, top: Color, bottom: Color) {
        if top == bottom {
            return self.fill_round_rect(r, radius, top);
        }
        let path = self.rounded_path(r, radius);
        self.with_gradient(self.r(r), top, bottom, |b| unsafe {
            GdipFillPath(self.g, b, path);
        });
        unsafe { GdipDeletePath(path) };
    }

    /// Fills a rounded rectangle with `color` at the top, fading out at `fraction` of its height.
    /// The fade keeps the outline of the whole rectangle, round ends included.
    pub fn fill_round_rect_fade(&self, r: Rect, radius: f32, color: Color, fraction: f32) {
        let path = self.rounded_path(r, radius);
        let device = self.r(r);
        // Where the fade ends on the gradient, which `with_gradient` overscans by one pixel.
        let end = (1.0 + fraction * device.h) / (device.h + 2.0);
        let (factors, positions) = ([0.0, 1.0, 1.0], [0.0, end, 1.0]);
        self.with_gradient(device, color, color.alpha(0), |b| unsafe {
            GdipSetLineBlend(b.cast(), factors.as_ptr(), positions.as_ptr(), 3);
            GdipFillPath(self.g, b, path);
        });
        unsafe { GdipDeletePath(path) };
    }

    /// Outlines a rounded rectangle with a vertical gradient, inside the rectangle.
    pub fn stroke_round_rect_v(&self, r: Rect, radius: f32, width: f32, top: Color, bottom: Color) {
        if top == bottom {
            return self.stroke_round_rect(r, radius, width, top);
        }
        let path = self.rounded_path(r.inset(width / 2.0, width / 2.0), radius);
        self.with_gradient(self.r(r), top, bottom, |b| unsafe {
            let mut pen = ptr::null_mut();
            GdipCreatePen2(b, self.s(width), UnitPixel, &mut pen);
            GdipDrawPath(self.g, pen, path);
            GdipDeletePen(pen);
        });
        unsafe { GdipDeletePath(path) };
    }

    pub fn stroke_round_rect(&self, r: Rect, radius: f32, width: f32, color: Color) {
        // Keep the stroke inside the rectangle.
        let path = self.rounded_path(r.inset(width / 2.0, width / 2.0), radius);
        self.with_pen(color, width, |p| unsafe {
            GdipDrawPath(self.g, p, path);
        });
        unsafe { GdipDeletePath(path) };
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, color: Color) {
        let (x, y, d) = (
            self.s(cx - radius),
            self.s(cy - radius),
            self.s(radius * 2.0),
        );
        self.with_brush(color, |b| unsafe {
            GdipFillEllipse(self.g, b, x, y, d, d);
        });
    }

    /// An arc of the circle at `(cx, cy)`; angles in degrees, clockwise from the positive x axis.
    pub fn stroke_arc(
        &self,
        (cx, cy): (f32, f32),
        radius: f32,
        (start, sweep): (f32, f32),
        width: f32,
        color: Color,
    ) {
        let (x, y, d) = (
            self.s(cx - radius),
            self.s(cy - radius),
            self.s(radius * 2.0),
        );
        self.with_pen(color, width, |p| unsafe {
            GdipDrawArc(self.g, p, x, y, d, d, start, sweep);
        });
    }

    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: Color) {
        let (x1, y1, x2, y2) = (self.s(x1), self.s(y1), self.s(x2), self.s(y2));
        self.with_pen(color, width, |p| unsafe {
            GdipDrawLine(self.g, p, x1, y1, x2, y2);
        });
    }

    pub fn dashed_line(&self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: Color) {
        let (x1, y1, x2, y2) = (self.s(x1), self.s(y1), self.s(x2), self.s(y2));
        self.with_pen(color, width, |p| unsafe {
            GdipSetPenStartCap(p, LineCapFlat);
            GdipSetPenEndCap(p, LineCapFlat);
            GdipSetPenDashStyle(p, DashStyleDash);
            GdipDrawLine(self.g, p, x1, y1, x2, y2);
        });
    }

    pub fn polyline(&self, points: &[(f32, f32)], width: f32, color: Color) {
        if points.len() < 2 {
            return;
        }
        let pts = self.points(points);
        self.with_pen(color, width, |p| unsafe {
            GdipDrawLines(self.g, p, pts.as_ptr(), pts.len() as i32);
        });
    }

    pub fn fill_polygon(&self, points: &[(f32, f32)], color: Color) {
        let pts = self.points(points);
        self.with_brush(color, |b| unsafe {
            GdipFillPolygon(self.g, b, pts.as_ptr(), pts.len() as i32, FillModeAlternate);
        });
    }

    fn points(&self, points: &[(f32, f32)]) -> Vec<PointF> {
        points
            .iter()
            .map(|&(x, y)| PointF {
                X: self.s(x),
                Y: self.s(y),
            })
            .collect()
    }

    fn font(&self, size: f32, weight: Weight) -> *mut GpFont {
        let mut fonts = self.fonts.borrow_mut();
        if let Some(f) = fonts.iter().find(|f| f.size == size && f.weight == weight) {
            return f.raw;
        }
        let family = self.families[weight as usize];
        let style = self.styles[weight as usize];
        let mut raw = ptr::null_mut();
        unsafe { GdipCreateFont(family, self.s(size), style, UnitPixel, &mut raw) };
        fonts.push(Font { size, weight, raw });
        raw
    }

    /// Draws one line of text, vertically centered in `r`.
    pub fn text(&self, text: &str, r: Rect, size: f32, weight: Weight, color: Color, align: Align) {
        unsafe {
            GdipSetStringFormatAlign(
                self.format,
                match align {
                    Align::Left => StringAlignmentNear,
                    Align::Center => StringAlignmentCenter,
                    Align::Right => StringAlignmentFar,
                },
            )
        };
        self.draw_string(text, r, size, weight, color, self.format);
    }

    /// Draws text wrapped at word boundaries, from the top of `r`, left-aligned.
    pub fn text_wrapped(&self, text: &str, r: Rect, size: f32, weight: Weight, color: Color) {
        self.draw_string(text, r, size, weight, color, self.wrap_format);
    }

    fn draw_string(
        &self,
        text: &str,
        r: Rect,
        size: f32,
        weight: Weight,
        color: Color,
        format: *mut GpStringFormat,
    ) {
        let font = self.font(size, weight);
        let text16: Vec<u16> = text.encode_utf16().collect();
        let layout = self.r(r);
        let rect = RectF {
            X: layout.x,
            Y: layout.y,
            Width: layout.w,
            Height: layout.h,
        };
        self.with_brush(color, |b| unsafe {
            GdipDrawString(
                self.g,
                text16.as_ptr(),
                text16.len() as i32,
                font,
                &rect,
                format,
                b,
            );
        });
    }

    /// How many lines `text` takes when wrapped to `width`.
    pub fn lines(&self, text: &str, size: f32, weight: Weight, width: f32) -> u32 {
        let font = self.font(size, weight);
        let text16: Vec<u16> = text.encode_utf16().collect();
        let layout = RectF {
            X: 0.0,
            Y: 0.0,
            Width: self.s(width),
            Height: 10_000.0,
        };
        let mut bounds = RectF::default();
        let mut lines = 0;
        unsafe {
            GdipMeasureString(
                self.g,
                text16.as_ptr(),
                text16.len() as i32,
                font,
                &layout,
                self.wrap_format,
                &mut bounds,
                ptr::null_mut(),
                &mut lines,
            );
        }
        lines.max(1) as u32
    }

    /// Width of `text` in device-independent pixels.
    pub fn measure(&self, text: &str, size: f32, weight: Weight) -> f32 {
        let font = self.font(size, weight);
        let text16: Vec<u16> = text.encode_utf16().collect();
        let layout = RectF {
            X: 0.0,
            Y: 0.0,
            Width: 10_000.0,
            Height: 10_000.0,
        };
        let mut bounds = RectF::default();
        unsafe {
            GdipSetStringFormatAlign(self.format, StringAlignmentNear);
            GdipMeasureString(
                self.g,
                text16.as_ptr(),
                text16.len() as i32,
                font,
                &layout,
                self.format,
                &mut bounds,
                ptr::null_mut(),
                ptr::null_mut(),
            );
        }
        bounds.Width / self.scale
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            for font in self.fonts.borrow().iter() {
                GdipDeleteFont(font.raw);
            }
            GdipDeleteStringFormat(self.format);
            GdipDeleteStringFormat(self.wrap_format);
            // Fallbacks share the regular family: delete each one once.
            for (i, &family) in self.families.iter().enumerate() {
                if !self.families[..i].contains(&family) {
                    GdipDeleteFontFamily(family);
                }
            }
            GdipDeleteGraphics(self.g);
            if !self.bitmap.is_null() {
                GdipDisposeImage(self.bitmap.cast());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_blends_every_channel() {
        let black = Color::rgb(0, 0, 0);
        let white = Color::rgb(0xFF, 0xFF, 0xFF);
        assert_eq!(black.mix(white, 0.0), black);
        assert_eq!(black.mix(white, 1.0), white);
        assert_eq!(black.mix(white, 0.5), Color::rgb(0x80, 0x80, 0x80));
        assert_eq!(white.alpha(0).mix(white, 0.5), white.alpha(0x80));
    }
}
