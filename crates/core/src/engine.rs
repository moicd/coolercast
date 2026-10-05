//! The update loop: read sensors, build a report, send it to every cooler, sleep.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use crate::config::{Config, Mode};
use crate::device::{self, Cooler, Reading};
use crate::ipc::{HISTORY_LEN, Sample, Shown, Status};
use crate::sensors::cpu_temp::CpuTemp;
use crate::sensors::cpu_usage::CpuUsage;
use crate::win::{Event, wait_any};
use crate::{info, warn};

/// How often to look for coolers while none is connected.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);

pub struct Engine {
    state: Mutex<State>,
    config_path: Option<PathBuf>,
    /// Signaled when settings change so the display updates immediately.
    wake: Event,
}

struct State {
    config: Config,
    config_modified: Option<SystemTime>,
    status: Status,
}

impl Engine {
    /// `config_path` is where settings changed over IPC are saved and where manual edits are
    /// picked up from.
    pub fn new(config: Config, config_path: Option<PathBuf>) -> std::io::Result<Self> {
        let config_modified = config_path.as_deref().and_then(modified);
        Ok(Self {
            state: Mutex::new(State {
                config,
                config_modified,
                status: Status::default(),
            }),
            config_path,
            wake: Event::new(false)?,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> Status {
        let state = self.lock();
        Status {
            config: state.config.clone(),
            ..state.status.clone()
        }
    }

    /// Handles one IPC request (see [`crate::ipc`]).
    pub fn handle_request(&self, request: &str) -> String {
        if request == "status" {
            return format!("ok\n{}", self.status().encode());
        }
        let Some((key, value)) = request
            .strip_prefix("set ")
            .and_then(|kv| kv.split_once('='))
        else {
            return "error: unknown request".into();
        };
        let (key, value) = (key.trim(), value.trim());

        let mut state = self.lock();
        let mut config = state.config.clone();
        if let Err(e) = config.set(key, value) {
            return format!("error: {e}");
        }
        if let Some(path) = &self.config_path {
            if let Err(e) = config.save(path) {
                return format!("error: cannot save {}: {e}", path.display());
            }
            state.config_modified = modified(path);
        }
        info!("setting changed: {key} = {value}");
        state.config = config;
        drop(state);
        self.wake.set();
        "ok".into()
    }

    /// Runs until `stop` is signaled.
    pub fn run(&self, stop: &Event) {
        let mut usage = CpuUsage::new();
        let mut temp = match CpuTemp::open() {
            Ok(sensor) => {
                info!("CPU temperature: {}", sensor.source());
                Some(sensor)
            }
            Err(e) => {
                warn!("CPU temperature unavailable, showing usage instead: {e}");
                self.lock().status.temp_error = Some(e.to_string());
                None
            }
        };
        let mut temp_failing = false;

        let mut coolers: Vec<Cooler> = Vec::new();
        let mut next_scan = Instant::now();
        let started = Instant::now();

        loop {
            self.reload_config_if_edited();
            let config = self.lock().config.clone();

            if coolers.is_empty() && Instant::now() >= next_scan {
                coolers = open_coolers();
                next_scan = Instant::now() + RESCAN_INTERVAL;
            }

            let cpu_usage = usage.sample();
            let cpu_temp = temp.as_mut().and_then(|sensor| match sensor.read() {
                Ok(t) => {
                    temp_failing = false;
                    Some(t)
                }
                Err(e) => {
                    if !temp_failing {
                        warn!("CPU temperature read failed: {e}");
                        temp_failing = true;
                    }
                    None
                }
            });

            let reading = choose_reading(&config, cpu_temp, cpu_usage, started.elapsed());
            let alarm =
                config.alarm && cpu_temp.is_some_and(|t| t >= f32::from(config.alarm_threshold));
            coolers.retain_mut(|cooler| match cooler.show(reading, alarm) {
                Ok(()) => true,
                Err(e) => {
                    warn!("{} disconnected: {e}", cooler.name());
                    false
                }
            });

            {
                let mut state = self.lock();
                state.status.devices = coolers.iter().map(|c| c.name().to_owned()).collect();
                let status = &mut state.status;
                status.cpu_temp = cpu_temp;
                status.cpu_usage = Some(cpu_usage);
                status.shown = Some(match reading {
                    Reading::Temperature { .. } => Shown::Temperature,
                    Reading::Usage { .. } => Shown::Usage,
                    Reading::Custom { .. } => Shown::Custom,
                });
                status.alarm_active = alarm;
                if status.history.len() == HISTORY_LEN {
                    status.history.remove(0);
                }
                status.history.push(Sample {
                    cpu_temp,
                    cpu_usage,
                });
            }

            let interval = Duration::from_millis(config.interval_ms.into());
            if wait_any(&[stop, &self.wake], interval) == Some(0) {
                break;
            }
        }
        info!("stopped");
    }

    fn reload_config_if_edited(&self) {
        let Some(path) = &self.config_path else {
            return;
        };
        let current = modified(path);
        let mut state = self.lock();
        if current.is_none() || current == state.config_modified {
            return;
        }
        state.config_modified = current;
        match Config::load(path) {
            Ok(config) if config != state.config => {
                info!("reloaded {}", path.display());
                state.config = config;
            }
            Ok(_) => {}
            Err(e) => warn!("ignoring invalid {}: {e}", path.display()),
        }
    }
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn open_coolers() -> Vec<Cooler> {
    let detected = match device::detect() {
        Ok(d) => d,
        Err(e) => {
            warn!("device enumeration failed: {e}");
            return Vec::new();
        }
    };
    let mut coolers = Vec::new();
    for d in &detected {
        let Some(model) = d.model else {
            info!(
                "ignoring unsupported DeepCool device {:04X} '{}'",
                d.info.product_id, d.info.product
            );
            continue;
        };
        match Cooler::open(d).and_then(|mut c| c.init().map(|()| c)) {
            Ok(cooler) => {
                info!("connected {} (serial {})", model.name, cooler.serial());
                coolers.push(cooler);
            }
            Err(e) => warn!("cannot open {}: {e}", model.name),
        }
    }
    coolers
}

/// Picks what to show. Without a temperature sensor every mode falls back to usage.
pub fn choose_reading(
    config: &Config,
    cpu_temp: Option<f32>,
    cpu_usage: f32,
    elapsed: Duration,
) -> Reading {
    let usage = Reading::Usage { percent: cpu_usage };
    let temperature = |celsius| Reading::Temperature {
        celsius,
        unit: config.unit,
    };
    match (config.mode, cpu_temp) {
        (Mode::Custom, _) => Reading::Custom {
            value: config.custom_value,
            symbol: config.custom_symbol,
            bar: config.custom_bar,
        },
        (_, None) | (Mode::Usage, _) => usage,
        (Mode::Temperature, Some(t)) => temperature(t),
        (Mode::Auto, Some(t)) => {
            let phase = elapsed.as_secs() / u64::from(config.auto_interval_s.max(1));
            if phase.is_multiple_of(2) {
                temperature(t)
            } else {
                usage
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Symbol, Unit};

    fn config(mode: Mode) -> Config {
        Config {
            mode,
            auto_interval_s: 5,
            ..Config::default()
        }
    }

    #[test]
    fn modes() {
        let at = Duration::ZERO;
        let temp = Reading::Temperature {
            celsius: 50.0,
            unit: Unit::Celsius,
        };
        let usage = Reading::Usage { percent: 20.0 };
        assert_eq!(
            choose_reading(&config(Mode::Temperature), Some(50.0), 20.0, at),
            temp
        );
        assert_eq!(
            choose_reading(&config(Mode::Usage), Some(50.0), 20.0, at),
            usage
        );
        assert_eq!(
            choose_reading(&config(Mode::Temperature), None, 20.0, at),
            usage
        );
    }

    #[test]
    fn auto_alternates() {
        let c = config(Mode::Auto);
        let show = |secs| choose_reading(&c, Some(50.0), 20.0, Duration::from_secs(secs));
        assert!(matches!(show(0), Reading::Temperature { .. }));
        assert!(matches!(show(4), Reading::Temperature { .. }));
        assert!(matches!(show(5), Reading::Usage { .. }));
        assert!(matches!(show(10), Reading::Temperature { .. }));
        assert!(matches!(
            choose_reading(&c, None, 20.0, Duration::ZERO),
            Reading::Usage { .. }
        ));
    }

    #[test]
    fn custom_ignores_the_sensors() {
        let c = Config {
            custom_value: 42,
            custom_symbol: Symbol::Percent,
            custom_bar: 3,
            ..config(Mode::Custom)
        };
        let expected = Reading::Custom {
            value: 42,
            symbol: Symbol::Percent,
            bar: 3,
        };
        assert_eq!(
            choose_reading(&c, Some(50.0), 20.0, Duration::ZERO),
            expected
        );
        assert_eq!(choose_reading(&c, None, 20.0, Duration::ZERO), expected);
    }

    #[test]
    fn set_request_updates_config() {
        let engine = Engine::new(Config::default(), None).unwrap();
        assert_eq!(engine.handle_request("set mode=usage"), "ok");
        assert_eq!(engine.status().config.mode, Mode::Usage);
        assert!(
            engine
                .handle_request("set mode=disco")
                .starts_with("error:")
        );
        assert!(engine.handle_request("reboot").starts_with("error:"));
        assert!(engine.handle_request("status").starts_with("ok\n"));
    }
}
