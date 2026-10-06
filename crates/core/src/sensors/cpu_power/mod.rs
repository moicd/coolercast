//! CPU package power in watts, from the RAPL energy counter: the package energy MSR through
//! PawnIO on Windows, powercap on Linux. Power is the energy used between two reads divided by
//! the time between them.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::Counter;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::Counter;

use std::io;
use std::time::{Duration, Instant};

/// Reads the CPU package power.
pub struct CpuPower {
    counter: Counter,
    last: Option<(u64, Instant)>,
}

impl CpuPower {
    pub fn open() -> io::Result<Self> {
        let mut sensor = Self {
            counter: Counter::open()?,
            last: None,
        };
        sensor.read()?;
        Ok(sensor)
    }

    /// Average power since the previous read, `None` on the first one.
    pub fn read(&mut self) -> io::Result<Option<f32>> {
        let value = self.counter.read()?;
        let now = Instant::now();
        let watts = self.last.and_then(|(previous, at)| {
            let counts = counter_delta(previous, value, self.counter.range());
            watts(counts as f64 * self.counter.joules_per_count(), now - at)
        });
        self.last = Some((value, now));
        Ok(watts)
    }

    /// Human-readable description of the counter in use.
    pub fn source(&self) -> &str {
        self.counter.source()
    }
}

/// Counts between two reads of a counter that wraps around to 0 after `range - 1`.
pub fn counter_delta(previous: u64, current: u64, range: u64) -> u64 {
    if current >= previous {
        current - previous
    } else {
        range.saturating_sub(previous) + current
    }
}

/// Average power, `None` when no time has passed.
pub fn watts(joules: f64, elapsed: Duration) -> Option<f32> {
    let seconds = elapsed.as_secs_f64();
    (seconds > 0.0).then(|| (joules / seconds) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_without_wrap() {
        assert_eq!(counter_delta(1_000, 4_500, 1 << 32), 3_500);
        assert_eq!(counter_delta(7, 7, 1 << 32), 0);
    }

    #[test]
    fn delta_across_the_wrap() {
        // A 32-bit counter going from 0xFFFF_FF00 to 0x100 advanced by 0x200.
        assert_eq!(counter_delta(0xFFFF_FF00, 0x100, 1 << 32), 0x200);
    }

    #[test]
    fn power_from_energy_and_time() {
        assert_eq!(watts(65.0, Duration::from_millis(1000)), Some(65.0));
        assert_eq!(watts(30.0, Duration::from_millis(500)), Some(60.0));
        assert_eq!(watts(1.0, Duration::ZERO), None);
    }
}
