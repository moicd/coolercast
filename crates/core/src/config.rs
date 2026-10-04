//! User settings, stored as TOML.

use std::path::Path;
use std::str::FromStr;
use std::{fmt, fs, io};

use serde::{Deserialize, Serialize};

/// What the display shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Temperature,
    Usage,
    /// Alternates between temperature and usage.
    Auto,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Celsius,
    Fahrenheit,
}

impl Unit {
    /// Converts a Celsius value into this unit.
    pub fn from_celsius(self, celsius: f32) -> f32 {
        match self {
            Unit::Celsius => celsius,
            Unit::Fahrenheit => celsius * 9.0 / 5.0 + 32.0,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Unit::Celsius => "°C",
            Unit::Fahrenheit => "°F",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub mode: Mode,
    pub unit: Unit,
    /// Blink the display when the CPU reaches `alarm_threshold`.
    pub alarm: bool,
    /// Alarm temperature in °C.
    pub alarm_threshold: u8,
    /// Display refresh interval in milliseconds.
    pub interval_ms: u32,
    /// Seconds each value stays on screen in `auto` mode.
    pub auto_interval_s: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Temperature,
            unit: Unit::Celsius,
            alarm: true,
            alarm_threshold: 90,
            interval_ms: 1000,
            auto_interval_s: 5,
        }
    }
}

const INTERVAL_MS: (u32, u32) = (250, 10_000);
const AUTO_INTERVAL_S: (u32, u32) = (1, 3600);
const ALARM_THRESHOLD: (u8, u8) = (40, 110);

impl Config {
    /// Loads the config file, falling back to defaults if it does not exist.
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                Self::parse(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str::<Self>(text)
            .map(Self::clamped)
            .map_err(|e| e.message().to_owned())
    }

    /// Writes the config atomically (temporary file + rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, self.to_toml())?;
        fs::rename(&tmp, path)
    }

    pub fn to_toml(&self) -> String {
        format!(
            "# deepcool-native settings. Changes are picked up automatically.\n\
             \n\
             # What the display shows: \"temperature\", \"usage\" or \"auto\" (alternates both).\n\
             mode = \"{mode}\"\n\
             # Temperature unit: \"celsius\" or \"fahrenheit\".\n\
             unit = \"{unit}\"\n\
             # Blink the display when the CPU reaches alarm_threshold (°C, {a_min}-{a_max}).\n\
             alarm = {alarm}\n\
             alarm_threshold = {threshold}\n\
             # Refresh interval in milliseconds ({i_min}-{i_max}).\n\
             interval_ms = {interval}\n\
             # Seconds each value stays on screen in auto mode ({s_min}-{s_max}).\n\
             auto_interval_s = {auto}\n",
            mode = self.mode,
            unit = self.unit,
            alarm = self.alarm,
            threshold = self.alarm_threshold,
            interval = self.interval_ms,
            auto = self.auto_interval_s,
            a_min = ALARM_THRESHOLD.0,
            a_max = ALARM_THRESHOLD.1,
            i_min = INTERVAL_MS.0,
            i_max = INTERVAL_MS.1,
            s_min = AUTO_INTERVAL_S.0,
            s_max = AUTO_INTERVAL_S.1,
        )
    }

    /// Changes one setting from its text form, as used by the CLI and the tray.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        fn num<T>(value: &str, (min, max): (T, T)) -> Result<T, String>
        where
            T: FromStr + PartialOrd + fmt::Display + Copy,
        {
            let n: T = value
                .parse()
                .map_err(|_| format!("'{value}' is not a number"))?;
            if n < min || n > max {
                return Err(format!("{value} is out of range ({min}-{max})"));
            }
            Ok(n)
        }

        match key {
            "mode" => self.mode = value.parse()?,
            "unit" => self.unit = value.parse()?,
            "alarm" => {
                self.alarm = match value {
                    "true" | "on" | "1" => true,
                    "false" | "off" | "0" => false,
                    _ => return Err(format!("'{value}' is not a boolean")),
                }
            }
            "alarm_threshold" => self.alarm_threshold = num(value, ALARM_THRESHOLD)?,
            "interval_ms" => self.interval_ms = num(value, INTERVAL_MS)?,
            "auto_interval_s" => self.auto_interval_s = num(value, AUTO_INTERVAL_S)?,
            _ => return Err(format!("unknown setting '{key}'")),
        }
        Ok(())
    }

    /// `(key, value)` pairs in the same text form accepted by [`Config::set`].
    pub fn entries(&self) -> [(&'static str, String); 6] {
        [
            ("mode", self.mode.to_string()),
            ("unit", self.unit.to_string()),
            ("alarm", self.alarm.to_string()),
            ("alarm_threshold", self.alarm_threshold.to_string()),
            ("interval_ms", self.interval_ms.to_string()),
            ("auto_interval_s", self.auto_interval_s.to_string()),
        ]
    }

    fn clamped(mut self) -> Self {
        self.alarm_threshold = self
            .alarm_threshold
            .clamp(ALARM_THRESHOLD.0, ALARM_THRESHOLD.1);
        self.interval_ms = self.interval_ms.clamp(INTERVAL_MS.0, INTERVAL_MS.1);
        self.auto_interval_s = self
            .auto_interval_s
            .clamp(AUTO_INTERVAL_S.0, AUTO_INTERVAL_S.1);
        self
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Temperature => "temperature",
            Mode::Usage => "usage",
            Mode::Auto => "auto",
        })
    }
}

