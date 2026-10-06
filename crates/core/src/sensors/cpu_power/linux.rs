//! Package energy counter on Linux, from the powercap RAPL zone of the first package (Intel and
//! AMD Zen). The counter is only readable by root, which is what the service runs as.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const ZONE: &str = "/sys/class/powercap/intel-rapl:0";

pub struct Counter {
    energy: PathBuf,
    range: u64,
    source: String,
}

impl Counter {
    pub fn open() -> io::Result<Self> {
        Self::open_in(Path::new(ZONE))
    }

    fn open_in(zone: &Path) -> io::Result<Self> {
        let name = fs::read_to_string(zone.join("name")).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("no RAPL power zone at {} ({e})", zone.display()),
            )
        })?;
        let counter = Self {
            energy: zone.join("energy_uj"),
            range: read_number(&zone.join("max_energy_range_uj"))?,
            source: format!("powercap {}", name.trim()),
        };
        counter.read()?;
        Ok(counter)
    }

    pub fn read(&self) -> io::Result<u64> {
        read_number(&self.energy)
    }

    pub fn range(&self) -> u64 {
        self.range
    }

    pub fn joules_per_count(&self) -> f64 {
        1e-6
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

fn read_number(path: &Path) -> io::Result<u64> {
    let text = fs::read_to_string(path).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            io::Error::new(e.kind(), format!("{}: root required", path.display()))
        } else {
            e
        }
    })?;
    text.trim().parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: not a number", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_fake_zone() {
        let zone = std::env::temp_dir().join(format!("coolercast-rapl-{}", std::process::id()));
        fs::create_dir_all(&zone).unwrap();
        fs::write(zone.join("name"), "package-0\n").unwrap();
        fs::write(zone.join("energy_uj"), "123456789\n").unwrap();
        fs::write(zone.join("max_energy_range_uj"), "262143328850\n").unwrap();

        let counter = Counter::open_in(&zone);
        let missing = Counter::open_in(&zone.join("nope"));
        fs::remove_dir_all(&zone).unwrap();

        let counter = counter.unwrap();
        assert_eq!(counter.source(), "powercap package-0");
        assert_eq!(counter.range(), 262_143_328_850);
        assert!(missing.is_err());
    }
}
