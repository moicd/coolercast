//! The update loop: read sensors, build a report, send it to every cooler, sleep.

use std::io;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use crate::config::{Config, Mode};
use crate::device::{self, Cooler, Reading, Readings, Update};
use crate::ipc::{HISTORY_LEN, Sample, Shown, Status};
use crate::sensors::cpu_freq::CpuFreq;
use crate::sensors::cpu_power::CpuPower;
use crate::sensors::cpu_temp::CpuTemp;
use crate::sensors::cpu_usage::CpuUsage;
use crate::{info, warn};

/// How often to look for coolers while none is connected.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);

pub struct Engine {
    state: Mutex<State>,
    config_path: Option<PathBuf>,
    signal: Signal,
}

/// Wakes the update loop early: to stop it, or to apply new settings right away.
#[derive(Default)]
struct Signal {
    flags: Mutex<Flags>,
    changed: Condvar,
}

#[derive(Default)]
struct Flags {
    stop: bool,
    wake: bool,
}

impl Signal {
    fn raise(&self, update: impl FnOnce(&mut Flags)) {
        update(&mut self.flags.lock().unwrap_or_else(|e| e.into_inner()));
        self.changed.notify_all();
    }

    /// Sleeps for `timeout` or until raised. Returns `true` if the loop must stop.
    fn wait(&self, timeout: Duration) -> bool {
        let flags = self.flags.lock().unwrap_or_else(|e| e.into_inner());
        let (mut flags, _) = self
            .changed
            .wait_timeout_while(flags, timeout, |f| !f.stop && !f.wake)
            .unwrap_or_else(|e| e.into_inner());
        flags.wake = false;
        flags.stop
    }
}

struct State {
    config: Config,
    config_modified: Option<SystemTime>,
    status: Status,
}

impl Engine {
    /// `config_path` is where settings changed over IPC are saved and where manual edits are
    /// picked up from.
    pub fn new(config: Config, config_path: Option<PathBuf>) -> Self {
        let config_modified = config_path.as_deref().and_then(modified);
        Self {
            state: Mutex::new(State {
                config,
                config_modified,
                status: Status::default(),
            }),
            config_path,
            signal: Signal::default(),
        }
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
        self.signal.raise(|f| f.wake = true);
        "ok".into()
    }

    /// Makes [`Engine::run`] return; safe to call from any thread.
    pub fn stop(&self) {
        self.signal.raise(|f| f.stop = true);
    }

