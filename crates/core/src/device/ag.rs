//! Output reports for the AG series (AG400/AG620 DIGITAL).
//!
//! Layout: `[report id, mode, 0, tens, units, alarm, 0...]`, 64 bytes. Two digits, no bar and
//! no Fahrenheit symbol: values of 100 and above show 99. See `docs/protocol-other-series.md`.

use super::Reading;
use super::ak::{self, PACKET_LEN, Packet};
use crate::config::Symbol;

pub fn packet(report_id: u8, reading: Reading, alarm: bool) -> Packet {
    let (mode, value) = match reading {
        // Always Celsius: the display has no Fahrenheit symbol.
        Reading::Temperature { celsius, .. } => (ak::MODE_CELSIUS, ak::display_value(celsius)),
        Reading::Usage { percent } => (ak::MODE_USAGE, ak::display_value(percent)),
        // There is no power symbol; `Update::reading_for` sends the temperature instead.
        Reading::Power { watts } => (ak::MODE_USAGE, ak::display_value(watts)),
        Reading::Custom { value, symbol, .. } => {
            let mode = match symbol {
                Symbol::Celsius | Symbol::Fahrenheit => ak::MODE_CELSIUS,
                Symbol::Percent => ak::MODE_USAGE,
            };
            (mode, value)
        }
    };
    let value = value.min(99) as u8;
    let mut p = [0; PACKET_LEN];
    p[0] = report_id;
    p[1] = mode;
    p[3] = value / 10;
    p[4] = value % 10;
    p[5] = alarm.into();
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Unit;

    const REPORT_ID: u8 = 16;

    fn head(p: &Packet) -> [u8; 6] {
        assert!(p[6..].iter().all(|&b| b == 0), "unused bytes must be zero");
        p[..6].try_into().unwrap()
    }

    #[test]
    fn temperature_is_always_celsius() {
        let reading = |unit| Reading::Temperature {
            celsius: 47.6,
            unit,
        };
        let expected = [16, 19, 0, 4, 8, 0];
        assert_eq!(
            head(&packet(REPORT_ID, reading(Unit::Celsius), false)),
            expected
        );
        assert_eq!(
            head(&packet(REPORT_ID, reading(Unit::Fahrenheit), false)),
            expected
        );
    }

    #[test]
    fn usage_with_alarm() {
        let reading = Reading::Usage { percent: 7.0 };
        assert_eq!(
            head(&packet(REPORT_ID, reading, true)),
            [16, 76, 0, 0, 7, 1]
        );
    }

    #[test]
    fn two_digits_at_most() {
        let reading = Reading::Usage { percent: 100.0 };
        assert_eq!(
            head(&packet(REPORT_ID, reading, false)),
            [16, 76, 0, 9, 9, 0]
        );
        let custom = Reading::Custom {
            value: 512,
            symbol: Symbol::Fahrenheit,
            bar: 3,
        };
        assert_eq!(
            head(&packet(REPORT_ID, custom, false)),
            [16, 19, 0, 9, 9, 0]
        );
    }
}
