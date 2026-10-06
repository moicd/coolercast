//! coolercast: CLI and service of CoolerCast, which shows CPU temperature and usage on the
//! display of DeepCool coolers. On Windows it is also the Windows service; on Linux systemd runs
//! `coolercast run`.

#[cfg(windows)]
mod service;

use std::error::Error;
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;
use std::{env, io};

use coolercast_core::config::{Config, Unit};
use coolercast_core::device::{self, Cooler, Reading, Readings, Update, ak};
use coolercast_core::engine::Engine;
use coolercast_core::sensors::cpu_freq::CpuFreq;
use coolercast_core::sensors::cpu_power::CpuPower;
use coolercast_core::sensors::cpu_temp::CpuTemp;
use coolercast_core::sensors::cpu_usage::CpuUsage;
use coolercast_core::{error, ipc, log, paths};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "\
Usage: coolercast <command>

Commands:
  list              Show detected coolers and sensor readings
  test              Play a test pattern on the connected coolers
  probe <what>      Step through undocumented display values (service stopped):
                    probe mode|bar|digit [from] [to], or probe raw <6 bytes>
                    (AK and LS series only)
  run               Drive the coolers in the foreground (Ctrl+C to stop)
  status            Show what the running service is doing
  set <key=value>   Change a setting of the running service, e.g. `set mode=auto`
{service}  version           Print the version

Settings: mode (temperature|usage|auto|power|custom), unit (celsius|fahrenheit), alarm (on|off),
          alarm_threshold (°C), interval_ms, auto_interval_s,
          custom_value (0-999), custom_symbol (celsius|fahrenheit|percent), custom_bar (1-10)
";

#[cfg(windows)]
const SERVICE_USAGE: &str = "\
  install           Install and start the Windows service (administrator)
  uninstall         Stop and remove the Windows service (administrator)
  start | stop      Start or stop the installed service (administrator)
";

#[cfg(target_os = "linux")]
const SERVICE_USAGE: &str = "";

/// How to stop and start the service, for messages.
#[cfg(windows)]
const STOP_HINT: &str = "`coolercast stop` (as administrator)";
#[cfg(target_os = "linux")]
const STOP_HINT: &str = "`sudo systemctl stop coolercast`";
#[cfg(windows)]
const START_HINT: &str = "`coolercast start` (as administrator)";
#[cfg(target_os = "linux")]
const START_HINT: &str = "`sudo systemctl start coolercast`";

