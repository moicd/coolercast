//! Local IPC between the service and its clients (CLI, app): a named pipe on Windows, a Unix
//! socket on Linux.
//!
//! Requests are single lines: `status` or `set <key>=<value>`. Responses start with `ok` or
//! `error: <message>`; a status response continues with one `key=value` per line.

use std::io;
use std::thread::JoinHandle;

use crate::config::Config;

#[cfg(target_os = "linux")]
mod unix;
#[cfg(target_os = "linux")]
use unix as transport;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as transport;

/// Sends one request to the running service and returns its response.
pub fn request(command: &str) -> io::Result<String> {
    transport::request(command)
}

/// Serves requests on a background thread, one client at a time.
pub fn serve<F>(handler: F) -> io::Result<JoinHandle<()>>
where
    F: Fn(&str) -> String + Send + 'static,
{
    transport::serve(handler)
}

/// Number of samples kept in [`Status::history`].
pub const HISTORY_LEN: usize = 120;

/// Which value the cooler display currently shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Temperature,
    Usage,
    Power,
    Custom,
}

/// One sensor reading per refresh interval.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub cpu_temp: Option<f32>,
    pub cpu_usage: f32,
}

/// Snapshot of what the service is doing, as reported by `status`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub devices: Vec<String>,
    pub cpu_temp: Option<f32>,
    pub cpu_usage: Option<f32>,
    /// CPU package power in watts, measured only while a display shows it.
    pub cpu_power: Option<f32>,
    /// Average CPU frequency in MHz, measured only while a display shows it.
    pub cpu_freq: Option<f32>,
    /// Why the temperature is unavailable, if it is.
    pub temp_error: Option<String>,
    /// What the display shows right now (`None` before the first update).
    pub shown: Option<Shown>,
    /// Whether the display is blinking its alarm.
    pub alarm_active: bool,
    /// Recent samples, oldest first, at most [`HISTORY_LEN`].
    pub history: Vec<Sample>,
    pub config: Config,
}

impl Status {
    pub fn encode(&self) -> String {
        let mut out = String::new();
        let mut line = |key: &str, value: &str| {
            out.push_str(key);
            out.push('=');
            out.push_str(&value.replace(['\r', '\n'], " "));
            out.push('\n');
        };
        line("devices", &self.devices.join(";"));
        line(
            "cpu_temp",
            &self.cpu_temp.map_or(String::new(), |t| format!("{t:.1}")),
        );
        line(
            "cpu_usage",
            &self.cpu_usage.map_or(String::new(), |u| format!("{u:.1}")),
        );
        line(
            "cpu_power",
            &self.cpu_power.map_or(String::new(), |w| format!("{w:.1}")),
        );
        line(
            "cpu_freq",
            &self.cpu_freq.map_or(String::new(), |f| format!("{f:.0}")),
        );
        line("temp_error", self.temp_error.as_deref().unwrap_or(""));
        line(
            "shown",
            match self.shown {
                Some(Shown::Temperature) => "temperature",
                Some(Shown::Usage) => "usage",
                Some(Shown::Power) => "power",
                Some(Shown::Custom) => "custom",
                None => "",
            },
        );
        line(
            "alarm_active",
            if self.alarm_active { "true" } else { "false" },
        );
        let history: Vec<String> = self
            .history
            .iter()
            .map(|s| match s.cpu_temp {
                Some(t) => format!("{t:.1}/{:.1}", s.cpu_usage),
                None => format!("/{:.1}", s.cpu_usage),
            })
            .collect();
        line("history", &history.join(","));
        for (key, value) in self.config.entries() {
            line(key, &value);
        }
        out
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let mut status = Status::default();
        for entry in text.lines().filter(|l| !l.is_empty()) {
            let (key, value) = entry
                .split_once('=')
                .ok_or_else(|| format!("bad line '{entry}'"))?;
            let number = || value.parse::<f32>().ok();
            match key {
                "devices" => {
                    status.devices = value
                        .split(';')
                        .filter(|d| !d.is_empty())
                        .map(Into::into)
                        .collect()
                }
                "cpu_temp" => status.cpu_temp = number(),
                "cpu_usage" => status.cpu_usage = number(),
                "cpu_power" => status.cpu_power = number(),
                "cpu_freq" => status.cpu_freq = number(),
                "temp_error" => {
                    status.temp_error = Some(value.to_owned()).filter(|e| !e.is_empty())
                }
                "shown" => {
                    status.shown = match value {
                        "temperature" => Some(Shown::Temperature),
                        "usage" => Some(Shown::Usage),
                        "power" => Some(Shown::Power),
                        "custom" => Some(Shown::Custom),
                        _ => None,
                    }
                }
                "alarm_active" => status.alarm_active = value == "true",
                "history" => {
                    status.history = value
                        .split(',')
                        .filter_map(|entry| {
                            let (temp, usage) = entry.split_once('/')?;
                            Some(Sample {
                                cpu_temp: temp.parse().ok(),
                                cpu_usage: usage.parse().ok()?,
                            })
                        })
                        .collect()
                }
                // Ignore settings added by a newer service.
                _ => {
                    let _ = status.config.set(key, value);
                }
            }
        }
        Ok(status)
    }
}

/// Asks the service for its status.
pub fn query_status() -> io::Result<Status> {
    let response = request("status")?;
    let body = response
        .strip_prefix("ok\n")
        .ok_or_else(|| io::Error::other(response.trim().to_owned()))?;
    Status::decode(body).map_err(io::Error::other)
}

/// Changes a setting on the running service.
pub fn set(key: &str, value: &str) -> io::Result<()> {
    let response = request(&format!("set {key}={value}"))?;
    match response.trim() {
        "ok" => Ok(()),
        other => Err(io::Error::other(
            other.trim_start_matches("error: ").to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, Unit};

    #[test]
    fn status_round_trip() {
        let status = Status {
            devices: vec!["AK400 DIGITAL".into(), "AK620 DIGITAL".into()],
            cpu_temp: Some(41.5),
            cpu_usage: Some(12.0),
            cpu_power: Some(87.5),
            cpu_freq: Some(4700.0),
            temp_error: None,
            shown: Some(Shown::Power),
            alarm_active: true,
            history: vec![
                Sample {
                    cpu_temp: Some(40.5),
                    cpu_usage: 3.0,
                },
                Sample {
                    cpu_temp: None,
                    cpu_usage: 99.5,
                },
            ],
            config: Config {
                mode: Mode::Auto,
                unit: Unit::Fahrenheit,
                ..Config::default()
            },
        };
        assert_eq!(Status::decode(&status.encode()).unwrap(), status);

        let empty = Status {
            temp_error: Some("PawnIO is not installed".into()),
            ..Status::default()
        };
        assert_eq!(Status::decode(&empty.encode()).unwrap(), empty);
    }
}
