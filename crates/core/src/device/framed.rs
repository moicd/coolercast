//! Framed status reports of the LD series, the AK DIGITAL PRO models and the LQ family (LQ240/
//! LQ360, ASSASSIN IV VC VISION, AK G2 and AK700 DIGITAL NYX).
//!
//! Each report is `[report id, header..., power, unit, temperature, usage, (frequency),
//! checksum, 22, 0...]`, 64 bytes, with multi-byte values in big-endian order. These displays
//! show every value at once, so the configured mode does not apply.
//! See `docs/protocol-other-series.md`.

use super::Readings;
use super::ak::{PACKET_LEN, Packet};
use crate::config::Unit;

/// Last byte of every frame.
const TERMINATOR: u8 = 22;

/// The fixed header and the optional fields of one model's report.
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    header: &'static [u8],
    frequency: bool,
}

pub const LD: Layout = Layout {
    header: &[104, 1, 1, 11, 1, 2, 5],
    frequency: false,
};

pub const AK400_PRO: Layout = Layout {
    header: &[104, 1, 2, 11, 1, 2, 5],
    frequency: false,
};

/// AK500 and AK620 DIGITAL PRO.
pub const AK620_PRO: Layout = Layout {
    header: &[104, 1, 4, 13, 1, 2, 8],
    frequency: true,
};

pub const LQ: Layout = Layout {
    header: &[104, 1, 8, 12, 1, 2],
    frequency: true,
};

/// The values of a report, already converted to what the display expects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Telemetry {
    /// CPU package power in watts.
    pub power: u16,
    pub unit: Unit,
    /// CPU temperature in `unit`, rounded to whole degrees.
    pub temperature: f32,
    /// CPU usage in percent.
    pub usage: u8,
    /// CPU frequency in MHz.
    pub frequency: u16,
}

impl Telemetry {
    /// Missing sensors are sent as 0.
    pub fn new(readings: &Readings, unit: Unit) -> Self {
        // `as` saturates and maps NaN to 0.
        let whole = |value: f32| value.round() as u16;
        Self {
            power: readings.cpu_power.map_or(0, whole),
            unit,
            temperature: readings
                .cpu_temp
                .map_or(0.0, |celsius| unit.from_celsius(celsius).round()),
            usage: readings.cpu_usage.round().clamp(0.0, 100.0) as u8,
            frequency: readings.cpu_freq.map_or(0, whole),
        }
    }
}

pub fn packet(report_id: u8, layout: &Layout, t: &Telemetry) -> Packet {
    let mut frame = Frame::new(report_id);
    frame.put(layout.header);
    frame.put(&t.power.to_be_bytes());
    frame.put(&[match t.unit {
        Unit::Celsius => 0,
        Unit::Fahrenheit => 1,
    }]);
    frame.put(&t.temperature.to_be_bytes());
    frame.put(&[t.usage]);
    if layout.frequency {
        frame.put(&t.frequency.to_be_bytes());
    }
    frame.close()
}

/// The two reports the LD series expects after connecting: the second one turns leading zeros
/// off.
pub fn ld_init_packets(report_id: u8) -> [Packet; 2] {
    [&[104, 1, 1, 2, 3, 1][..], &[104, 1, 1, 2, 2, 0]].map(|body| {
        let mut frame = Frame::new(report_id);
        frame.put(body);
        frame.close()
    })
}

/// Sum of the bytes modulo 256.
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |sum, &b| sum.wrapping_add(b))
}

/// A report being filled from byte 1 on.
struct Frame {
    packet: Packet,
    len: usize,
}

impl Frame {
    fn new(report_id: u8) -> Self {
        let mut packet = [0; PACKET_LEN];
        packet[0] = report_id;
        Self { packet, len: 1 }
    }