impl FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "temperature" | "temp" => Ok(Mode::Temperature),
            "usage" => Ok(Mode::Usage),
            "auto" => Ok(Mode::Auto),
            _ => Err(format!(
                "unknown mode '{s}' (expected temperature, usage or auto)"
            )),
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Unit::Celsius => "celsius",
            Unit::Fahrenheit => "fahrenheit",
        })
    }
}

impl FromStr for Unit {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "celsius" | "c" => Ok(Unit::Celsius),
            "fahrenheit" | "f" => Ok(Unit::Fahrenheit),
            _ => Err(format!(
                "unknown unit '{s}' (expected celsius or fahrenheit)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_file_parses_back() {
        let config = Config {
            mode: Mode::Auto,
            unit: Unit::Fahrenheit,
            alarm: false,
            alarm_threshold: 85,
            interval_ms: 500,
            auto_interval_s: 3,
        };
        assert_eq!(Config::parse(&config.to_toml()).unwrap(), config);
        assert_eq!(
            Config::parse(&Config::default().to_toml()).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn missing_keys_use_defaults() {
        let config = Config::parse("mode = \"usage\"").unwrap();
        assert_eq!(
            config,
            Config {
                mode: Mode::Usage,
                ..Config::default()
            }
        );
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let config = Config::parse("interval_ms = 1\nalarm_threshold = 255").unwrap();
        assert_eq!(config.interval_ms, 250);
        assert_eq!(config.alarm_threshold, 110);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("colour = \"red\"").is_err());
        assert!(Config::parse("mode = \"rainbow\"").is_err());
    }

    #[test]
    fn set_validates_input() {
        let mut config = Config::default();
        config.set("mode", "usage").unwrap();
        config.set("unit", "f").unwrap();
        config.set("alarm", "off").unwrap();
        config.set("alarm_threshold", "80").unwrap();
        assert_eq!(config.mode, Mode::Usage);
        assert_eq!(config.unit, Unit::Fahrenheit);
        assert!(!config.alarm);
        assert_eq!(config.alarm_threshold, 80);

        assert!(config.set("interval_ms", "10").is_err());
        assert!(config.set("alarm", "maybe").is_err());
        assert!(config.set("nope", "1").is_err());
        assert_eq!(config.interval_ms, 1000);
    }

    #[test]
    fn entries_round_trip_through_set() {
        let source = Config {
            mode: Mode::Auto,
            alarm_threshold: 70,
            ..Config::default()
        };
        let mut copy = Config::default();
        for (key, value) in source.entries() {
            copy.set(key, &value).unwrap();
        }
        assert_eq!(copy, source);
    }
}
