//! Output reports for the AK series (AK400/AK500/AK500S/AK620 DIGITAL and their SE variants).
//!
//! Layout: `[report id, mode, bar, hundreds, tens, units, alarm, 0...]`, 64 bytes.
//! See `docs/protocol-ak-series.md` for details.

use super::Reading;
use crate::config::Unit;

pub const PACKET_LEN: usize = 64;

/// Report ID of the regular models. SE variants have no report ID (0 on Windows).
pub const REPORT_ID: u8 = 16;

const MODE_INIT: u8 = 170;
const MODE_CELSIUS: u8 = 19;
const MODE_FAHRENHEIT: u8 = 35;
const MODE_USAGE: u8 = 76;

pub type Packet = [u8; PACKET_LEN];

/// Start-up report: makes the display play its status bar animation.
pub fn init_packet(report_id: u8) -> Packet {
    let mut p = [0; PACKET_LEN];
    p[0] = report_id;
    p[1] = MODE_INIT;
    p
}

pub fn packet(report_id: u8, reading: Reading, alarm: bool) -> Packet {
    // The bar always follows the Celsius / percent scale so it means the same in both units.
    let (mode, value, bar) = match reading {
        Reading::Temperature { celsius, unit } => {
            let mode = match unit {
                Unit::Celsius => MODE_CELSIUS,
                Unit::Fahrenheit => MODE_FAHRENHEIT,
            };
            (mode, unit.from_celsius(celsius), celsius)
        }
        Reading::Usage { percent } => (MODE_USAGE, percent, percent),
    };
    let value = display_value(value);

    let mut p = [0; PACKET_LEN];
    p[0] = report_id;
    p[1] = mode;
    p[2] = bar_level(bar);
    p[3] = (value / 100) as u8;
    p[4] = (value / 10 % 10) as u8;
    p[5] = (value % 10) as u8;
    p[6] = alarm.into();
    p
}

/// Rounds a reading to what three digits can show.
pub fn display_value(value: f32) -> u16 {
    if value.is_nan() {
        0
    } else {
        value.round().clamp(0.0, 999.0) as u16
    }
}

/// Bar level 1–10: 1 below 15, otherwise the value divided by ten, rounded.
pub fn bar_level(value: f32) -> u8 {
    if value.is_nan() || value < 15.0 {
        1
    } else {
        (value / 10.0).round().clamp(1.0, 10.0) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(celsius: f32, unit: Unit) -> Reading {
        Reading::Temperature { celsius, unit }
    }

    fn head(p: &Packet) -> [u8; 7] {
        assert!(p[7..].iter().all(|&b| b == 0), "unused bytes must be zero");
        p[..7].try_into().unwrap()
    }

    #[test]
    fn init() {
        assert_eq!(head(&init_packet(REPORT_ID)), [16, 170, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn celsius() {
        let p = packet(REPORT_ID, temp(41.3, Unit::Celsius), false);
        assert_eq!(head(&p), [16, 19, 4, 0, 4, 1, 0]);
    }

    #[test]
    fn fahrenheit_uses_celsius_bar() {
        // 41 °C = 105.8 °F
        let p = packet(REPORT_ID, temp(41.0, Unit::Fahrenheit), false);
        assert_eq!(head(&p), [16, 35, 4, 1, 0, 6, 0]);
    }

    #[test]
    fn usage() {
        let p = packet(REPORT_ID, Reading::Usage { percent: 7.4 }, false);
        assert_eq!(head(&p), [16, 76, 1, 0, 0, 7, 0]);
        let p = packet(REPORT_ID, Reading::Usage { percent: 100.0 }, false);
        assert_eq!(head(&p), [16, 76, 10, 1, 0, 0, 0]);
    }

    #[test]
    fn alarm() {
        let p = packet(REPORT_ID, temp(92.0, Unit::Celsius), true);
        assert_eq!(head(&p), [16, 19, 9, 0, 9, 2, 1]);
    }

    #[test]
    fn se_variant_has_no_report_id() {
        let p = packet(0, temp(41.0, Unit::Celsius), false);
        assert_eq!(head(&p), [0, 19, 4, 0, 4, 1, 0]);
    }

    #[test]
    fn values_are_clamped_to_three_digits() {
        assert_eq!(
            head(&packet(16, Reading::Usage { percent: -3.0 }, false))[3..6],
            [0, 0, 0]
        );
        assert_eq!(
            head(&packet(16, Reading::Usage { percent: 1500.0 }, false))[3..6],
            [9, 9, 9]
        );
        assert_eq!(
            head(&packet(16, Reading::Usage { percent: f32::NAN }, false))[2..6],
            [1, 0, 0, 0]
        );
    }

    #[test]
    fn bar_levels() {
        let cases = [
            (0.0, 1),
            (14.9, 1),
            (15.0, 2),
            (44.0, 4),
            (45.0, 5),
            (95.0, 10),
            (100.0, 10),
            (250.0, 10),
        ];
        for (value, level) in cases {
            assert_eq!(bar_level(value), level, "value {value}");
        }
    }
}
