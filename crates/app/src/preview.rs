//! Simulated AK series display: the 10-step bar on top, three seven-segment digits and the unit
//! symbols under it, lit the same way as the real one.

use coolercast_core::config::{Bar, Config, Symbol, Unit};
use coolercast_core::device::{self, Component, Family, ak};
use coolercast_core::ipc::{Shown, Status};

use crate::gfx::{Align, Canvas, Color, Rect, Weight};

const PANEL: Color = Color::rgb(0x0B, 0x0F, 0x10);
const LIT: Color = Color::rgb(0xE6, 0xFF, 0xFB);
const UNLIT: Color = Color::rgb(0x1C, 0x24, 0x26);
const ALARM: Color = Color::rgb(0xFF, 0x5A, 0x5A);

/// What the display shows: three digits (`None` = blank), the unit and the bar level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub digits: [Option<u8>; 3],
    pub symbol: Symbol,
    pub bar: u8,
    pub alarm: bool,
}

/// Whether the preview matches what the connected coolers show: with an AK series display, or
/// before any cooler is connected.
pub fn applies_to(devices: &[String]) -> bool {
    devices.is_empty()
        || devices
            .iter()
            .any(|name| device::model_named(name).is_some_and(|m| m.family == Family::AkSeries))
}

impl Frame {
    pub fn from_status(status: &Status) -> Option<Self> {
        let alarm = status.alarm_active;
        let (temp, usage) = match status.component {
            Some(Component::Gpu) => (status.gpu.temp, status.gpu.usage),
            _ => (status.cpu_temp, status.cpu_usage),
        };
        let (value, bar, symbol) = match (status.shown?, temp) {
            (Shown::Custom, _) => return Some(Self::custom(&status.config, alarm)),
            // AK displays have no power symbol and show the temperature instead.
            (Shown::Temperature | Shown::Power, Some(t)) => {
                let unit = status.config.unit;
                let symbol = match unit {
                    Unit::Celsius => Symbol::Celsius,
                    Unit::Fahrenheit => Symbol::Fahrenheit,
                };
                (unit.from_celsius(t), t, symbol)
            }
            _ => {
                let usage = usage?;
                (usage, usage, Symbol::Percent)
            }
        };
        // With the usage bar, a temperature comes with the usage of the same component.
        let bar = match (status.config.bar, symbol, usage) {
            (Bar::Usage, Symbol::Celsius | Symbol::Fahrenheit, Some(usage)) => usage,
            _ => bar,
        };
        Some(Self::new(
            ak::display_value(value),
            symbol,
            ak::bar_level(bar),
            alarm,
        ))
    }

    /// The custom value of `config`, as the cooler shows it in custom mode.
    pub fn custom(config: &Config, alarm: bool) -> Self {
        Self::new(
            config.custom_value.min(999),
            config.custom_symbol,
            config.custom_bar.clamp(1, 10),
            alarm,
        )
    }

    fn new(value: u16, symbol: Symbol, bar: u8, alarm: bool) -> Self {
        let digits = [value / 100, value / 10 % 10, value % 10].map(|d| d as u8);
        // Leading zeros stay dark, like on the cooler.
        let digits = [
            (value >= 100).then_some(digits[0]),
            (value >= 10).then_some(digits[1]),
            Some(digits[2]),
        ];
        Self {
            digits,
            symbol,
            bar,
            alarm,
        }
    }
}

/// Segments a–g of each digit, as in the usual seven-segment naming.
const DIGIT_SEGMENTS: [u8; 10] = [
    0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110, 0b1101101, 0b1111101, 0b0000111,
    0b1111111, 0b1101111,
];

/// Size the layout below is drawn at; other sizes scale it.
const BASE_W: f32 = 184.0;

