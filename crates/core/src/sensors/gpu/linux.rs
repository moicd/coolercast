//! GPU sensors on Linux, from sysfs, with no privileges or extra drivers:
//!
//! - AMD (`amdgpu`): `gpu_busy_percent`, and the hwmon `temp1_input` (edge, m°C),
//!   `power1_average` or `power1_input` (µW) and `freq1_input` (Hz).
//! - Intel (`i915`, `xe`): the actual GT clock, and on discrete cards the hwmon temperature and
//!   energy counter. Intel exposes no usage in sysfs.
//! - NVIDIA with `nouveau`: the hwmon temperature and power.
//! - NVIDIA with the proprietary driver, which has no sysfs sensors: `nvidia-smi` running in its
//!   loop mode, so there is one long-lived helper process instead of one per refresh. NVML itself
//!   would need `dlopen`, which a static binary without libc cannot do.
//!
//! With several GPUs, NVIDIA and AMD cards come before Intel ones, and the one with the most
//! video memory first.

use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{Values, Vendor};
use crate::sensors::cpu_power::Average;

const DRM: &str = "/sys/class/drm";

/// Reads the main GPU.
pub struct Gpu {
    name: String,
    backend: Backend,
}

enum Backend {
    Sysfs(Files),
    NvidiaSmi(NvidiaSmi),
}

/// The sysfs files of one card; `None` for values its driver does not expose.
#[derive(Debug, Default, PartialEq)]
struct Files {
    usage: Option<PathBuf>,
    temp: Option<PathBuf>,
    power: Option<PowerFile>,
    freq: Option<FreqFile>,
}

#[derive(Debug, PartialEq)]
enum PowerFile {
    /// Average power in µW.
    Microwatts(PathBuf),
    /// Energy counter in µJ, averaged into power over the time between reads.
    Microjoules { path: PathBuf, average: Average },
}

#[derive(Debug, PartialEq)]
enum FreqFile {
    Hz(PathBuf),
    Mhz(PathBuf),
}

/// A display controller under `/sys/class/drm`.
#[derive(Debug)]
struct Card {
    dir: PathBuf,
    vendor: Vendor,
    driver: String,
    vram: u64,
}

impl Gpu {
    pub fn open() -> io::Result<Self> {
        Self::open_in(Path::new(DRM))
    }

    fn open_in(drm: &Path) -> io::Result<Self> {
        let card = find_cards(drm)
            .into_iter()
            .min_by_key(|c| (c.vendor.rank(), std::cmp::Reverse(c.vram)))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no GPU found"))?;
        let card_name = card.dir.file_name().unwrap_or_default().to_string_lossy();
        let (name, backend) = match (card.vendor, card.driver.as_str()) {
            (Vendor::Nvidia, "nvidia") => {
                let smi = NvidiaSmi::spawn()?;
                (smi.name.clone(), Backend::NvidiaSmi(smi))
            }
            (vendor, driver) => (
                format!("{vendor:?} GPU ({card_name}, {driver})"),
                Backend::Sysfs(files(&card)),
            ),
        };
        let mut gpu = Self { name, backend };
        if let Backend::Sysfs(files) = &gpu.backend
            && *files == Files::default()
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("{}: no sensors exposed by the driver", gpu.name),
            ));
        }
        gpu.read()?;
        Ok(gpu)
    }

    pub fn read(&mut self) -> io::Result<Values> {
        match &mut self.backend {
            Backend::Sysfs(files) => Ok(files.read()),
            Backend::NvidiaSmi(smi) => smi.latest(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Human-readable description of the interface in use.
    pub fn source(&self) -> &'static str {
        match self.backend {
            Backend::Sysfs(_) => "sysfs",
            Backend::NvidiaSmi(_) => "nvidia-smi",
        }
    }
}

impl Files {
    fn read(&mut self) -> Values {
        Values {
            temp: self
                .temp
                .as_deref()
                .and_then(read_number)
                .map(|m| m as f32 / 1000.0),
            usage: self
                .usage
                .as_deref()
                .and_then(read_number)
                .map(|p| p as f32),
            power: self.power.as_mut().and_then(PowerFile::read),
            freq: self.freq.as_ref().and_then(|f| match f {
                FreqFile::Hz(path) => read_number(path).map(|hz| (hz as f64 / 1e6) as f32),
                FreqFile::Mhz(path) => read_number(path).map(|mhz| mhz as f32),
            }),
        }
    }
}

impl PowerFile {
    fn read(&mut self) -> Option<f32> {
        match self {
            PowerFile::Microwatts(path) => read_number(path).map(|uw| uw as f32 / 1e6),
            PowerFile::Microjoules { path, average } => {
                let microjoules = read_number(path)?;
                average.update(microjoules, u64::MAX, 1e-6, Instant::now())
            }
        }
    }
}

/// Every display controller (PCI class 0x03) with a known driver.
fn find_cards(drm: &Path) -> Vec<Card> {
    let Ok(entries) = fs::read_dir(drm) else {
        return Vec::new();
    };
    let mut cards: Vec<Card> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix("card"))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter_map(|e| {
            let dir = e.path();
            let device = dir.join("device");
            let class = read_hex(&device.join("class"))?;
            if class >> 16 != 0x03 {
                return None;
            }
            let driver = fs::read_link(device.join("driver"))
                .ok()?
                .file_name()?
                .to_string_lossy()
                .into_owned();
            Some(Card {
                vendor: Vendor::from_pci_id(read_hex(&device.join("vendor"))?),
                driver,
                vram: read_number(&device.join("mem_info_vram_total")).unwrap_or(0),
                dir,
            })
        })
        .collect();
    cards.sort_by(|a, b| a.dir.cmp(&b.dir));
    cards
}

