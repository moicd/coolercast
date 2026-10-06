//! CPU frequency on Windows, from the same PDH counters Task Manager uses:
//! `Processor Frequency` (the nominal MHz) × `% Processor Performance` / 100.
//!
//! `pdh.dll` is loaded at runtime, only when a connected display shows the frequency.

use std::io;

use crate::sensors::pdh::{Counter, PDH_FMT_NOCAP100, Query};

const PERFORMANCE: &str = r"\Processor Information(_Total)\% Processor Performance";
const FREQUENCY: &str = r"\Processor Information(_Total)\Processor Frequency";

/// Reads the average CPU frequency in MHz.
pub struct CpuFreq {
    query: Query,
    performance: Counter,
    frequency: Counter,
}

impl CpuFreq {
    pub fn open() -> io::Result<Self> {
        let mut query = Query::open()?;
        let performance = query.add(PERFORMANCE)?;
        let frequency = query.add(FREQUENCY)?;
        // Rate counters need two samples: this is the first one.
        query.collect()?;
        Ok(Self {
            query,
            performance,
            frequency,
        })
    }

    pub fn read(&mut self) -> io::Result<f32> {
        self.query.collect()?;
        let performance = self.query.value(self.performance, PDH_FMT_NOCAP100)?;
        let nominal = self.query.value(self.frequency, 0)?;
        Ok(mhz(performance, nominal))
    }

    /// Human-readable description of the sensor in use.
    pub fn source(&self) -> &'static str {
        "PDH Processor Information"
    }
}

/// Effective frequency from the performance percentage and the nominal frequency.
pub fn mhz(performance_percent: f64, nominal_mhz: f64) -> f32 {
    (nominal_mhz * performance_percent / 100.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_above_nominal() {
        assert_eq!(mhz(125.0, 3600.0), 4500.0);
        assert_eq!(mhz(0.0, 3600.0), 0.0);
    }

    #[test]
    fn live_reading() {
        let mut sensor = CpuFreq::open().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mhz = sensor.read().unwrap();
        assert!((100.0..10_000.0).contains(&mhz), "{mhz} MHz");
    }
}