/// Draws the display in `area`, scaled to its width.
pub fn draw(c: &Canvas, area: Rect, frame: Option<Frame>) {
    let k = area.w / BASE_W;
    let radius = 10.0 * k;
    c.fill_round_rect(area, radius, PANEL);
    // Reflection on the cover glass, over the top of the panel.
    let white = Color::rgb(0xFF, 0xFF, 0xFF);
    let sheen = Rect::new(area.x, area.y, area.w, area.h * 0.5);
    c.fill_round_rect_v(sheen, radius, white.alpha(0x14), white.alpha(0x00));

    let digit_w = 30.0 * k;
    let digit_h = 54.0 * k;
    let gap = 10.0 * k;
    let symbols_w = 40.0 * k;
    let total_w = 3.0 * digit_w + 2.0 * gap + 16.0 * k + symbols_w;
    let x0 = area.x + (area.w - total_w) / 2.0;

    // Bar: ten segments across the top, above the digits, red while the alarm is on.
    let alarm = frame.is_some_and(|f| f.alarm);
    let level = frame.map_or(0, |f| f.bar);
    let bar_y = area.y + 16.0 * k;
    let bar_h = 8.0 * k;
    let seg_gap = 4.0 * k;
    let seg_w = (total_w - 9.0 * seg_gap) / 10.0;
    for i in 0..10u8 {
        let r = Rect::new(x0 + f32::from(i) * (seg_w + seg_gap), bar_y, seg_w, bar_h);
        let color = match (i < level, alarm) {
            (true, true) => ALARM,
            (true, false) => LIT,
            _ => {
                c.fill_round_rect(r, 2.0 * k, UNLIT);
                continue;
            }
        };
        c.fill_round_rect(r.inset(-2.5 * k, -2.5 * k), 4.0 * k, color.alpha(0x1C));
        c.fill_round_rect(r, 2.0 * k, color);
    }

    let y0 = bar_y + bar_h + 16.0 * k;
    for i in 0..3 {
        let digit = frame.and_then(|f| f.digits[i]);
        let mask = digit.map_or(0, |d| DIGIT_SEGMENTS[d as usize]);
        let x = x0 + i as f32 * (digit_w + gap);
        draw_digit(c, x, y0, digit_w, digit_h, k, mask);
    }

    // Unit symbols, stacked next to the digits.
    let sx = x0 + 3.0 * digit_w + 2.0 * gap + 16.0 * k;
    let symbol = frame.map(|f| f.symbol);
    for (i, (label, s)) in [
        ("°C", Symbol::Celsius),
        ("°F", Symbol::Fahrenheit),
        ("%", Symbol::Percent),
    ]
    .into_iter()
    .enumerate()
    {
        let r = Rect::new(sx, y0 + i as f32 * 18.0 * k, symbols_w, 18.0 * k);
        let on = symbol == Some(s);
        if on {
            glow_text(c, label, r, 14.0 * k, k);
        }
        let color = if on { LIT } else { UNLIT };
        c.text(label, r, 14.0 * k, Weight::Semibold, color, Align::Left);
    }
}

/// A soft halo behind lit text, like the light bleeding through the cover.
fn glow_text(c: &Canvas, text: &str, r: Rect, size: f32, k: f32) {
    for (dx, dy) in HALO {
        let shifted = Rect::new(r.x + dx * k, r.y + dy * k, r.w, r.h);
        c.text(
            text,
            shifted,
            size,
            Weight::Semibold,
            LIT.alpha(0x1A),
            Align::Left,
        );
    }
}

/// Offsets of the copies that make up a glow.
const HALO: [(f32, f32); 8] = [
    (-1.6, 0.0),
    (1.6, 0.0),
    (0.0, -1.6),
    (0.0, 1.6),
    (-1.1, -1.1),
    (1.1, -1.1),
    (-1.1, 1.1),
    (1.1, 1.1),
];