fn usage() -> String {
    USAGE.replace("{service}", SERVICE_USAGE)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().map_or("help", String::as_str);
    let result = match command {
        "list" => list(),
        "test" => test_pattern(),
        "probe" => probe(&args[1..]),
        "run" => run_foreground(),
        "status" => status(),
        "set" => set(&args[1..]),
        #[cfg(windows)]
        "install" => service::install(),
        #[cfg(windows)]
        "uninstall" => service::uninstall(),
        #[cfg(windows)]
        "start" => service::start(),
        #[cfg(windows)]
        "stop" => service::stop(),
        #[cfg(windows)]
        "service" => service::run(),
        #[cfg(target_os = "linux")]
        "install" | "uninstall" | "start" | "stop" => Err(
            "on Linux, install CoolerCast with the install.sh script from the release and manage \
             the service with `systemctl`"
                .into(),
        ),
        "version" | "--version" | "-V" => {
            println!("coolercast {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print!("{}", usage());
            Ok(())
        }
        other => Err(format!("unknown command '{other}'\n\n{}", usage()).into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "linux")]
fn warn_if_official_app_running() {}

#[cfg(windows)]
fn warn_if_official_app_running() {
    if coolercast_core::win::process_running("DeepCool.exe") {
        eprintln!(
            "warning: the official DeepCool app is running and will overwrite the display.\n\
             Close it from its tray icon and disable it at startup.\n"
        );
    }
}

fn list() -> Result {
    let detected = device::detect()?;
    println!("Coolers");
    if detected.is_empty() {
        println!("  none found");
    }
    for d in &detected {
        let i = &d.info;
        let name = match d.model {
            Some(m) if m.experimental => format!("{} (experimental)", m.name),
            Some(m) => m.name.to_owned(),
            None => "unsupported".to_owned(),
        };
        println!(
            "  {name:<16} {} (serial {}), VID {:04X} PID {:04X}, report ID {}, {}-byte reports",
            i.product,
            if i.serial.is_empty() { "-" } else { &i.serial },
            i.vendor_id,
            i.product_id,
            i.report_id,
            i.output_report_len,
        );
    }

    println!("CPU temperature");
    match CpuTemp::open().and_then(|mut t| Ok((t.read()?, t.source().to_owned()))) {
        Ok((celsius, source)) => println!("  {celsius:.0} °C from {source}"),
        Err(e) if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied => {
            println!("  unavailable: {e}\n  Run this command from an elevated terminal.")
        }
        Err(e) => println!("  unavailable: {e}"),
    }

    // Usage, power and frequency are averages: sample them over the same half second.
    let mut usage = CpuUsage::new();
    let power = CpuPower::open();
    let freq = CpuFreq::open();
    thread::sleep(Duration::from_millis(500));
    println!("CPU usage\n  {:.0} %", usage.sample());

    println!("CPU power (LS, LD, LQ and PRO displays)");
    match power.and_then(|mut p| Ok((p.read()?, p.source().to_owned()))) {
        Ok((Some(watts), source)) => println!("  {watts:.0} W from {source}"),
        Ok((None, source)) => println!("  no reading yet from {source}"),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            println!("  unavailable: {e}\n  Run this command {ELEVATED}.")
        }
        Err(e) => println!("  unavailable: {e}"),
    }

    println!("CPU frequency (AK500/AK620 PRO and LQ displays)");
    match freq.and_then(|mut f| Ok((f.read()?, f.source()))) {
        Ok((mhz, source)) => println!("  {mhz:.0} MHz from {source}"),
        Err(e) => println!("  unavailable: {e}"),
    }
    Ok(())
}

#[cfg(windows)]
const ELEVATED: &str = "from an elevated terminal";
#[cfg(target_os = "linux")]
const ELEVATED: &str = "with sudo";

fn test_pattern() -> Result {
    warn_if_official_app_running();
    let mut coolers: Vec<Cooler> = device::detect()?
        .iter()
        .filter(|d| d.model.is_some())
        .map(Cooler::open)
        .collect::<io::Result<_>>()?;
    if coolers.is_empty() {
        return Err("no supported cooler found".into());
    }
    for cooler in &coolers {
        println!("Testing {}", cooler.name());
    }

    let mut send = |label: &str, update: Option<Update>, hold_ms: u64| -> Result {
        println!("{label}");
        for cooler in &mut coolers {
            match &update {
                Some(update) => cooler.show(update)?,
                None => cooler.init()?,
            }
        }
        thread::sleep(Duration::from_millis(hold_ms));
        Ok(())
    };

    send("init: bar animation", None, 2500)?;
    // Every value moves together, so displays that show several values change everywhere.
    for percent in (0..=100).step_by(10) {
        let usage = percent as f32;
        let readings = test_readings(30.0 + usage * 0.6, usage, usage * 2.0, 800.0 + usage * 40.0);
        let label = format!(
            "usage {percent} %, {:.0} °C, {:.0} W, {:.0} MHz",
            readings.cpu_temp.unwrap_or_default(),
            readings.cpu_power.unwrap_or_default(),
            readings.cpu_freq.unwrap_or_default(),
        );
        let reading = Reading::Usage { percent: usage };
        send(
            &label,
            Some(test_update(reading, readings, Unit::Celsius, false)),
            400,
        )?;
    }

    let idle = test_readings(41.0, 12.0, 35.0, 3600.0);
    let temperature = |celsius, unit| Reading::Temperature { celsius, unit };
    let shown = temperature(41.0, Unit::Celsius);
    send(
        "41 °C",
        Some(test_update(shown, idle, Unit::Celsius, false)),
        2000,
    )?;
    let shown = temperature(41.0, Unit::Fahrenheit);
    let update = test_update(shown, idle, Unit::Fahrenheit, false);
    send("106 °F (41 °C)", Some(update), 2000)?;
    let busy = test_readings(41.0, 95.0, 142.0, 4800.0);
    let update = test_update(Reading::Power { watts: 142.0 }, busy, Unit::Celsius, false);
    send(
        "142 W (power mode: LS series; others show 41 °C)",
        Some(update),
        2000,
    )?;
    let hot = test_readings(92.0, 100.0, 180.0, 5000.0);
    let update = test_update(temperature(92.0, Unit::Celsius), hot, Unit::Celsius, true);
    send("92 °C with alarm", Some(update), 4000)?;
    let eights = test_readings(888.0, 100.0, 888.0, 8888.0);
    let update = test_update(
        Reading::Usage { percent: 888.0 },
        eights,
        Unit::Celsius,
        false,
    );
    send("all segments: 888", Some(update), 2000)?;
    println!("done");
    Ok(())
}

fn test_readings(celsius: f32, usage: f32, watts: f32, mhz: f32) -> Readings {
    Readings {
        cpu_temp: Some(celsius),
        cpu_usage: usage,
        cpu_power: Some(watts),
        cpu_freq: Some(mhz),
    }
}

fn test_update(reading: Reading, readings: Readings, unit: Unit, alarm: bool) -> Update {
    Update {
        reading,
        readings,
        unit,
        alarm,
    }
}

/// Which payload byte `probe` steps through; the others keep values that make the change
/// visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeField {
    Mode,
    Bar,
    Digit,
}

