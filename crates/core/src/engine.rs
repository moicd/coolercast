//! The update loop: read sensors, build a report, send it to every cooler, sleep.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use crate::clock::LocalClock;
use crate::config::{Bar, ClockTime, Config, Mode, Source};
use crate::device::{self, Component, Cooler, Reading, Readings, Update};
use crate::ipc::{DisplayOff, HISTORY_LEN, Sample, Shown, Status};
use crate::sensors::Values;
use crate::sensors::cpu_freq::CpuFreq;
use crate::sensors::cpu_power::CpuPower;
use crate::sensors::cpu_temp::CpuTemp;
use crate::sensors::cpu_usage::CpuUsage;
use crate::sensors::gpu::Gpu;
use crate::{info, warn};

/// How often to look for coolers while one is missing.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);
/// How long after the start, or after losing a cooler, to keep looking for coolers although
/// others work: a display that enumerates late, or comes back after a USB reset or a resume from
/// sleep, takes a few seconds. With no cooler at all the search never ends.
const RESCAN_WINDOW: Duration = Duration::from_secs(60);

/// `source = "smart"` switches to the GPU above this usage...
const SMART_GPU_ON: f32 = 50.0;
/// ...and back to the CPU below this one.
const SMART_GPU_OFF: f32 = 30.0;
/// Shortest time `smart` keeps a component, so short spikes do not flip the display.
const SMART_HOLD: Duration = Duration::from_secs(3);

