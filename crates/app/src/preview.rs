//! Simulated cooler display: three seven-segment digits, the unit symbols and the 10-step bar,
//! lit the same way as the real one.

use coolercast_core::config::{Config, Symbol, Unit};
use coolercast_core::device::ak;
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

impl Frame {
    pub fn from_status(status: &Status) -> Option<Self> {
        let alarm = status.alarm_active;
        let (value, bar, symbol) = match (status.shown?, status.cpu_temp) {
            (Shown::Custom, _) => return Some(Self::custom(&status.config, alarm)),
            (Shown::Temperature, Some(t)) => {
                let unit = status.config.unit;
                let symbol = match unit {
                    Unit::Celsius => Symbol::Celsius,
                    Unit::Fahrenheit => Symbol::Fahrenheit,
                };
                (unit.from_celsius(t), t, symbol)
            }
            _ => {
                let usage = status.cpu_usage?;
                (usage, usage, Symbol::Percent)
            }
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

pub fn draw(c: &Canvas, area: Rect, frame: Option<Frame>) {
    c.fill_round_rect(area, 10.0, PANEL);
    // Reflection on the cover glass, over the top of the panel.
    let white = Color::rgb(0xFF, 0xFF, 0xFF);
    let sheen = Rect::new(area.x, area.y, area.w, area.h * 0.5);
    c.fill_round_rect_v(sheen, 10.0, white.alpha(0x14), white.alpha(0x00));

    let digit_w = 30.0;
    let digit_h = 54.0;
    let gap = 10.0;
    let symbols_w = 40.0;
    let total_w = 3.0 * digit_w + 2.0 * gap + 16.0 + symbols_w;
    let x0 = area.x + (area.w - total_w) / 2.0;
    let y0 = area.y + 16.0;

    let lit = |on: bool| if on { LIT } else { UNLIT };
    for i in 0..3 {
        let digit = frame.and_then(|f| f.digits[i]);
        let mask = digit.map_or(0, |d| DIGIT_SEGMENTS[d as usize]);
        draw_digit(
            c,
            x0 + i as f32 * (digit_w + gap),
            y0,
            digit_w,
            digit_h,
            mask,
            lit,
        );
    }

    // Unit symbols, stacked next to the digits.
    let sx = x0 + 3.0 * digit_w + 2.0 * gap + 16.0;
    let symbol = frame.map(|f| f.symbol);
    for (i, (label, s)) in [
        ("°C", Symbol::Celsius),
        ("°F", Symbol::Fahrenheit),
        ("%", Symbol::Percent),
    ]
    .into_iter()
    .enumerate()
    {
        let r = Rect::new(sx, y0 + i as f32 * 18.0, symbols_w, 18.0);
        c.text(
            label,
            r,
            14.0,
            Weight::Semibold,
            lit(symbol == Some(s)),
            Align::Left,
        );
    }

    // Bar: ten segments under the digits, red while the alarm is on.
    let alarm = frame.is_some_and(|f| f.alarm);
    let level = frame.map_or(0, |f| f.bar);
    let bar_y = y0 + digit_h + 16.0;
    let seg_gap = 4.0;
    let seg_w = (total_w - 9.0 * seg_gap) / 10.0;
    for i in 0..10u8 {
        let on = i < level;
        let color = match (on, alarm) {
            (true, true) => ALARM,
            (true, false) => LIT,
            _ => UNLIT,
        };
        let r = Rect::new(x0 + f32::from(i) * (seg_w + seg_gap), bar_y, seg_w, 8.0);
        c.fill_round_rect(r, 2.0, color);
    }
}

fn draw_digit(c: &Canvas, x: f32, y: f32, w: f32, h: f32, mask: u8, lit: impl Fn(bool) -> Color) {
    let t = 5.0; // segment thickness
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
        c.fill_polygon(points, lit(mask & (1 << i) != 0));
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
    fn nothing_shown_yet() {
        assert_eq!(Frame::from_status(&Status::default()), None);
    }
}
