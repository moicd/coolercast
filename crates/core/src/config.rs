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
    /// A fixed value chosen by the user (`custom_value`, `custom_symbol`, `custom_bar`).
    Custom,
    /// CPU power in watts, on displays with a power symbol (LS series); the others show the
    /// temperature.
    Power,
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

/// Unit symbol lit next to the digits in `custom` mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Symbol {
    #[default]
    Celsius,
    Fahrenheit,
    Percent,
}

/// Which component the display shows. Displays with a CPU and a GPU section (CH series) show
/// both, and displays made for the CPU only (LD, LQ, DIGITAL PRO) ignore it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Cpu,
    Gpu,
    /// Alternates between the CPU and the GPU.
    Auto,
    /// The GPU while it is busy (gaming), the CPU otherwise.
    Smart,
}

/// What the bar of single-value displays shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bar {
    /// Follows the number on the display.
    #[default]
    Value,
    /// The usage of the component shown, so a temperature and a usage are visible at once.
    Usage,
}

/// A time of day in minutes since midnight, written `HH:MM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ClockTime(u16);

impl ClockTime {
    pub const fn new(hour: u16, minute: u16) -> Self {
        Self(hour * 60 + minute)
    }

    pub fn from_minutes(minutes: u16) -> Self {
        Self(minutes % (24 * 60))
    }

    pub fn minutes(self) -> u16 {
        self.0
    }

    /// Whether `now` falls in `[start, end)`, which may wrap past midnight. An empty range
    /// (`start == end`) contains nothing.
    pub fn in_range(now: Self, start: Self, end: Self) -> bool {
        if start <= end {
            start <= now && now < end
        } else {
            now >= start || now < end
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub mode: Mode,
    pub source: Source,
    pub bar: Bar,
    pub unit: Unit,
    /// Blink the display when the CPU reaches `alarm_threshold`.
    pub alarm: bool,
    /// Alarm temperature in °C.
    pub alarm_threshold: u8,
    /// Display refresh interval in milliseconds.
    pub interval_ms: u32,
    /// Seconds each value stays on screen in `auto` mode.
    pub auto_interval_s: u32,
    /// Number shown in `custom` mode.
    pub custom_value: u16,
    /// Symbol lit in `custom` mode.
    pub custom_symbol: Symbol,
    /// Bar level in `custom` mode.
    pub custom_bar: u8,
    /// Turn the display off while the session is locked (Windows).
    pub off_when_locked: bool,
    /// Turn the display off while the PC screen is off (Windows).
    pub off_when_screen_off: bool,
    /// Turn the display off every night between `night_start` and `night_end`.
    pub off_at_night: bool,
    /// Local time.
    pub night_start: ClockTime,
    /// Local time.
    pub night_end: ClockTime,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Temperature,
            source: Source::Cpu,
            bar: Bar::Value,
            unit: Unit::Celsius,
            alarm: true,
            alarm_threshold: 90,
            interval_ms: 1000,
            auto_interval_s: 5,
            custom_value: 0,
            custom_symbol: Symbol::Celsius,
            custom_bar: 1,
            off_when_locked: false,
            off_when_screen_off: false,
            off_at_night: false,
            night_start: ClockTime::new(23, 0),
            night_end: ClockTime::new(7, 0),
        }
    }
}