impl ProbeField {
    /// Default range: everything for the mode byte, just past the documented values otherwise.
    fn range(self) -> (u8, u8) {
        match self {
            ProbeField::Mode => (0, 255),
            ProbeField::Bar => (0, 20),
            ProbeField::Digit => (10, 31),
        }
    }

    fn payload(self, value: u8) -> [u8; ak::PAYLOAD_LEN] {
        match self {
            // "123" and a half bar show which symbols and segments a mode lights.
            ProbeField::Mode => [value, 5, 1, 2, 3, 0],
            ProbeField::Bar => [19, value, 1, 2, 3, 0],
            // The same value in every position, in case leading digits are blanked.
            ProbeField::Digit => [19, 5, value, value, value, 0],
        }
    }
}

fn parse_probe(args: &[String]) -> Result<(ProbeField, u8, u8)> {
    let field = match args.first().map(String::as_str) {
        Some("mode") => ProbeField::Mode,
        Some("bar") => ProbeField::Bar,
        Some("digit") => ProbeField::Digit,
        _ => return Err("expected mode, bar, digit or raw".into()),
    };
    let (default_from, default_to) = field.range();
    let byte = |i: usize, default: u8| -> Result<u8> {
        args.get(i).map_or(Ok(default), |a| {
            a.parse()
                .map_err(|_| format!("'{a}' is not a byte (0-255)").into())
        })
    };
    let (from, to) = (byte(1, default_from)?, byte(2, default_to)?);
    if from > to {
        return Err(format!("empty range {from}-{to}").into());
    }
    Ok((field, from, to))
}

fn probe(args: &[String]) -> Result {
    if ipc::query_status().is_ok() {
        return Err(format!(
            "the service is running and would overwrite the display.\nStop it first with {STOP_HINT}."
        )
        .into());
    }
    warn_if_official_app_running();
    let mut coolers: Vec<Cooler> = device::detect()?
        .iter()
        .filter(|d| d.model.is_some_and(|m| m.family.is_ak_like()))
        .map(Cooler::open)
        .collect::<io::Result<_>>()?;
    if coolers.is_empty() {
        return Err(
            "no AK or LS series cooler found (probe only knows their report layout)".into(),
        );
    }
    let mut send = |payload: [u8; ak::PAYLOAD_LEN]| -> Result {
        for cooler in &mut coolers {
            cooler.send_raw(payload)?;
        }
        Ok(())
    };

    if args.first().map(String::as_str) == Some("raw") {
        let bytes: Vec<u8> = args[1..]
            .iter()
            .map(|a| a.parse::<u8>())
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| "raw expects 6 bytes (0-255): mode bar digit digit digit alarm")?;
        let payload: [u8; ak::PAYLOAD_LEN] = bytes
            .try_into()
            .map_err(|_| "raw expects 6 bytes: mode bar digit digit digit alarm")?;
        send(payload)?;
        println!("sent {payload:?}; the display keeps it until the next report");
        return Ok(());
    }

    let (field, from, to) = parse_probe(args)?;
    println!(
        "Sends undocumented values to the display, one at a time. If it stops responding,\n\
         unplug the cooler's USB cable (or restart the PC).\n\
         \n\
         Enter: next   -: previous   #N: jump to N   q: quit\n\
         Anything else is saved as a note for the current value; notes are printed at the end.\n"
    );
    let mut notes: Vec<(u8, [u8; ak::PAYLOAD_LEN], String)> = Vec::new();
    let mut value = from;
    let stdin = io::stdin();
    loop {
        let payload = field.payload(value);
        send(payload)?;
        print!("{field:?} = {value:<3}  payload {payload:?} > ");
        io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        match line.trim() {
            "q" => break,
            "-" => value = value.saturating_sub(1).max(from),
            jump if jump.starts_with('#') => match jump[1..].parse::<u8>() {
                Ok(n) if (from..=to).contains(&n) => value = n,
                _ => println!("  out of range {from}-{to}"),
            },
            input => {
                if !input.is_empty() {
                    notes.push((value, payload, input.to_owned()));
                }
                if value == to {
                    break;
                }
                value += 1;
            }
        }
    }

    for cooler in &mut coolers {
        cooler.init()?;
    }
    if !notes.is_empty() {
        println!("\n| {field:?} | Payload | Shows |\n|---|---|---|");
        for (value, payload, note) in &notes {
            println!("| {value} | `{payload:?}` | {note} |");
        }
    }
    println!("\nDone. Start the service again with {START_HINT}.");
    Ok(())
}

