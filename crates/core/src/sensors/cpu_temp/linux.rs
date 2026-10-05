//! CPU temperature on Linux from the kernel's hwmon drivers (or a thermal zone as a fallback).
//! No privileges or extra drivers are needed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const HWMON: &str = "/sys/class/hwmon";
const THERMAL: &str = "/sys/class/thermal";

/// Reads the CPU temperature in °C.
pub struct CpuTemp {
    input: PathBuf,
    source: String,
}

impl CpuTemp {
    pub fn open() -> io::Result<Self> {
        let (input, source) = discover(Path::new(HWMON), Path::new(THERMAL)).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no CPU temperature sensor found (load the k10temp or coretemp driver)",
            )
        })?;
        let mut sensor = Self { input, source };
        sensor.read()?;
        Ok(sensor)
    }

    pub fn read(&mut self) -> io::Result<f32> {
        read_millidegrees(&self.input)
    }

    /// Human-readable description of the sensor in use.
    pub fn source(&self) -> &str {
        &self.source
    }
}

fn read_millidegrees(path: &Path) -> io::Result<f32> {
    let text = fs::read_to_string(path)?;
    let value: i64 = text
        .trim()
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid temperature"))?;
    Ok(value as f32 / 1000.0)
}

/// hwmon drivers that report the CPU, best first, with the preferred sensor labels.
const DRIVERS: &[(&str, &[&str])] = &[
    ("k10temp", &["Tctl", "Tdie"]),
    ("zenpower", &["Tctl", "Tdie"]),
    ("coretemp", &["Package id 0"]),
    ("cpu_thermal", &[]),
    ("cpu-thermal", &[]),
    ("soc_thermal", &[]),
];

/// Thermal zone types that describe the CPU, used when no hwmon driver matches.
const ZONES: &[&str] = &[
    "x86_pkg_temp",
    "cpu-thermal",
    "cpu_thermal",
    "soc-thermal",
    "cpu",
];

/// Finds the best CPU temperature input under the given sysfs roots.
fn discover(hwmon: &Path, thermal: &Path) -> Option<(PathBuf, String)> {
    let mut best: Option<(usize, PathBuf, String)> = None;
    for dir in sorted_dirs(hwmon) {
        let Ok(name) = fs::read_to_string(dir.join("name")) else {
            continue;
        };
        let name = name.trim();
        let Some(rank) = DRIVERS.iter().position(|(driver, _)| *driver == name) else {
            continue;
        };
        if best.as_ref().is_some_and(|(r, ..)| *r <= rank) {
            continue;
        }
        let labels = DRIVERS[rank].1;
        if let Some((input, label)) = pick_input(&dir, labels) {
            let source = match label {
                Some(label) => format!("{name} {label}"),
                None => name.to_owned(),
            };
            best = Some((rank, input, source));
        }
    }
    if let Some((_, input, source)) = best {
        return Some((input, source));
    }

    for dir in sorted_dirs(thermal) {
        let Ok(kind) = fs::read_to_string(dir.join("type")) else {
            continue;
        };
        let kind = kind.trim();
        let input = dir.join("temp");
        if ZONES.contains(&kind) && input.is_file() {
            return Some((input, format!("thermal zone {kind}")));
        }
    }
    None
}

/// The `tempN_input` whose label is first in `labels`, or `temp1_input`.
fn pick_input(dir: &Path, labels: &[&str]) -> Option<(PathBuf, Option<String>)> {
    for wanted in labels {
        for n in 1..=32 {
            let label = dir.join(format!("temp{n}_label"));
            if fs::read_to_string(&label).is_ok_and(|l| l.trim() == *wanted) {
                let input = dir.join(format!("temp{n}_input"));
                if input.is_file() {
                    return Some((input, Some((*wanted).to_owned())));
                }
            }
        }
    }
    let input = dir.join("temp1_input");
    input.is_file().then_some((input, None))
}

fn sorted_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    dirs.sort();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("coolercast-temp-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            Self(root)
        }

        fn file(&self, path: &str, content: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn discover(&self) -> Option<(PathBuf, String)> {
            discover(&self.0.join("hwmon"), &self.0.join("thermal"))
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn prefers_amd_tctl() {
        let t = Tree::new("amd");
        t.file("hwmon/hwmon0/name", "nvme\n");
        t.file("hwmon/hwmon0/temp1_input", "38000\n");
        t.file("hwmon/hwmon1/name", "k10temp\n");
        t.file("hwmon/hwmon1/temp1_label", "Tctl\n");
        t.file("hwmon/hwmon1/temp1_input", "45250\n");
        t.file("hwmon/hwmon1/temp3_label", "Tccd1\n");
        t.file("hwmon/hwmon1/temp3_input", "41000\n");

        let (input, source) = t.discover().unwrap();
        assert_eq!(source, "k10temp Tctl");
        assert_eq!(read_millidegrees(&input).unwrap(), 45.25);
    }

    #[test]
    fn intel_package_sensor() {
        let t = Tree::new("intel");
        t.file("hwmon/hwmon2/name", "coretemp\n");
        t.file("hwmon/hwmon2/temp2_label", "Core 0\n");
        t.file("hwmon/hwmon2/temp2_input", "40000\n");
        t.file("hwmon/hwmon2/temp1_label", "Package id 0\n");
        t.file("hwmon/hwmon2/temp1_input", "47000\n");
        t.file("hwmon/hwmon5/name", "cpu_thermal\n");
        t.file("hwmon/hwmon5/temp1_input", "50000\n");

        let (input, source) = t.discover().unwrap();
        assert_eq!(source, "coretemp Package id 0");
        assert_eq!(read_millidegrees(&input).unwrap(), 47.0);
    }

    #[test]
    fn falls_back_to_a_thermal_zone() {
        let t = Tree::new("zone");
        t.file("thermal/thermal_zone0/type", "acpitz\n");
        t.file("thermal/thermal_zone0/temp", "27800\n");
        t.file("thermal/thermal_zone1/type", "x86_pkg_temp\n");
        t.file("thermal/thermal_zone1/temp", "52000\n");

        let (input, source) = t.discover().unwrap();
        assert_eq!(source, "thermal zone x86_pkg_temp");
        assert_eq!(read_millidegrees(&input).unwrap(), 52.0);
    }

    #[test]
    fn nothing_found() {
        let t = Tree::new("empty");
        t.file("hwmon/hwmon0/name", "nvme\n");
        assert_eq!(t.discover(), None);
    }
}