/// Allowed range of [`Config::interval_ms`].
pub const INTERVAL_MS: (u32, u32) = (250, 10_000);
/// Allowed range of [`Config::auto_interval_s`].
pub const AUTO_INTERVAL_S: (u32, u32) = (1, 3600);
/// Allowed range of [`Config::alarm_threshold`].
pub const ALARM_THRESHOLD: (u8, u8) = (40, 110);
/// Allowed range of [`Config::custom_value`]: what three digits can show.
pub const CUSTOM_VALUE: (u16, u16) = (0, 999);
/// Allowed range of [`Config::custom_bar`].
pub const CUSTOM_BAR: (u8, u8) = (1, 10);

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
            "# CoolerCast settings. Changes are picked up automatically.\n\
             \n\
             # What the display shows: \"temperature\", \"usage\", \"auto\" (alternates both),\n\
             # \"power\" (LS series; other displays show the temperature) or \"custom\" (the\n\
             # custom_* values below). Displays that show several values at once ignore it.\n\
             mode = \"{mode}\"\n\
             # Component shown: \"cpu\", \"gpu\", \"auto\" (alternates both every\n\
             # auto_interval_s) or \"smart\" (the GPU while it is busy, the CPU otherwise).\n\
             # CH series cases always show both.\n\
             source = \"{source}\"\n\
             # Bar: \"value\" (follows the number shown) or \"usage\" (the usage of the component\n\
             # shown, so AK displays show its temperature and usage at once).\n\
             bar = \"{bar}\"\n\
             # Temperature unit: \"celsius\" or \"fahrenheit\".\n\
             unit = \"{unit}\"\n\
             # Blink the display when the CPU reaches alarm_threshold (°C, {a_min}-{a_max}).\n\
             alarm = {alarm}\n\
             alarm_threshold = {threshold}\n\
             # Refresh interval in milliseconds ({i_min}-{i_max}).\n\
             interval_ms = {interval}\n\
             # Seconds each value stays on screen in auto mode ({s_min}-{s_max}).\n\
             auto_interval_s = {auto}\n\
             \n\
             # Custom mode: a number ({v_min}-{v_max}), the symbol next to it (\"celsius\",\n\
             # \"fahrenheit\" or \"percent\") and the bar level ({b_min}-{b_max}).\n\
             custom_value = {custom_value}\n\
             custom_symbol = \"{custom_symbol}\"\n\
             custom_bar = {custom_bar}\n\
             \n\
             # Turn the display off while the PC is locked or its screen is off (Windows only),\n\
             # or every night from night_start to night_end (\"HH:MM\", local time).\n\
             off_when_locked = {off_when_locked}\n\
             off_when_screen_off = {off_when_screen_off}\n\
             off_at_night = {off_at_night}\n\
             night_start = \"{night_start}\"\n\
             night_end = \"{night_end}\"\n",
            mode = self.mode,
            source = self.source,
            bar = self.bar,
            unit = self.unit,
            alarm = self.alarm,
            threshold = self.alarm_threshold,
            interval = self.interval_ms,
            auto = self.auto_interval_s,
            custom_value = self.custom_value,
            custom_symbol = self.custom_symbol,
            custom_bar = self.custom_bar,
            off_when_locked = self.off_when_locked,
            off_when_screen_off = self.off_when_screen_off,
            off_at_night = self.off_at_night,
            night_start = self.night_start,
            night_end = self.night_end,
            a_min = ALARM_THRESHOLD.0,
            a_max = ALARM_THRESHOLD.1,
            i_min = INTERVAL_MS.0,
            i_max = INTERVAL_MS.1,
            s_min = AUTO_INTERVAL_S.0,
            s_max = AUTO_INTERVAL_S.1,
            v_min = CUSTOM_VALUE.0,
            v_max = CUSTOM_VALUE.1,
            b_min = CUSTOM_BAR.0,
            b_max = CUSTOM_BAR.1,
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

        fn boolean(value: &str) -> Result<bool, String> {
            match value {
                "true" | "on" | "1" => Ok(true),
                "false" | "off" | "0" => Ok(false),
                _ => Err(format!("'{value}' is not a boolean")),
            }
        }

        match key {
            "mode" => self.mode = value.parse()?,
            "source" => self.source = value.parse()?,
            "bar" => self.bar = value.parse()?,
            "unit" => self.unit = value.parse()?,
            "alarm" => self.alarm = boolean(value)?,
            "alarm_threshold" => self.alarm_threshold = num(value, ALARM_THRESHOLD)?,
            "interval_ms" => self.interval_ms = num(value, INTERVAL_MS)?,
            "auto_interval_s" => self.auto_interval_s = num(value, AUTO_INTERVAL_S)?,
            "custom_value" => self.custom_value = num(value, CUSTOM_VALUE)?,
            "custom_symbol" => self.custom_symbol = value.parse()?,
            "custom_bar" => self.custom_bar = num(value, CUSTOM_BAR)?,
            "off_when_locked" => self.off_when_locked = boolean(value)?,
            "off_when_screen_off" => self.off_when_screen_off = boolean(value)?,
            "off_at_night" => self.off_at_night = boolean(value)?,
            "night_start" => self.night_start = value.parse()?,
            "night_end" => self.night_end = value.parse()?,
            _ => return Err(format!("unknown setting '{key}'")),
        }
        Ok(())
    }

    /// `(key, value)` pairs in the same text form accepted by [`Config::set`].
    pub fn entries(&self) -> [(&'static str, String); 16] {
        [
            ("mode", self.mode.to_string()),
            ("source", self.source.to_string()),
            ("bar", self.bar.to_string()),
            ("unit", self.unit.to_string()),
            ("alarm", self.alarm.to_string()),
            ("alarm_threshold", self.alarm_threshold.to_string()),
            ("interval_ms", self.interval_ms.to_string()),
            ("auto_interval_s", self.auto_interval_s.to_string()),
            ("custom_value", self.custom_value.to_string()),
            ("custom_symbol", self.custom_symbol.to_string()),
            ("custom_bar", self.custom_bar.to_string()),
            ("off_when_locked", self.off_when_locked.to_string()),
            ("off_when_screen_off", self.off_when_screen_off.to_string()),
            ("off_at_night", self.off_at_night.to_string()),
            ("night_start", self.night_start.to_string()),
            ("night_end", self.night_end.to_string()),
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
        self.custom_value = self.custom_value.clamp(CUSTOM_VALUE.0, CUSTOM_VALUE.1);
        self.custom_bar = self.custom_bar.clamp(CUSTOM_BAR.0, CUSTOM_BAR.1);
        self
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Temperature => "temperature",
            Mode::Usage => "usage",
            Mode::Auto => "auto",
            Mode::Custom => "custom",
            Mode::Power => "power",
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
            "custom" => Ok(Mode::Custom),
            "power" => Ok(Mode::Power),
            _ => Err(format!(
                "unknown mode '{s}' (expected temperature, usage, auto, power or custom)"
            )),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Source::Cpu => "cpu",
            Source::Gpu => "gpu",
            Source::Auto => "auto",
            Source::Smart => "smart",
        })
    }
}

