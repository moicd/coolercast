//! Total CPU usage: `GetSystemTimes` on Windows, `/proc/stat` on Linux.

use std::io;

/// Cumulative CPU time counters, in platform-specific units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    /// All time, idle included.
    pub total: u64,
}

impl CpuTimes {
    #[cfg(windows)]
    pub fn now() -> io::Result<Self> {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::GetSystemTimes;

        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut idle, mut kernel, mut user) = (zero, zero, zero);
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        // Kernel time includes idle time.
        Ok(Self {
            idle: ticks(idle),
            total: ticks(kernel) + ticks(user),
        })
    }

    #[cfg(target_os = "linux")]
    pub fn now() -> io::Result<Self> {
        let stat = std::fs::read_to_string("/proc/stat")?;
        Self::from_proc_stat(&stat)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unexpected /proc/stat"))
    }

    /// Parses the aggregate `cpu` line of `/proc/stat` (times in clock ticks).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn from_proc_stat(stat: &str) -> Option<Self> {
        let line = stat.lines().find(|l| l.starts_with("cpu "))?;
        let fields: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .map(|f| f.parse().ok())
            .collect::<Option<_>>()?;
        // user nice system idle iowait irq softirq steal; guest time is already in user.
        let time = |i: usize| fields.get(i).copied().unwrap_or(0);
        Some(Self {
            idle: time(3) + time(4),
            total: (0..8).map(time).sum(),
        })
    }

    /// Busy percentage between an earlier sample and this one.
    pub fn usage_since(&self, earlier: &CpuTimes) -> f32 {
        let idle = self.idle.saturating_sub(earlier.idle);
        let total = self.total.saturating_sub(earlier.total);
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
            total: 2_500,
        };
        // 1000 ticks elapsed, 750 of them idle.
        let b = CpuTimes {
            idle: 1_750,
            total: 3_500,
        };
        assert_eq!(b.usage_since(&a), 25.0);
    }

    #[test]
    fn no_elapsed_time_is_zero() {
        let a = CpuTimes { idle: 5, total: 13 };
        assert_eq!(a.usage_since(&a), 0.0);
    }

    #[test]
    fn proc_stat_line() {
        let stat = "cpu  100 5 50 800 20 3 2 1 9 0\ncpu0 50 2 25 400 10 1 1 0 0 0\nintr 1 2\n";
        assert_eq!(
            CpuTimes::from_proc_stat(stat),
            Some(CpuTimes {
                idle: 820,
                total: 981,
            })
        );
        assert_eq!(CpuTimes::from_proc_stat("intr 1 2\n"), None);
    }

    #[test]
    fn live_sample_is_a_percentage() {
        let mut meter = CpuUsage::new();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let usage = meter.sample();
        assert!((0.0..=100.0).contains(&usage), "{usage}");
    }
}