fn draw_digit(c: &Canvas, x: f32, y: f32, w: f32, h: f32, k: f32, mask: u8) {
    let t = 5.0 * k; // segment thickness
    let half = h / 2.0;
    // Horizontal and vertical segments as hexagons.
    let hseg = |sx: f32, sy: f32| {
        vec![
            (sx + t / 2.0, sy),
            (sx + t, sy - t / 2.0),
            (sx + w - t, sy - t / 2.0),
            (sx + w - t / 2.0, sy),
            (sx + w - t, sy + t / 2.0),
            (sx + t, sy + t / 2.0),
        ]
    };
    let vseg = |sx: f32, sy: f32, len: f32| {
        vec![
            (sx, sy + t / 2.0),
            (sx + t / 2.0, sy + t),
            (sx + t / 2.0, sy + len - t),
            (sx, sy + len - t / 2.0),
            (sx - t / 2.0, sy + len - t),
            (sx - t / 2.0, sy + t),
        ]
    };
    let segments = [
        hseg(x, y),                  // a
        vseg(x + w, y, half),        // b
        vseg(x + w, y + half, half), // c
        hseg(x, y + h),              // d
        vseg(x, y + half, half),     // e
        vseg(x, y, half),            // f
        hseg(x, y + half),           // g
    ];
    for (i, points) in segments.iter().enumerate() {
        if mask & (1 << i) == 0 {
            c.fill_polygon(points, UNLIT);
            continue;
        }
        for (dx, dy) in HALO {
            let shifted: Vec<_> = points
                .iter()
                .map(|&(px, py)| (px + dx * k, py + dy * k))
                .collect();
            c.fill_polygon(&shifted, LIT.alpha(0x16));
        }
        c.fill_polygon(points, LIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coolercast_core::config::Config;

    fn status(shown: Shown, temp: f32, usage: f32) -> Status {
        Status {
            cpu_temp: Some(temp),
            cpu_usage: Some(usage),
            shown: Some(shown),
            config: Config::default(),
            ..Status::default()
        }
    }

    #[test]
    fn temperature_frame() {
        let f = Frame::from_status(&status(Shown::Temperature, 47.6, 3.0)).unwrap();
        assert_eq!(f.digits, [None, Some(4), Some(8)]);
        assert_eq!(f.symbol, Symbol::Celsius);
        assert_eq!(f.bar, 5);
    }

    #[test]
    fn usage_frame() {
        let f = Frame::from_status(&status(Shown::Usage, 47.0, 100.0)).unwrap();
        assert_eq!(f.digits, [Some(1), Some(0), Some(0)]);
        assert_eq!(f.symbol, Symbol::Percent);
        assert_eq!(f.bar, 10);
        let f = Frame::from_status(&status(Shown::Usage, 47.0, 0.2)).unwrap();
        assert_eq!(f.digits, [None, None, Some(0)]);
    }

    #[test]
    fn custom_frame_shows_the_config() {
        let mut s = status(Shown::Custom, 47.0, 3.0);
        s.config.custom_value = 7;
        s.config.custom_symbol = Symbol::Percent;
        s.config.custom_bar = 9;
        let f = Frame::from_status(&s).unwrap();
        assert_eq!(f.digits, [None, None, Some(7)]);
        assert_eq!(f.symbol, Symbol::Percent);
        assert_eq!(f.bar, 9);
    }

    #[test]
    fn power_mode_shows_the_temperature() {
        let f = Frame::from_status(&status(Shown::Power, 47.6, 3.0)).unwrap();
        assert_eq!(f.digits, [None, Some(4), Some(8)]);
        assert_eq!(f.symbol, Symbol::Celsius);
    }

    #[test]
    fn preview_only_for_ak_displays() {
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert!(applies_to(&[]));
        assert!(applies_to(&names(&["AK400 DIGITAL"])));
        assert!(applies_to(&names(&["LQ240/LQ360", "AK620 DIGITAL"])));
        assert!(!applies_to(&names(&["AK620 DIGITAL PRO"])));
        assert!(!applies_to(&names(&["LS520/LS720 SE DIGITAL"])));
    }

    #[test]
    fn gpu_component_shows_gpu_values() {
        let mut s = status(Shown::Temperature, 47.0, 3.0);
        s.component = Some(Component::Gpu);
        s.gpu.temp = Some(71.2);
        let f = Frame::from_status(&s).unwrap();
        assert_eq!(f.digits, [None, Some(7), Some(1)]);
    }

    #[test]
    fn usage_bar_with_the_temperature() {
        let mut s = status(Shown::Temperature, 47.6, 23.0);
        s.config.bar = Bar::Usage;
        let f = Frame::from_status(&s).unwrap();
        assert_eq!((f.digits, f.bar), ([None, Some(4), Some(8)], 2));
    }

    #[test]
    fn nothing_shown_yet() {
        assert_eq!(Frame::from_status(&Status::default()), None);
    }
}