impl FromStr for Source {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "cpu" => Ok(Source::Cpu),
            "gpu" => Ok(Source::Gpu),
            "auto" => Ok(Source::Auto),
            "smart" => Ok(Source::Smart),
            _ => Err(format!(
                "unknown source '{s}' (expected cpu, gpu, auto or smart)"
            )),
        }
    }
}

impl fmt::Display for Bar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Bar::Value => "value",
            Bar::Usage => "usage",
        })
    }
}

impl FromStr for Bar {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "value" => Ok(Bar::Value),
            "usage" => Ok(Bar::Usage),
            _ => Err(format!("unknown bar '{s}' (expected value or usage)")),
        }
    }
}

impl fmt::Display for ClockTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.0 / 60, self.0 % 60)
    }
}

impl FromStr for ClockTime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let invalid = || format!("'{s}' is not a time (expected HH:MM)");
        let (hour, minute) = s.split_once(':').ok_or_else(invalid)?;
        let hour: u16 = hour.parse().map_err(|_| invalid())?;
        let minute: u16 = minute.parse().map_err(|_| invalid())?;
        if hour > 23 || minute > 59 || s.len() > 5 {
            return Err(invalid());
        }
        Ok(Self::new(hour, minute))
    }
}

impl TryFrom<String> for ClockTime {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

impl From<ClockTime> for String {
    fn from(time: ClockTime) -> Self {
        time.to_string()
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

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Symbol::Celsius => "celsius",
            Symbol::Fahrenheit => "fahrenheit",
            Symbol::Percent => "percent",
        })
    }
}