/// The sensor files the card's driver exposes.
fn files(card: &Card) -> Files {
    let device = card.dir.join("device");
    let hwmon = first_dir(&device.join("hwmon"));
    let in_hwmon = |name: &str| hwmon.as_ref().map(|h| h.join(name)).filter(|p| p.is_file());
    let existing = |path: PathBuf| path.is_file().then_some(path);
    match card.driver.as_str() {
        "amdgpu" => Files {
            usage: existing(device.join("gpu_busy_percent")),
            temp: in_hwmon("temp1_input"),
            power: in_hwmon("power1_average")
                .or_else(|| in_hwmon("power1_input"))
                .map(PowerFile::Microwatts),
            freq: in_hwmon("freq1_input").map(FreqFile::Hz),
        },
        "i915" | "xe" => Files {
            usage: None,
            temp: hwmon.as_deref().and_then(first_temp_input),
            power: in_hwmon("energy1_input").map(|path| PowerFile::Microjoules {
                path,
                average: Average::default(),
            }),
            freq: existing(card.dir.join("gt_act_freq_mhz"))
                .or_else(|| existing(device.join("tile0/gt0/freq0/act_freq")))
                .map(FreqFile::Mhz),
        },
        _ => Files {
            usage: None,
            temp: in_hwmon("temp1_input"),
            power: in_hwmon("power1_input").map(PowerFile::Microwatts),
            freq: None,
        },
    }
}

fn first_dir(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).min()
}

fn first_temp_input(hwmon: &Path) -> Option<PathBuf> {
    (1..=8)
        .map(|n| hwmon.join(format!("temp{n}_input")))
        .find(|p| p.is_file())
}

