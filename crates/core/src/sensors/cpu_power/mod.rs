//! CPU package power in watts, from the RAPL energy counter: the package energy MSR through
//! PawnIO on Windows, powercap on Linux. Power is the energy used between two reads divided by
//! the time between them, over windows of at least [`MIN_WINDOW`].

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

/// Shortest time power is averaged over. The energy counters advance about once a millisecond,
/// so over a much shorter window the energy is 0 or one step, which reads as 0 W or a spike of
/// kilowatts. Regular refreshes are at least 250 ms apart and never affected; the one right after
/// a sensor is opened, or a second one triggered by a wake (unlock, display on), can be.
const MIN_WINDOW: Duration = Duration::from_millis(100);

/// Reads the CPU package power.
pub struct CpuPower {
    counter: Counter,
    average: Average,
}

impl CpuPower {
    pub fn open() -> io::Result<Self> {
        let mut sensor = Self {
            counter: Counter::open()?,
            average: Average::default(),
        };
        sensor.read()?;
        Ok(sensor)
    }

    /// Average power over the last window of at least [`MIN_WINDOW`], `None` until the first one
    /// has passed.
    pub fn read(&mut self) -> io::Result<Option<f32>> {
        let count = self.counter.read()?;
        Ok(self.average.update(
            count,
            self.counter.range(),
            self.counter.joules_per_count(),
            Instant::now(),
        ))
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

/// Average power, `None` when `elapsed` is shorter than [`MIN_WINDOW`].
pub fn watts(joules: f64, elapsed: Duration) -> Option<f32> {
    (elapsed >= MIN_WINDOW).then(|| (joules / elapsed.as_secs_f64()) as f32)
}

/// Average power from an energy counter that is read at irregular times.
#[derive(Debug, Default, PartialEq)]
pub struct Average {
    /// Counter value and time at the start of the current window.
    start: Option<(u64, Instant)>,
    /// Power over the last complete window.
    watts: Option<f32>,
}

impl Average {
    /// Takes the counter value `count` read at `now`, for a counter of `joules_per_count` joules
    /// per step that wraps around to 0 after `range - 1`. Returns the power over the last window
    /// of at least [`MIN_WINDOW`]. A read that comes sooner leaves the window open and repeats
    /// the previous power; before the first complete window there is none.
    pub fn update(
        &mut self,
        count: u64,
        range: u64,
        joules_per_count: f64,
        now: Instant,
    ) -> Option<f32> {
        let Some((first, at)) = self.start else {
            self.start = Some((count, now));
            return None;
        };
        let joules = counter_delta(first, count, range) as f64 * joules_per_count;
        if let Some(watts) = watts(joules, now - at) {
            self.start = Some((count, now));
            self.watts = Some(watts);
        }
        self.watts
    }
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
        assert_eq!(watts(10.0, MIN_WINDOW), Some(100.0));
    }

    #[test]
    fn no_power_over_a_window_that_is_too_short() {
        assert_eq!(watts(1.0, Duration::ZERO), None);
        assert_eq!(watts(65.0, MIN_WINDOW - Duration::from_nanos(1)), None);
    }

    /// µJ counter, as read from powercap.
    fn feed(average: &mut Average, count: u64, start: Instant, after: Duration) -> Option<f32> {
        average.update(count, u64::MAX, 1e-6, start + after)
    }

    #[test]
    fn average_has_no_power_right_after_opening() {
        let (mut average, t0) = (Average::default(), Instant::now());
        let micros = Duration::from_micros;
        assert_eq!(feed(&mut average, 5_000, t0, Duration::ZERO), None);
        // The counter has not ticked yet, or ticked once: 0 W or a spike without the minimum.
        assert_eq!(feed(&mut average, 5_000, t0, micros(30)), None);
        assert_eq!(feed(&mut average, 70_000, t0, micros(40)), None);
    }

    #[test]
    fn average_uses_the_whole_window_since_opening() {
        let (mut average, t0) = (Average::default(), Instant::now());
        let secs = Duration::from_secs;
        feed(&mut average, 5_000, t0, Duration::ZERO);
        feed(&mut average, 70_000, t0, Duration::from_micros(40));
        // 65 J since the first read, one second ago.
        assert_eq!(feed(&mut average, 65_005_000, t0, secs(1)), Some(65.0));
    }

    #[test]
    fn average_repeats_the_last_power_when_read_again_at_once() {
        let (mut average, t0) = (Average::default(), Instant::now());
        let (secs, millis) = (Duration::from_secs, Duration::from_millis);
        feed(&mut average, 0, t0, Duration::ZERO);
        assert_eq!(feed(&mut average, 65_000_000, t0, secs(1)), Some(65.0));
        // A wake refreshes 3 ms later, just as the counter ticked.
        assert_eq!(
            feed(&mut average, 130_000_000, t0, secs(1) + millis(3)),
            Some(65.0)
        );
        // The short read did not move the window: 70 J in the second after the 65 W reading.
        assert_eq!(feed(&mut average, 135_000_000, t0, secs(2)), Some(70.0));
    }

    #[test]
    fn average_handles_the_counter_wrapping() {
        let (mut average, t0) = (Average::default(), Instant::now());
        feed(&mut average, u64::MAX - 1_000_000, t0, Duration::ZERO);
        // 10 J across the wrap, in 2 s.
        let watts = feed(&mut average, 9_000_000, t0, Duration::from_secs(2));
        assert_eq!(watts, Some(5.0));
    }
}
