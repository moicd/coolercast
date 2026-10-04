//! Total CPU usage from `GetSystemTimes`.

use std::io;

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Threading::GetSystemTimes;

/// Cumulative system CPU times, in 100 ns units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    /// Includes idle time.
    pub kernel: u64,
    pub user: u64,
}

impl CpuTimes {
    pub fn now() -> io::Result<Self> {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut idle, mut kernel, mut user) = (zero, zero, zero);
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        Ok(Self {
            idle: ticks(idle),
            kernel: ticks(kernel),
            user: ticks(user),
        })
    }

    /// Busy percentage between an earlier sample and this one.
    pub fn usage_since(&self, earlier: &CpuTimes) -> f32 {
        let idle = self.idle.saturating_sub(earlier.idle);
        let total =
            self.kernel.saturating_sub(earlier.kernel) + self.user.saturating_sub(earlier.user);
        if total == 0 {
            return 0.0;
        }
        (total.saturating_sub(idle) as f64 * 100.0 / total as f64) as f32
    }
}

/// Usage meter that reports the average since its previous sample.
#[derive(Debug, Default)]
pub struct CpuUsage {
    last: Option<CpuTimes>,
}

impl CpuUsage {
    pub fn new() -> Self {
        Self {
            last: CpuTimes::now().ok(),
        }
    }

    pub fn sample(&mut self) -> f32 {
        let Ok(now) = CpuTimes::now() else { return 0.0 };
        let usage = self.last.map_or(0.0, |last| now.usage_since(&last));
        self.last = Some(now);
        usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_between_samples() {
        let a = CpuTimes {
            idle: 1_000,
            kernel: 2_000,
            user: 500,
        };
        // 1000 ticks elapsed: kernel 800 (of which 750 idle) + user 200 → 250 busy.
        let b = CpuTimes {
            idle: 1_750,
            kernel: 2_800,
            user: 700,
        };
        assert_eq!(b.usage_since(&a), 25.0);
    }

    #[test]
    fn no_elapsed_time_is_zero() {
        let a = CpuTimes {
            idle: 5,
            kernel: 10,
            user: 3,
        };
        assert_eq!(a.usage_since(&a), 0.0);
    }

    #[test]
    fn live_sample_is_a_percentage() {
        let mut meter = CpuUsage::new();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let usage = meter.sample();
        assert!((0.0..=100.0).contains(&usage), "{usage}");
    }
}