    fn put(&mut self, bytes: &[u8]) {
        self.packet[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }

    /// Appends the checksum of everything after the report ID, and the terminator.
    fn close(mut self) -> Packet {
        let sum = checksum(&self.packet[1..self.len]);
        self.put(&[sum, TERMINATOR]);
        self.packet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT_ID: u8 = 16;

    /// 0x01F4 W, 0x42280000 = 42.0 in f32, 0x1068 = 4200 MHz.
    const T: Telemetry = Telemetry {
        power: 500,
        unit: Unit::Celsius,
        temperature: 42.0,
        usage: 37,
        frequency: 4200,
    };

    fn assert_frame(p: &Packet, expected: &[u8]) {
        assert_eq!(&p[..expected.len()], expected);
        assert!(
            p[expected.len()..].iter().all(|&b| b == 0),
            "unused bytes must be zero"
        );
        let n = expected.len();
        assert_eq!(p[n - 1], TERMINATOR);
        assert_eq!(p[n - 2], checksum(&p[1..n - 2]));
    }

    #[test]
    fn ld_status() {
        // Checksum: 104+1+1+11+1+2+5 + 1+244 + 0 + 66+40+0+0 + 37 = 513 → 1.
        assert_frame(
            &packet(REPORT_ID, &LD, &T),
            &[
                16, 104, 1, 1, 11, 1, 2, 5, 1, 244, 0, 66, 40, 0, 0, 37, 1, 22,
            ],
        );
    }

    #[test]
    fn ld_init() {
        let [first, second] = ld_init_packets(REPORT_ID);
        assert_frame(&first, &[16, 104, 1, 1, 2, 3, 1, 112, 22]);
        assert_frame(&second, &[16, 104, 1, 1, 2, 2, 0, 110, 22]);
    }

    #[test]
    fn ak400_pro_status() {
        assert_frame(
            &packet(REPORT_ID, &AK400_PRO, &T),
            &[
                16, 104, 1, 2, 11, 1, 2, 5, 1, 244, 0, 66, 40, 0, 0, 37, 2, 22,
            ],
        );
    }

    #[test]
    fn ak620_pro_status_has_the_frequency() {
        // 104+1+4+13+1+2+8 + 245 + 106 + 37 + 16+104 = 641 → 129.
        assert_frame(
            &packet(REPORT_ID, &AK620_PRO, &T),
            &[
                16, 104, 1, 4, 13, 1, 2, 8, 1, 244, 0, 66, 40, 0, 0, 37, 16, 104, 129, 22,
            ],
        );
    }

    #[test]
    fn lq_status_has_a_shorter_header() {
        // 104+1+8+12+1+2 + 245 + 106 + 37 + 120 = 636 → 124.
        assert_frame(
            &packet(REPORT_ID, &LQ, &T),
            &[
                16, 104, 1, 8, 12, 1, 2, 1, 244, 0, 66, 40, 0, 0, 37, 16, 104, 124, 22,
            ],
        );
    }

    #[test]
    fn fahrenheit_sets_the_unit_byte() {
        let readings = Readings {
            cpu_temp: Some(41.0),
            cpu_usage: 0.0,
            cpu_power: None,
            cpu_freq: None,
        };
        let t = Telemetry::new(&readings, Unit::Fahrenheit);
        // 41 °C = 105.8 °F → 106.0 = 0x42D40000.
        let p = packet(REPORT_ID, &LD, &t);
        assert_eq!(p[10..15], [1, 0x42, 0xD4, 0, 0]);
    }

    #[test]
    fn telemetry_rounds_and_clamps() {
        let readings = Readings {
            cpu_temp: Some(47.6),
            cpu_usage: 99.7,
            cpu_power: Some(87.5),
            cpu_freq: Some(4699.6),
        };
        assert_eq!(
            Telemetry::new(&readings, Unit::Celsius),
            Telemetry {
                power: 88,
                unit: Unit::Celsius,
                temperature: 48.0,
                usage: 100,
                frequency: 4700,
            }
        );
        let odd = Readings {
            cpu_temp: None,
            cpu_usage: f32::NAN,
            cpu_power: Some(-3.0),
            cpu_freq: Some(1e9),
        };
        let t = Telemetry::new(&odd, Unit::Celsius);
        assert_eq!(
            (t.power, t.temperature, t.usage, t.frequency),
            (0, 0.0, 0, u16::MAX)
        );
    }

    #[test]
    fn checksum_wraps() {
        assert_eq!(checksum(&[200, 100]), 44);
        assert_eq!(checksum(&[]), 0);
    }
}
