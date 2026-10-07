//! Output reports for the LS series (LS520/LS720 SE DIGITAL).
//!
//! Same layout as the AK series, `[report id, mode, bar, hundreds, tens, units, alarm, 0...]`,
//! but mode 76 shows the CPU power in watts instead of the usage, and the bar always follows the
//! CPU usage. See `docs/protocol-other-series.md`.

use super::Reading;
use super::ak::{self, Packet};

const MODE_POWER: u8 = ak::MODE_USAGE;

/// A status report. The start-up report is the AK one ([`ak::init_packet`]).
pub fn packet(report_id: u8, reading: Reading, cpu_usage: f32, alarm: bool) -> Packet {
    let usage_bar = ak::bar_level(cpu_usage);
    let (mode, value, bar) = match reading {
        Reading::Temperature { celsius, unit } => (
            ak::unit_mode(unit),
            ak::display_value(unit.from_celsius(celsius)),
            usage_bar,
        ),
        Reading::Power { watts } => (MODE_POWER, ak::display_value(watts), usage_bar),
        // There is no usage symbol; `Update::reading_for` sends something else instead.
        Reading::Usage { percent } => (MODE_POWER, ak::display_value(percent), usage_bar),
        // Its percent mode is the power one.
        Reading::Custom { value, symbol, bar } => ak::custom(value, symbol, bar),
    };
    ak::raw_packet(report_id, ak::payload(mode, bar, value, alarm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Symbol, Unit};

    const REPORT_ID: u8 = 16;

    fn head(p: &Packet) -> [u8; 7] {
        assert!(p[7..].iter().all(|&b| b == 0), "unused bytes must be zero");
        p[..7].try_into().unwrap()
    }

    #[test]
    fn temperature_with_the_usage_bar() {
        let reading = Reading::Temperature {
            celsius: 52.4,
            unit: Unit::Celsius,
        };
        assert_eq!(
            head(&packet(REPORT_ID, reading, 73.0, false)),
            [16, 19, 7, 0, 5, 2, 0]
        );
    }

    #[test]
    fn fahrenheit() {
        // 41 °C = 105.8 °F
        let reading = Reading::Temperature {
            celsius: 41.0,
            unit: Unit::Fahrenheit,
        };
        assert_eq!(
            head(&packet(REPORT_ID, reading, 5.0, true)),
            [16, 35, 1, 1, 0, 6, 1]
        );
    }

    #[test]
    fn power_in_watts() {
        let reading = Reading::Power { watts: 142.6 };
        assert_eq!(
            head(&packet(REPORT_ID, reading, 100.0, false)),
            [16, 76, 10, 1, 4, 3, 0]
        );
    }

    #[test]
    fn custom_value_keeps_its_bar() {
        let reading = Reading::Custom {
            value: 1234,
            symbol: Symbol::Percent,
            bar: 0,
        };
        assert_eq!(
            head(&packet(REPORT_ID, reading, 50.0, false)),
            [16, 76, 1, 9, 9, 9, 0]
        );
    }
}
