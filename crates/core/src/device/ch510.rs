//! Output reports for the CH510 MESH DIGITAL case display: the ASCII text
//! `HLXDATA(<usage>,<temperature>,0,0,<C|F>)\r\n` after the report ID. The usage drives the bar
//! and the temperature the three digits. See `docs/protocol-other-series.md`.

use std::io::Write;

use super::Readings;
use super::ak::{self, PACKET_LEN, Packet};
use crate::config::Unit;

pub fn packet(report_id: u8, readings: &Readings, unit: Unit) -> Packet {
    let usage = readings.cpu_usage.round().clamp(0.0, 100.0) as u8;
    let temperature = readings
        .cpu_temp
        .map_or(0, |celsius| ak::display_value(unit.from_celsius(celsius)));
    let unit = match unit {
        Unit::Celsius => 'C',
        Unit::Fahrenheit => 'F',
    };
    let mut p = [0; PACKET_LEN];
    p[0] = report_id;
    // At most 25 bytes, so writing into the 63 free bytes cannot fail.
    let _ = write!(&mut p[1..], "HLXDATA({usage},{temperature},0,0,{unit})\r\n");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(p: &Packet) -> &str {
        let end = p
            .iter()
            .skip(1)
            .position(|&b| b == 0)
            .map_or(p.len(), |n| n + 1);
        std::str::from_utf8(&p[1..end]).unwrap()
    }

    fn readings(cpu_temp: Option<f32>, cpu_usage: f32) -> Readings {
        Readings {
            cpu_temp,
            cpu_usage,
            ..Readings::default()
        }
    }

    #[test]
    fn celsius() {
        let p = packet(0, &readings(Some(36.4), 29.6), Unit::Celsius);
        assert_eq!(p[0], 0);
        assert_eq!(text(&p), "HLXDATA(30,36,0,0,C)\r\n");
    }

    #[test]
    fn fahrenheit_and_full_usage() {
        // 93.3 °C = 199.94 °F
        let p = packet(16, &readings(Some(93.3), 100.0), Unit::Fahrenheit);
        assert_eq!(p[0], 16);
        assert_eq!(text(&p), "HLXDATA(100,200,0,0,F)\r\n");
    }

    #[test]
    fn missing_temperature_is_zero() {
        let p = packet(0, &readings(None, 3.0), Unit::Celsius);
        assert_eq!(text(&p), "HLXDATA(3,0,0,0,C)\r\n");
    }
}