/// Loads the settings, creates the engine and serves IPC requests for it. The IPC result is
/// returned separately so callers can decide whether it is fatal.
fn start_engine() -> (Arc<Engine>, io::Result<()>) {
    // A few microseconds of work per refresh: efficiency cores and clocks are plenty.
    #[cfg(windows)]
    if let Err(e) = coolercast_core::win::set_efficiency_mode(true) {
        coolercast_core::warn!("efficiency mode unavailable: {e}");
    }
    let config_path = paths::config_file();
    let config = Config::load(&config_path).unwrap_or_else(|e| {
        error!("invalid {}, using defaults: {e}", config_path.display());
        Config::default()
    });
    if !config_path.exists() {
        // Fails without write access (e.g. `run` as a normal user); the defaults still apply.
        let _ = config.save(&config_path);
    }
    let engine = Arc::new(Engine::new(config, Some(config_path)));
    let ipc_engine = Arc::clone(&engine);
    let ipc = ipc::serve(move |request| ipc_engine.handle_request(request)).map(|_| ());
    (engine, ipc)
}

static FOREGROUND: OnceLock<Arc<Engine>> = OnceLock::new();

#[cfg(windows)]
fn stop_on_ctrl_c() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    use windows_sys::core::BOOL;

    unsafe extern "system" fn on_console_ctrl(_ctrl_type: u32) -> BOOL {
        if let Some(engine) = FOREGROUND.get() {
            engine.stop();
        }
        1
    }
    unsafe { SetConsoleCtrlHandler(Some(on_console_ctrl), 1) };
}

/// SIGINT and SIGTERM keep their default action: the process exits, which needs no cleanup.
#[cfg(target_os = "linux")]
fn stop_on_ctrl_c() {}

fn run_foreground() -> Result {
    warn_if_official_app_running();
    log::init(None, true);

    let (engine, ipc) = start_engine();
    match ipc {
        Ok(()) => {}
        // Another instance owns the control channel: two engines would fight over the display.
        Err(e) if cfg!(windows) || e.kind() == io::ErrorKind::AddrInUse => {
            return Err(format!(
                "cannot open the control channel ({e}); is the service already running?"
            )
            .into());
        }
        Err(e) => coolercast_core::warn!(
            "control socket unavailable, `status` and `set` will not work: {e}"
        ),
    }

    let _ = FOREGROUND.set(Arc::clone(&engine));
    stop_on_ctrl_c();
    engine.run();
    Ok(())
}

fn status() -> Result {
    let s = ipc::query_status().map_err(|e| format!("service not reachable: {e}"))?;
    let unit = s.config.unit;
    let devices = if s.devices.is_empty() {
        "none".to_owned()
    } else {
        s.devices.join(", ")
    };
    println!("Coolers      {devices}");
    match (s.cpu_temp, &s.temp_error) {
        (Some(t), _) => println!("Temperature  {:.0} {}", unit.from_celsius(t), unit.symbol()),
        (None, Some(e)) => println!("Temperature  unavailable: {e}"),
        (None, None) => println!("Temperature  -"),
    }
    match s.cpu_usage {
        Some(u) => println!("Usage        {u:.0} %"),
        None => println!("Usage        -"),
    }
    // Only measured while a display shows them.
    if let Some(w) = s.cpu_power {
        println!("Power        {w:.0} W");
    }
    if let Some(f) = s.cpu_freq {
        println!("Frequency    {f:.0} MHz");
    }
    println!();
    for (key, value) in s.config.entries() {
        println!("{key:<16} {value}");
    }
    Ok(())
}

fn set(args: &[String]) -> Result {
    if args.is_empty() {
        return Err("expected one or more key=value pairs".into());
    }
    for arg in args {
        let (key, value) = arg
            .split_once('=')
            .ok_or_else(|| format!("expected key=value, got '{arg}'"))?;
        ipc::set(key, value)?;
        println!("{key} = {value}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn probe_ranges_default_per_field() {
        assert_eq!(
            parse_probe(&args(&["digit"])).unwrap(),
            (ProbeField::Digit, 10, 31)
        );
        assert_eq!(
            parse_probe(&args(&["mode", "100", "120"])).unwrap(),
            (ProbeField::Mode, 100, 120)
        );
    }

    #[test]
    fn probe_rejects_bad_arguments() {
        assert!(parse_probe(&args(&[])).is_err());
        assert!(parse_probe(&args(&["leds"])).is_err());
        assert!(parse_probe(&args(&["bar", "300"])).is_err());
        assert!(parse_probe(&args(&["bar", "9", "3"])).is_err());
    }

    #[test]
    fn probe_payload_puts_the_value_in_place() {
        assert_eq!(ProbeField::Mode.payload(42), [42, 5, 1, 2, 3, 0]);
        assert_eq!(ProbeField::Bar.payload(0), [19, 0, 1, 2, 3, 0]);
        assert_eq!(ProbeField::Digit.payload(12), [19, 5, 12, 12, 12, 0]);
    }
}