fn read_number(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_hex(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// `nvidia-smi` printing one CSV line per second, read by a background thread.
struct NvidiaSmi {
    child: Child,
    name: String,
    latest: Arc<Mutex<Option<Values>>>,
}

impl NvidiaSmi {
    fn spawn() -> io::Result<Self> {
        let name = Command::new("nvidia-smi")
            .args(["--query-gpu=name", "--format=csv,noheader", "-i", "0"])
            .output()
            .map_err(|e| io::Error::new(e.kind(), format!("nvidia-smi: {e}")))?;
        let name = String::from_utf8_lossy(&name.stdout).trim().to_owned();
        let mut child = Command::new("nvidia-smi")
            .args([
                "--query-gpu=temperature.gpu,utilization.gpu,power.draw,clocks.gr",
                "--format=csv,noheader,nounits",
                "-i",
                "0",
                "-lms",
                "1000",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no stdout"))?;
        let latest = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&latest);
        std::thread::Builder::new()
            .name("nvidia-smi".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Some(readings) = parse_smi_line(&line) {
                        *sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(readings);
                    }
                }
            })?;
        Ok(Self {
            child,
            name: if name.is_empty() {
                "NVIDIA GPU".into()
            } else {
                name
            },
            latest,
        })
    }

    fn latest(&mut self) -> io::Result<Values> {
        if let Some(status) = self.child.try_wait()? {
            return Err(io::Error::other(format!("nvidia-smi exited ({status})")));
        }
        // Before its first line the values are unknown, not an error.
        Ok(self
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or_default())
    }
}

impl Drop for NvidiaSmi {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Parses `temperature, utilization, power, clock`; unsupported fields read `[N/A]`.
fn parse_smi_line(line: &str) -> Option<Values> {
    let mut fields = line.split(',').map(|f| f.trim().parse::<f32>().ok());
    Some(Values {
        temp: fields.next()?,
        usage: fields.next()?,
        power: fields.next()?,
        freq: fields.next()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("coolercast-gpu-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn file(&self, path: &str, content: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        /// A card whose `device/driver` link points at a driver directory.
        fn card(&self, card: &str, vendor: &str, driver: &str) {
            self.file(&format!("{card}/device/vendor"), &format!("{vendor}\n"));
            self.file(&format!("{card}/device/class"), "0x030000\n");
            let target = self.0.join("drivers").join(driver);
            fs::create_dir_all(&target).unwrap();
            std::os::unix::fs::symlink(&target, self.0.join(card).join("device/driver")).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn amd_card_is_read_from_sysfs() {
        let t = Tree::new("amd");
        t.card("card0", "0x8086", "i915");
        t.card("card1", "0x1002", "amdgpu");
        t.file("card1/device/gpu_busy_percent", "37\n");
        t.file("card1/device/hwmon/hwmon4/temp1_input", "45000\n");
        t.file("card1/device/hwmon/hwmon4/power1_average", "87500000\n");
        t.file("card1/device/hwmon/hwmon4/freq1_input", "2450000000\n");
        t.file("card1-DP-1/status", "connected\n");

        let mut gpu = Gpu::open_in(&t.0).unwrap();
        assert_eq!(gpu.source(), "sysfs");
        assert!(gpu.name().contains("card1"), "{}", gpu.name());
        assert_eq!(
            gpu.read().unwrap(),
            Values {
                temp: Some(45.0),
                usage: Some(37.0),
                power: Some(87.5),
                freq: Some(2450.0),
            }
        );
    }

    #[test]
    fn intel_card_has_clock_and_energy() {
        let t = Tree::new("intel");
        t.card("card0", "0x8086", "i915");
        t.file("card0/gt_act_freq_mhz", "1300\n");
        t.file("card0/device/hwmon/hwmon2/energy1_input", "1000000\n");

        let mut gpu = Gpu::open_in(&t.0).unwrap();
        let r = gpu.read().unwrap();
        assert_eq!((r.temp, r.usage, r.freq), (None, None, Some(1300.0)));
        // Opening primed the counter: read right after, the power is not known yet (not 0 W).
        assert_eq!(r.power, None);
    }

    #[test]
    fn biggest_amd_card_wins() {
        let t = Tree::new("two-amd");
        t.card("card0", "0x1002", "amdgpu");
        t.file("card0/device/mem_info_vram_total", "536870912\n");
        t.file("card0/device/gpu_busy_percent", "1\n");
        t.card("card1", "0x1002", "amdgpu");
        t.file("card1/device/mem_info_vram_total", "17163091968\n");
        t.file("card1/device/gpu_busy_percent", "2\n");
        let mut gpu = Gpu::open_in(&t.0).unwrap();
        assert_eq!(gpu.read().unwrap().usage, Some(2.0));
    }

    #[test]
    fn no_card() {
        let t = Tree::new("none");
        assert!(Gpu::open_in(&t.0).is_err());
    }

    #[test]
    fn nvidia_smi_lines() {
        assert_eq!(
            parse_smi_line("45, 12, 35.20, 1530"),
            Some(Values {
                temp: Some(45.0),
                usage: Some(12.0),
                power: Some(35.2),
                freq: Some(1530.0),
            })
        );
        let partial = parse_smi_line("50, 3, [N/A], 210").unwrap();
        assert_eq!((partial.power, partial.freq), (None, Some(210.0)));
        assert_eq!(parse_smi_line("No devices were found"), None);
    }
}
