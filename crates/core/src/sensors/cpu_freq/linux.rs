//! CPU frequency on Linux: the average `scaling_cur_freq` of every online CPU, in kHz.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const CPUS: &str = "/sys/devices/system/cpu";

/// Reads the average CPU frequency in MHz.
pub struct CpuFreq {
    inputs: Vec<PathBuf>,
}

impl CpuFreq {
    pub fn open() -> io::Result<Self> {
        Self::open_in(Path::new(CPUS))
    }

    fn open_in(root: &Path) -> io::Result<Self> {
        let mut inputs: Vec<PathBuf> = fs::read_dir(root)?
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_prefix("cpu"))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .map(|e| e.path().join("cpufreq/scaling_cur_freq"))
            .filter(|p| p.is_file())
            .collect();
        if inputs.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no cpufreq driver (scaling_cur_freq) found",
            ));
        }
        inputs.sort();
        let mut sensor = Self { inputs };
        sensor.read()?;
        Ok(sensor)
    }

    pub fn read(&mut self) -> io::Result<f32> {
        let (mut sum, mut count) = (0u64, 0u64);
        // CPUs taken offline since `open` are skipped.
        for khz in self.inputs.iter().filter_map(|p| read_khz(p)) {
            sum += khz;
            count += 1;
        }
        if count == 0 {
            return Err(io::Error::other("no CPU frequency readable"));
        }
        Ok((sum as f64 / count as f64 / 1000.0) as f32)
    }

    /// Human-readable description of the sensor in use.
    pub fn source(&self) -> &'static str {
        "cpufreq scaling_cur_freq"
    }
}

fn read_khz(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_every_cpu() {
        let root = std::env::temp_dir().join(format!("coolercast-freq-{}", std::process::id()));
        let cpu = |name: &str, khz: Option<&str>| {
            let dir = root.join(name).join("cpufreq");
            fs::create_dir_all(&dir).unwrap();
            if let Some(khz) = khz {
                fs::write(dir.join("scaling_cur_freq"), khz).unwrap();
            }
        };
        cpu("cpu0", Some("4000000\n"));
        cpu("cpu1", Some("3000000\n"));
        cpu("cpu2", None);
        cpu("cpufreq", Some("999\n"));
        cpu("cpuidle", Some("999\n"));

        let freq = CpuFreq::open_in(&root).and_then(|mut f| f.read());
        let empty = CpuFreq::open_in(&root.join("cpu2"));
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(freq.unwrap(), 3500.0);
        assert!(empty.is_err());
    }
}
