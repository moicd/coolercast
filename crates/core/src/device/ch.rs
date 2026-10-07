//! Output reports for the CH series cases (CH560 DIGITAL, CH360 DIGITAL and MORPHEUS): a CPU
//! section above and a GPU section below, each laid out like an AK series display.
//!
//! Layout: `[report id, CPU mode, CPU bar, CPU digits ×3, GPU mode, GPU bar, GPU digits ×3,
//! 0...]`, 64 bytes. There is no alarm byte, and each bar follows its component's usage.
//! The start-up report is the AK one. See `docs/protocol-other-series.md`.

use super::ak::{self, PACKET_LEN, Packet};
use super::{Component, Reading, Update};
use crate::config::Unit;
use crate::sensors::Values;

/// Both sections show the same kind of value: the usage when the configured mode shows the
/// usage, the temperature otherwise. A custom value goes to the CPU section.
pub fn packet(report_id: u8, update: &Update) -> Packet {
    let usage = matches!(update.reading, Reading::Usage { .. });
    let cpu = match update.reading {
        Reading::Custom { value, symbol, bar } => {
            let (mode, value, bar) = ak::custom(value, symbol, bar);
            section(mode, bar, value)
        }
        _ => component(&update.readings.of(Component::Cpu), usage, update.unit),
    };
    let gpu = component(&update.readings.of(Component::Gpu), usage, update.unit);
    let mut p = [0; PACKET_LEN];
    p[0] = report_id;
    p[1..6].copy_from_slice(&cpu);
    p[6..11].copy_from_slice(&gpu);
    p
}

/// One section: the usage if asked for or if there is no temperature, else the temperature.
/// A component without sensors shows 0 %.
fn component(values: &Values, prefer_usage: bool, unit: Unit) -> [u8; 5] {
    let usage = values.usage.unwrap_or(0.0);
    let bar = ak::bar_level(usage);
    match values.temp {
        Some(celsius) if !prefer_usage || values.usage.is_none() => section(
            ak::unit_mode(unit),
            bar,
            ak::display_value(unit.from_celsius(celsius)),
        ),
        _ => section(ak::MODE_USAGE, bar, ak::display_value(usage)),
    }
}

fn section(mode: u8, bar: u8, value: u16) -> [u8; 5] {
    let p = ak::payload(mode, bar, value, false);
    [p[0], p[1], p[2], p[3], p[4]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Symbol;
    use crate::device::Readings;

    const REPORT_ID: u8 = 16;

    fn update(reading: Reading, gpu: Values) -> Update {
        Update {
            reading,
            component: Component::Cpu,
            readings: Readings {
                cpu_temp: Some(52.0),
                cpu_usage: 23.0,
                gpu,
                ..Readings::default()
            },
            unit: Unit::Celsius,
            alarm: false,
            usage_bar: false,
        }
    }

    const GPU: Values = Values {
        temp: Some(68.4),
        usage: Some(97.0),
        power: None,
        freq: None,
    };

    const TEMP: Reading = Reading::Temperature {
        celsius: 52.0,
        unit: Unit::Celsius,
    };

    fn head(p: &Packet) -> [u8; 11] {
        assert!(p[11..].iter().all(|&b| b == 0), "unused bytes must be zero");
        p[..11].try_into().unwrap()
    }

    #[test]
    fn temperatures_with_usage_bars() {
        let p = packet(REPORT_ID, &update(TEMP, GPU));
        assert_eq!(head(&p), [16, 19, 2, 0, 5, 2, 19, 10, 0, 6, 8]);
    }

    #[test]
    fn usages() {
        let p = packet(REPORT_ID, &update(Reading::Usage { percent: 23.0 }, GPU));
        assert_eq!(head(&p), [16, 76, 2, 0, 2, 3, 76, 10, 0, 9, 7]);
    }

    #[test]
    fn fahrenheit() {
        let mut u = update(TEMP, GPU);
        u.unit = Unit::Fahrenheit;
        // 52 °C = 125.6 °F, 68.4 °C = 155.1 °F.
        assert_eq!(
            head(&packet(REPORT_ID, &u)),
            [16, 35, 2, 1, 2, 6, 35, 10, 1, 5, 5]
        );
    }

    #[test]
    fn custom_value_in_the_cpu_section() {
        let custom = Reading::Custom {
            value: 42,
            symbol: Symbol::Percent,
            bar: 7,
        };
        let p = packet(REPORT_ID, &update(custom, GPU));
        assert_eq!(head(&p), [16, 76, 7, 0, 4, 2, 19, 10, 0, 6, 8]);
    }

    #[test]
    fn gpu_without_sensors_shows_zero_percent() {
        let p = packet(REPORT_ID, &update(TEMP, Values::default()));
        assert_eq!(head(&p)[6..], [76, 1, 0, 0, 0]);
    }
}