impl FromStr for Symbol {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "celsius" | "c" => Ok(Symbol::Celsius),
            "fahrenheit" | "f" => Ok(Symbol::Fahrenheit),
            "percent" | "%" => Ok(Symbol::Percent),
            _ => Err(format!(
                "unknown symbol '{s}' (expected celsius, fahrenheit or percent)"
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
            source: Source::Gpu,
            bar: Bar::Usage,
            unit: Unit::Fahrenheit,
            alarm: false,
            alarm_threshold: 85,
            interval_ms: 500,
            auto_interval_s: 3,
            custom_value: 123,
            custom_symbol: Symbol::Percent,
            custom_bar: 7,
            off_when_locked: true,
            off_when_screen_off: true,
            off_at_night: true,
            night_start: ClockTime::new(22, 30),
            night_end: ClockTime::new(6, 45),
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
        let config = Config::parse("custom_value = 5000\ncustom_bar = 0").unwrap();
        assert_eq!((config.custom_value, config.custom_bar), (999, 1));
    }

    #[test]
    fn custom_settings_are_validated() {
        let mut config = Config::default();
        config.set("mode", "custom").unwrap();
        config.set("custom_value", "42").unwrap();
        config.set("custom_symbol", "percent").unwrap();
        config.set("custom_bar", "10").unwrap();
        assert_eq!(config.mode, Mode::Custom);
        assert_eq!(
            (config.custom_value, config.custom_symbol, config.custom_bar),
            (42, Symbol::Percent, 10)
        );
        assert!(config.set("custom_value", "1000").is_err());
        assert!(config.set("custom_bar", "11").is_err());
        assert!(config.set("custom_symbol", "kelvin").is_err());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("colour = \"red\"").is_err());
        assert!(Config::parse("mode = \"rainbow\"").is_err());
    }

    #[test]
    fn set_validates_input() {
        let mut config = Config::default();
        config.set("mode", "power").unwrap();
        assert_eq!(config.mode, Mode::Power);
        config.set("source", "smart").unwrap();
        assert_eq!(config.source, Source::Smart);
        assert!(config.set("source", "psu").is_err());
        config.set("bar", "usage").unwrap();
        assert_eq!(config.bar, Bar::Usage);
        config.set("off_at_night", "on").unwrap();
        config.set("night_start", "22:15").unwrap();
        assert_eq!(config.night_start, ClockTime::new(22, 15));
        assert!(config.set("night_end", "24:00").is_err());
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
            source: Source::Gpu,
            off_at_night: true,
            night_end: ClockTime::new(8, 0),
            alarm_threshold: 70,
            ..Config::default()
        };
        let mut copy = Config::default();
        for (key, value) in source.entries() {
            copy.set(key, &value).unwrap();
        }
        assert_eq!(copy, source);
    }

    #[test]
    fn clock_times() {
        assert_eq!("07:05".parse(), Ok(ClockTime::new(7, 5)));
        assert_eq!("7:05".parse(), Ok(ClockTime::new(7, 5)));
        assert_eq!(ClockTime::new(23, 0).to_string(), "23:00");
        for bad in ["24:00", "12:60", "12", "ab:cd", "12:000"] {
            assert!(bad.parse::<ClockTime>().is_err(), "{bad}");
        }
        assert_eq!(ClockTime::from_minutes(24 * 60 + 5), ClockTime::new(0, 5));
    }

    #[test]
    fn night_ranges_wrap_past_midnight() {
        let t = ClockTime::new;
        let night = |now| ClockTime::in_range(now, t(23, 0), t(7, 0));
        assert!(night(t(23, 0)));
        assert!(night(t(2, 30)));
        assert!(!night(t(7, 0)));
        assert!(!night(t(12, 0)));
        let afternoon = |now| ClockTime::in_range(now, t(13, 0), t(15, 0));
        assert!(afternoon(t(14, 0)));
        assert!(!afternoon(t(15, 0)));
        assert!(!ClockTime::in_range(t(5, 0), t(5, 0), t(5, 0)));
    }
}