    /// Runs until [`Engine::stop`] is called.
    pub fn run(&self) {
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
        let mut power = OnDemand::new("CPU power");
        let mut freq = OnDemand::new("CPU frequency");

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

            let readings = Readings {
                cpu_temp,
                cpu_usage,
                cpu_power: power.sample(
                    coolers.iter().any(|c| c.family().uses_power()),
                    CpuPower::open,
                    CpuPower::read,
                ),
                cpu_freq: freq.sample(
                    coolers.iter().any(|c| c.family().uses_frequency()),
                    CpuFreq::open,
                    |sensor| sensor.read().map(Some),
                ),
            };
            let reading = choose_reading(&config, &readings, started.elapsed());
            let alarm =
                config.alarm && cpu_temp.is_some_and(|t| t >= f32::from(config.alarm_threshold));
            let update = Update {
                reading,
                readings,
                unit: config.unit,
                alarm,
            };
            coolers.retain_mut(|cooler| match cooler.show(&update) {
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
                status.cpu_power = readings.cpu_power;
                status.cpu_freq = readings.cpu_freq;
                status.shown = Some(match reading {
                    Reading::Temperature { .. } => Shown::Temperature,
                    Reading::Usage { .. } => Shown::Usage,
                    Reading::Power { .. } => Shown::Power,
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
            if self.signal.wait(interval) {
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

/// A sensor opened the first time a connected display needs it. If it cannot be opened, it is
/// not tried again until the service restarts.
struct OnDemand<T> {
    name: &'static str,
    state: SensorState<T>,
    failing: bool,
}

enum SensorState<T> {
    Unopened,
    Open(T),
    Unavailable,
}

impl<T> OnDemand<T> {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            state: SensorState::Unopened,
            failing: false,
        }
    }

    /// Reads the sensor if `wanted`, opening it on first use. Errors are logged once.
    fn sample(
        &mut self,
        wanted: bool,
        open: impl FnOnce() -> io::Result<T>,
        read: impl FnOnce(&mut T) -> io::Result<Option<f32>>,
    ) -> Option<f32> {
        if !wanted {
            return None;
        }
        if let SensorState::Unopened = self.state {
            self.state = match open() {
                Ok(sensor) => {
                    info!("{} sensor opened", self.name);
                    SensorState::Open(sensor)
                }
                Err(e) => {
                    warn!("{} unavailable: {e}", self.name);
                    SensorState::Unavailable
                }
            };
        }
        let SensorState::Open(sensor) = &mut self.state else {
            return None;
        };
        match read(sensor) {
            Ok(value) => {
                self.failing = false;
                value
            }
            Err(e) => {
                if !self.failing {
                    warn!("{} read failed: {e}", self.name);
                    self.failing = true;
                }
                None
            }
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

/// Picks what single-value displays show. Without a power reading the power mode falls back to
/// the temperature, and without a temperature sensor every mode falls back to usage.
pub fn choose_reading(config: &Config, readings: &Readings, elapsed: Duration) -> Reading {
    let usage = Reading::Usage {
        percent: readings.cpu_usage,
    };
    let temperature = |celsius| Reading::Temperature {
        celsius,
        unit: config.unit,
    };
    match (config.mode, readings.cpu_temp) {
        (Mode::Custom, _) => Reading::Custom {
            value: config.custom_value,
            symbol: config.custom_symbol,
            bar: config.custom_bar,
        },
        (Mode::Power, temp) => match (readings.cpu_power, temp) {
            (Some(watts), _) => Reading::Power { watts },
            (None, Some(t)) => temperature(t),
            (None, None) => usage,
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

    fn sensors(cpu_temp: Option<f32>, cpu_usage: f32) -> Readings {
        Readings {
            cpu_temp,
            cpu_usage,
            ..Readings::default()
        }
    }

    #[test]
    fn power_mode_falls_back_to_temperature() {
        let at = Duration::ZERO;
        let c = config(Mode::Power);
        let with_power = Readings {
            cpu_power: Some(65.0),
            ..sensors(Some(50.0), 20.0)
        };
        assert_eq!(
            choose_reading(&c, &with_power, at),
            Reading::Power { watts: 65.0 }
        );
        assert_eq!(
            choose_reading(&c, &sensors(Some(50.0), 20.0), at),
            Reading::Temperature {
                celsius: 50.0,
                unit: Unit::Celsius
            }
        );
        assert_eq!(
            choose_reading(&c, &sensors(None, 20.0), at),
            Reading::Usage { percent: 20.0 }
        );
    }

    #[test]
    fn sensors_open_on_demand_and_once() {
        let mut opens = 0;
        let mut sensor = OnDemand::<f32>::new("test");
        let read = |v: &mut f32| Ok(Some(*v));
        assert_eq!(sensor.sample(false, || unreachable!(), read), None);
        for _ in 0..2 {
            let open = || {
                opens += 1;
                Ok(1.5)
            };
            assert_eq!(sensor.sample(true, open, read), Some(1.5));
        }
        assert_eq!(opens, 1);

        let mut broken = OnDemand::<f32>::new("broken");
        let fail = || Err(io::Error::other("no driver"));
        assert_eq!(broken.sample(true, fail, read), None);
        assert_eq!(broken.sample(true, || unreachable!(), read), None);
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
            choose_reading(&config(Mode::Temperature), &sensors(Some(50.0), 20.0), at),
            temp
        );
        assert_eq!(
            choose_reading(&config(Mode::Usage), &sensors(Some(50.0), 20.0), at),
            usage
        );
        assert_eq!(
            choose_reading(&config(Mode::Temperature), &sensors(None, 20.0), at),
            usage
        );
    }

    #[test]
    fn auto_alternates() {
        let c = config(Mode::Auto);
        let show = |secs| choose_reading(&c, &sensors(Some(50.0), 20.0), Duration::from_secs(secs));
        assert!(matches!(show(0), Reading::Temperature { .. }));
        assert!(matches!(show(4), Reading::Temperature { .. }));
        assert!(matches!(show(5), Reading::Usage { .. }));
        assert!(matches!(show(10), Reading::Temperature { .. }));
        assert!(matches!(
            choose_reading(&c, &sensors(None, 20.0), Duration::ZERO),
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
            choose_reading(&c, &sensors(Some(50.0), 20.0), Duration::ZERO),
            expected
        );
        assert_eq!(
            choose_reading(&c, &sensors(None, 20.0), Duration::ZERO),
            expected
        );
    }

    #[test]
    fn stop_wakes_the_loop() {
        let signal = Signal::default();
        signal.raise(|f| f.wake = true);
        assert!(
            !signal.wait(Duration::from_secs(5)),
            "a wake-up is not a stop"
        );
        signal.raise(|f| f.stop = true);
        assert!(signal.wait(Duration::from_secs(5)));
        assert!(!Signal::default().wait(Duration::from_millis(1)));
    }

    #[test]
    fn set_request_updates_config() {
        let engine = Engine::new(Config::default(), None);
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