pub struct Engine {
    state: Mutex<State>,
    config_path: Option<PathBuf>,
    signal: Signal,
    /// Set by the Windows service from session and power notifications.
    locked: AtomicBool,
    screen_off: AtomicBool,
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
            locked: AtomicBool::new(false),
            screen_off: AtomicBool::new(false),
        }
    }

    /// Tells the engine whether the user session is locked.
    pub fn set_locked(&self, locked: bool) {
        self.locked.store(locked, Ordering::Relaxed);
        self.signal.raise(|f| f.wake = true);
    }

    /// Tells the engine whether the PC screen is off.
    pub fn set_screen_off(&self, off: bool) {
        self.screen_off.store(off, Ordering::Relaxed);
        self.signal.raise(|f| f.wake = true);
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
        let mut power = OnDemand::new("CPU power", |p: &CpuPower| p.source().to_owned());
        let mut freq = OnDemand::new("CPU frequency", |f: &CpuFreq| f.source().to_owned());
        let mut gpu = OnDemand::new("GPU", |g: &Gpu| format!("{} ({})", g.name(), g.source()));

        let mut coolers: Vec<Cooler> = Vec::new();
        let mut next_scan = Instant::now();
        let started = Instant::now();
        let mut retry_until = started + RESCAN_WINDOW;
        let mut smart = Smart::default();
        let mut clock = LocalClock::default();
        let mut was_off = None;

        loop {
            self.reload_config_if_edited();
            let config = self.lock().config.clone();

            if rescan_due(coolers.len(), Instant::now(), next_scan, retry_until) {
                open_new_coolers(&mut coolers);
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
                gpu: gpu
                    .sample(
                        coolers.iter().any(|c| c.family().uses_gpu(config.source)),
                        Gpu::open,
                        |sensor| sensor.read().map(Some),
                    )
                    .unwrap_or_default(),
            };
            let elapsed = started.elapsed();
            let gpu_available = readings.gpu.temp.is_some() || readings.gpu.usage.is_some();
            let smart = smart.update(readings.gpu.usage, elapsed);
            let component = choose_component(&config, gpu_available, smart, elapsed);
            let reading = choose_reading(&config, &readings.of(component), elapsed);
            let alarm =
                config.alarm && cpu_temp.is_some_and(|t| t >= f32::from(config.alarm_threshold));
            let update = Update {
                reading,
                component,
                readings,
                unit: config.unit,
                alarm,
                usage_bar: config.bar == Bar::Usage,
            };
            let now = if config.off_at_night {
                clock.now()
            } else {
                None
            };
            let off = display_off(
                &config,
                self.locked.load(Ordering::Relaxed),
                self.screen_off.load(Ordering::Relaxed),
                now,
            );
            if was_off != Some(off) {
                match off {
                    Some(reason) => info!("display off ({})", reason.as_str()),
                    None if was_off.is_some() => info!("display on"),
                    None => {}
                }
                was_off = Some(off);
            }
            // Without reports the display goes dark by itself within a few seconds.
            if off.is_none() {
                let open = coolers.len();
                coolers.retain_mut(|cooler| match cooler.show(&update) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("{} disconnected: {e}", cooler.name());
                        false
                    }
                });
                if coolers.len() < open {
                    retry_until = Instant::now() + RESCAN_WINDOW;
                }
            }

            {
                let mut state = self.lock();
                state.status.devices = coolers.iter().map(|c| c.name().to_owned()).collect();
                let status = &mut state.status;
                status.cpu_temp = cpu_temp;
                status.cpu_usage = Some(cpu_usage);
                status.cpu_power = readings.cpu_power;
                status.cpu_freq = readings.cpu_freq;
                status.gpu = readings.gpu;
                let gpu_name = gpu.get().map(Gpu::name);
                if status.gpu_name.as_deref() != gpu_name {
                    status.gpu_name = gpu_name.map(ToOwned::to_owned);
                }
                status.component = Some(component);
                status.display_off = off;
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

/// How long a sensor may keep failing before it is closed and opened again. Opening it again is
/// not tried more often than this either, so a GPU that is gone is not probed every refresh.
const REOPEN_AFTER: Duration = Duration::from_secs(10);

/// A sensor opened the first time a connected display needs it. If it cannot be opened, it is
/// not tried again until the service restarts. One that opened but then keeps failing to read,
/// like the `nvidia-smi` helper exiting after a driver reload, is closed after [`REOPEN_AFTER`]
/// and opened again, until it works.
struct OnDemand<T> {
    name: &'static str,
    /// What the log says about the sensor once it is open.
    describe: fn(&T) -> String,
    state: SensorState<T>,
    /// When the sensor, as open now, started failing to read.
    failing_since: Option<Instant>,
    /// Whether the current run of read errors is logged; it ends with a good read, not with a
    /// reopen, so a sensor that keeps failing is logged once.
    logged: bool,
}

enum SensorState<T> {
    Unopened,
    Open(T),
    Unavailable,
    /// Closed after failing; opened again once the time has come.
    Reopen(Instant),
}

impl<T> OnDemand<T> {
    fn new(name: &'static str, describe: fn(&T) -> String) -> Self {
        Self {
            name,
            describe,
            state: SensorState::Unopened,
            failing_since: None,
            logged: false,
        }
    }

    /// The sensor, if it is open.
    fn get(&self) -> Option<&T> {
        match &self.state {
            SensorState::Open(sensor) => Some(sensor),
            _ => None,
        }
    }

    /// Reads the sensor if `wanted`, opening it on first use. A run of read errors is logged
    /// once.
    fn sample<V>(
        &mut self,
        wanted: bool,
        open: impl FnOnce() -> io::Result<T>,
        read: impl FnOnce(&mut T) -> io::Result<Option<V>>,
    ) -> Option<V> {
        self.sample_at(Instant::now(), wanted, open, read)
    }

    fn sample_at<V>(
        &mut self,
        now: Instant,
        wanted: bool,
        open: impl FnOnce() -> io::Result<T>,
        read: impl FnOnce(&mut T) -> io::Result<Option<V>>,
    ) -> Option<V> {
        if !wanted {
            // Errors from before the pause do not count against the sensor afterwards.
            self.failing_since = None;
            return None;
        }
        let reopening = matches!(self.state, SensorState::Reopen(at) if now >= at);
        if reopening || matches!(self.state, SensorState::Unopened) {
            self.state = match open() {
                Ok(sensor) => {
                    if !reopening {
                        info!("{}: {}", self.name, (self.describe)(&sensor));
                    }
                    self.failing_since = None;
                    SensorState::Open(sensor)
                }
                // The read errors that closed the sensor are logged already.
                Err(_) if reopening => SensorState::Reopen(now + REOPEN_AFTER),
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
                self.failing_since = None;
                self.logged = false;
                value
            }
            Err(e) => {
                if !self.logged {
                    warn!("{} read failed: {e}", self.name);
                    self.logged = true;
                }
                let since = *self.failing_since.get_or_insert(now);
                if now - since >= REOPEN_AFTER {
                    self.state = SensorState::Reopen(now);
                }
                None
            }
        }
    }
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Whether to look for coolers now: when none is open, or until `retry_until`, but never more
/// often than `next_scan` allows, since enumerating does device I/O.
fn rescan_due(open: usize, now: Instant, next_scan: Instant, retry_until: Instant) -> bool {
    now >= next_scan && (open == 0 || now < retry_until)
}

/// Opens the supported coolers that are not open yet, so a working one is never opened twice.
/// The devices it skips are logged only while no cooler works, when they tell why; otherwise each
/// rescan after a lost cooler would log them again.
fn open_new_coolers(coolers: &mut Vec<Cooler>) {
    let verbose = coolers.is_empty();
    let detected = match device::detect() {
        Ok(d) => d,
        Err(e) => {
            warn!("device enumeration failed: {e}");
            return;
        }
    };
    for d in &detected {
        if coolers.iter().any(|c| c.path() == d.info.path) {
            continue;
        }
        let Some(model) = d.model else {
            if verbose {
                info!(
                    "ignoring unsupported DeepCool device {:04X} '{}'",
                    d.info.product_id, d.info.product
                );
            }
            continue;
        };
        match Cooler::open(d).and_then(|mut c| c.init().map(|()| c)) {
            Ok(cooler) => {
                info!("connected {} (serial {})", model.name, cooler.serial());
                coolers.push(cooler);
            }
            Err(e) if verbose => warn!("cannot open {}: {e}", model.name),
            Err(_) => {}
        }
    }
}

/// Why the display should be off now, if it should. `now` is the local time, when known.
pub fn display_off(
    config: &Config,
    locked: bool,
    screen_off: bool,
    now: Option<ClockTime>,
) -> Option<DisplayOff> {
    if config.off_when_locked && locked {
        Some(DisplayOff::Locked)
    } else if config.off_when_screen_off && screen_off {
        Some(DisplayOff::ScreenOff)
    } else if config.off_at_night
        && now.is_some_and(|t| ClockTime::in_range(t, config.night_start, config.night_end))
    {
        Some(DisplayOff::Night)
    } else {
        None
    }
}

/// The component `source = "smart"` shows: the GPU while it is busy, the CPU otherwise.
#[derive(Debug, Default)]
struct Smart {
    component: Component,
    /// When `component` was last changed.
    since: Duration,
}

impl Smart {
    fn update(&mut self, gpu_usage: Option<f32>, now: Duration) -> Component {
        let busy = match (self.component, gpu_usage) {
            (_, None) => false,
            (Component::Gpu, Some(u)) => u >= SMART_GPU_OFF,
            (Component::Cpu, Some(u)) => u >= SMART_GPU_ON,
        };
        let wanted = if busy { Component::Gpu } else { Component::Cpu };
        if wanted != self.component && now.saturating_sub(self.since) >= SMART_HOLD {
            self.component = wanted;
            self.since = now;
        }
        self.component
    }
}

/// The component single-value displays show now. Without GPU readings it is always the CPU.
pub fn choose_component(
    config: &Config,
    gpu_available: bool,
    smart: Component,
    elapsed: Duration,
) -> Component {
    match config.source {
        Source::Cpu => Component::Cpu,
        _ if !gpu_available => Component::Cpu,
        Source::Gpu => Component::Gpu,
        Source::Smart => smart,
        Source::Auto => {
            let phase = auto_phase(config, elapsed);
            // When the mode alternates too, each component shows both of its values in turn.
            let phase = if config.mode == Mode::Auto {
                phase / 2
            } else {
                phase
            };
            if phase.is_multiple_of(2) {
                Component::Cpu
            } else {
                Component::Gpu
            }
        }
    }
}

/// Picks what single-value displays show from one component's values. A missing value falls
/// back to the temperature, then to the usage.
pub fn choose_reading(config: &Config, values: &Values, elapsed: Duration) -> Reading {
    let temperature = values.temp.map(|celsius| Reading::Temperature {
        celsius,
        unit: config.unit,
    });
    let usage = values.usage.map(|percent| Reading::Usage { percent });
    let power = values.power.map(|watts| Reading::Power { watts });
    let reading = match config.mode {
        Mode::Custom => {
            return Reading::Custom {
                value: config.custom_value,
                symbol: config.custom_symbol,
                bar: config.custom_bar,
            };
        }
        Mode::Temperature => temperature.or(usage),
        Mode::Usage => usage.or(temperature),
        Mode::Power => power.or(temperature).or(usage),
        Mode::Auto if auto_phase(config, elapsed).is_multiple_of(2) => temperature.or(usage),
        Mode::Auto => usage.or(temperature),
    };
    reading.unwrap_or(Reading::Usage { percent: 0.0 })
}

/// How many `auto_interval_s` periods have passed.
fn auto_phase(config: &Config, elapsed: Duration) -> u64 {
    elapsed.as_secs() / u64::from(config.auto_interval_s.max(1))
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

    fn sensors(temp: Option<f32>, usage: f32) -> Values {
        Values {
            temp,
            usage: Some(usage),
            ..Values::default()
        }
    }

    #[test]
    fn power_mode_falls_back_to_temperature() {
        let at = Duration::ZERO;
        let c = config(Mode::Power);
        let with_power = Values {
            power: Some(65.0),
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
    fn gpu_values_without_usage_fall_back_to_temperature() {
        let gpu = Values {
            temp: Some(61.0),
            ..Values::default()
        };
        assert_eq!(
            choose_reading(&config(Mode::Usage), &gpu, Duration::ZERO),
            Reading::Temperature {
                celsius: 61.0,
                unit: Unit::Celsius
            }
        );
        assert_eq!(
            choose_reading(&config(Mode::Usage), &Values::default(), Duration::ZERO),
            Reading::Usage { percent: 0.0 }
        );
    }

    #[test]
    fn components() {
        let at = |secs| Duration::from_secs(secs);
        let with = |source, mode| Config {
            source,
            ..config(mode)
        };
        let gpu = with(Source::Gpu, Mode::Temperature);
        let cpu = Component::Cpu;
        assert_eq!(choose_component(&gpu, true, cpu, at(0)), Component::Gpu);
        assert_eq!(choose_component(&gpu, false, cpu, at(0)), Component::Cpu);
        let smart = with(Source::Smart, Mode::Temperature);
        let g = Component::Gpu;
        assert_eq!(choose_component(&smart, true, g, at(0)), Component::Gpu);
        assert_eq!(choose_component(&smart, false, g, at(0)), Component::Cpu);

        let auto = with(Source::Auto, Mode::Temperature);
        let shown: Vec<_> = [0, 5, 10]
            .map(|s| choose_component(&auto, true, cpu, at(s)))
            .into();
        assert_eq!(shown, [Component::Cpu, Component::Gpu, Component::Cpu]);

        // Both alternating: CPU temperature, CPU usage, GPU temperature, GPU usage.
        let both = with(Source::Auto, Mode::Auto);
        let shown: Vec<_> = [0, 5, 10, 15, 20]
            .map(|s| choose_component(&both, true, cpu, at(s)))
            .into();
        let (c, g) = (Component::Cpu, Component::Gpu);
        assert_eq!(shown, [c, c, g, g, c]);
    }

    #[test]
    fn sensors_open_on_demand_and_once() {
        let mut opens = 0;
        let mut sensor = OnDemand::<f32>::new("test", |v| v.to_string());
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

        let mut broken = OnDemand::<f32>::new("broken", |v| v.to_string());
        let fail = || Err(io::Error::other("no driver"));
        assert_eq!(broken.sample(true, fail, read), None);
        assert_eq!(broken.sample(true, || unreachable!(), read), None);
    }

    #[test]
    fn a_sensor_that_keeps_failing_is_opened_again() {
        use std::cell::Cell;
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let (up, opens) = (Cell::new(true), Cell::new(0));
        let open = || {
            opens.set(opens.get() + 1);
            if up.get() {
                Ok(opens.get())
            } else {
                Err(io::Error::other("driver gone"))
            }
        };
        let read = |opened: &mut u32| {
            if up.get() {
                Ok(Some(*opened))
            } else {
                Err(io::Error::other("exited"))
            }
        };
        let mut sensor = OnDemand::<u32>::new("flaky", |v| v.to_string());
        let mut sample = |secs| sensor.sample_at(at(secs), true, open, read);

        assert_eq!(sample(0), Some(1));
        up.set(false);
        // Failing for less than ten seconds: kept.
        assert_eq!((sample(1), sample(5), sample(10)), (None, None, None));
        assert_eq!(opens.get(), 1);
        // Closed at 11 s, opened again at the next sample, which fails.
        assert_eq!((sample(11), opens.get()), (None, 1));
        assert_eq!((sample(12), opens.get()), (None, 2));
        // Not again for ten seconds.
        assert_eq!((sample(13), sample(21)), (None, None));
        assert_eq!(opens.get(), 2);
        // The driver is back.
        up.set(true);
        assert_eq!((sample(22), opens.get()), (Some(3), 3));
        assert_eq!((sample(23), opens.get()), (Some(3), 3));
    }

    #[test]
    fn a_short_failure_does_not_close_the_sensor() {
        use std::cell::Cell;
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let (up, opens) = (Cell::new(true), Cell::new(0));
        let open = || {
            opens.set(opens.get() + 1);
            Ok(1.5)
        };
        let read = |v: &mut f32| {
            if up.get() {
                Ok(Some(*v))
            } else {
                Err(io::Error::other("blip"))
            }
        };
        let mut sensor = OnDemand::<f32>::new("blip", |v| v.to_string());
        let mut sample = |secs| sensor.sample_at(at(secs), true, open, read);

        assert_eq!(sample(0), Some(1.5));
        up.set(false);
        assert_eq!(sample(1), None);
        up.set(true);
        assert_eq!(sample(9), Some(1.5));
        // The run of errors started over: this one is not 11 s old.
        up.set(false);
        assert_eq!(sample(12), None);
        assert_eq!(sample(20), None);
        up.set(true);
        assert_eq!(sample(21), Some(1.5));
        assert_eq!(opens.get(), 1);
    }

    #[test]
    fn a_reopened_or_paused_sensor_gets_its_full_grace_time() {
        use std::cell::Cell;
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let (opens_ok, reads_ok, opens) = (Cell::new(true), Cell::new(true), Cell::new(0));
        let open = || {
            opens.set(opens.get() + 1);
            if opens_ok.get() {
                Ok(())
            } else {
                Err(io::Error::other("gone"))
            }
        };
        let read = |_: &mut ()| {
            if reads_ok.get() {
                Ok(Some(1))
            } else {
                Err(io::Error::other("failed"))
            }
        };
        let mut sensor = OnDemand::<()>::new("flaky", |_| String::new());
        let mut sample = |secs, wanted| sensor.sample_at(at(secs), wanted, open, read);

        assert_eq!(sample(0, true), Some(1));
        // Closed at 11 s; reopening at 12 s fails, so the next try is at 22 s.
        opens_ok.set(false);
        reads_ok.set(false);
        assert_eq!(
            (sample(1, true), sample(11, true), sample(12, true)),
            (None, None, None)
        );
        // At 22 s it opens but still cannot read: it gets ten seconds from now, not from 1 s.
        opens_ok.set(true);
        assert_eq!((sample(22, true), sample(31, true)), (None, None));
        assert_eq!(opens.get(), 3);
        assert_eq!((sample(32, true), opens.get()), (None, 3));
        reads_ok.set(true);
        assert_eq!((sample(33, true), opens.get()), (Some(1), 4));

        // A failure, a long pause while no display needs the sensor, then another failure: the
        // pause does not count as failing time.
        reads_ok.set(false);
        assert_eq!(sample(34, true), None);
        assert_eq!(sample(100, false), None);
        assert_eq!((sample(200, true), sample(205, true)), (None, None));
        reads_ok.set(true);
        assert_eq!((sample(206, true), opens.get()), (Some(1), 4));
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
    fn rescan_is_due_only_while_a_cooler_may_be_missing() {
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let window = at(RESCAN_WINDOW.as_secs());
        // Nothing open: scan every interval, even long after the window.
        assert!(rescan_due(0, at(0), at(0), t0));
        assert!(!rescan_due(0, at(2), at(5), t0));
        assert!(rescan_due(0, at(5), at(5), t0));
        assert!(rescan_due(0, at(3600), at(3595), t0));
        // One of two open, still inside the window: scan, but not before the interval.
        assert!(rescan_due(1, at(5), at(5), window));
        assert!(!rescan_due(1, at(4), at(5), window));
        assert!(rescan_due(1, at(59), at(55), window));
        // Window over: a working setup is not scanned any more.
        assert!(!rescan_due(1, window, at(5), window));
        assert!(!rescan_due(2, at(3600), at(5), window));
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

    #[test]
    fn smart_follows_a_busy_gpu_with_hysteresis_and_hold() {
        let at = Duration::from_secs;
        let mut smart = Smart::default();
        assert_eq!(smart.update(Some(40.0), at(10)), Component::Cpu);
        assert_eq!(smart.update(Some(80.0), at(11)), Component::Gpu);
        // Within the hold time a drop does not switch back.
        assert_eq!(smart.update(Some(5.0), at(12)), Component::Gpu);
        // Above the lower threshold it stays on the GPU.
        assert_eq!(smart.update(Some(35.0), at(20)), Component::Gpu);
        assert_eq!(smart.update(Some(10.0), at(21)), Component::Cpu);
        assert_eq!(smart.update(None, at(30)), Component::Cpu);
    }

    #[test]
    fn display_off_reasons() {
        let t = ClockTime::new;
        let all = Config {
            off_when_locked: true,
            off_when_screen_off: true,
            off_at_night: true,
            ..Config::default()
        };
        let day = Some(t(12, 0));
        let night = Some(t(1, 0));
        assert_eq!(
            display_off(&all, true, false, day),
            Some(DisplayOff::Locked)
        );
        assert_eq!(
            display_off(&all, false, true, day),
            Some(DisplayOff::ScreenOff)
        );
        assert_eq!(
            display_off(&all, false, false, night),
            Some(DisplayOff::Night)
        );
        assert_eq!(display_off(&all, false, false, day), None);
        // Unknown local time: the night schedule does not apply.
        assert_eq!(display_off(&all, false, false, None), None);
        // Disabled by default.
        let defaults = Config::default();
        assert_eq!(display_off(&defaults, true, true, night), None);
    }
}
