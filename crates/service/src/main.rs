//! deepcool-native: shows CPU temperature and usage on DeepCool cooler displays.

mod service;

use std::error::Error;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;
use std::{env, io};

use deepcool_core::config::{Config, Unit};
use deepcool_core::device::{self, Cooler, Reading};
use deepcool_core::engine::Engine;
use deepcool_core::sensors::cpu_temp::CpuTemp;
use deepcool_core::sensors::cpu_usage::CpuUsage;
use deepcool_core::win::{self, Event};
use deepcool_core::{ipc, log, paths};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::core::BOOL;

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "\
Usage: deepcool-native <command>

Commands:
  list              Show detected coolers and sensor readings
  test              Play a test pattern on the connected coolers
  run               Drive the coolers in the foreground (Ctrl+C to stop)
  status            Show what the running service is doing
  set <key=value>   Change a setting of the running service, e.g. `set mode=auto`
  install           Install and start the Windows service (administrator)
  uninstall         Stop and remove the Windows service (administrator)
  start | stop      Start or stop the installed service (administrator)
  version           Print the version

Settings: mode (temperature|usage|auto), unit (celsius|fahrenheit), alarm (on|off),
          alarm_threshold (°C), interval_ms, auto_interval_s
";

const OFFICIAL_APP: &str = "DeepCool.exe";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().map_or("help", String::as_str);
    let result = match command {
        "list" => list(),
        "test" => test_pattern(),
        "run" => run_foreground(),
        "status" => status(),
        "set" => set(&args[1..]),
        "install" => service::install(),
        "uninstall" => service::uninstall(),
        "start" => service::start(),
        "stop" => service::stop(),
        "service" => service::run(),
        "version" | "--version" | "-V" => {
            println!("deepcool-native {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command '{other}'\n\n{USAGE}").into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn warn_if_official_app_running() {
    if win::process_running(OFFICIAL_APP) {
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
        println!(
            "  {:<16} {} (serial {}), VID {:04X} PID {:04X}, report ID {}, {}-byte reports",
            d.model.map_or("unsupported", |m| m.name),
            i.product,
            if i.serial.is_empty() { "-" } else { &i.serial },
            i.vendor_id,
            i.product_id,
            i.report_id,
            i.output_report_len,
        );
    }

    println!("CPU temperature");
    match CpuTemp::open().and_then(|mut t| Ok((t.read()?, t.source()))) {
        Ok((celsius, source)) => println!("  {celsius:.0} °C from {source}"),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            println!("  unavailable: {e}\n  Run this command from an elevated terminal.")
        }
        Err(e) => println!("  unavailable: {e}"),
    }

    let mut usage = CpuUsage::new();
    thread::sleep(Duration::from_millis(500));
    println!("CPU usage\n  {:.0} %", usage.sample());
    Ok(())
}

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

    let mut send = |label: &str, reading: Option<Reading>, alarm: bool, hold_ms: u64| -> Result {
        println!("{label}");
        for cooler in &mut coolers {
            match reading {
                Some(r) => cooler.show(r, alarm)?,
                None => cooler.init()?,
            }
        }
        thread::sleep(Duration::from_millis(hold_ms));
        Ok(())
    };

    send("init: bar animation", None, false, 2500)?;
    for percent in (0..=100).step_by(10) {
        let reading = Reading::Usage {
            percent: percent as f32,
        };
        send(&format!("usage {percent} %"), Some(reading), false, 400)?;
    }
    let celsius = |celsius, unit| Some(Reading::Temperature { celsius, unit });
    send("41 °C", celsius(41.0, Unit::Celsius), false, 2000)?;
    send(
        "106 °F (41 °C)",
        celsius(41.0, Unit::Fahrenheit),
        false,
        2000,
    )?;
    send("92 °C with alarm", celsius(92.0, Unit::Celsius), true, 4000)?;
    send(
        "all segments: 888",
        Some(Reading::Usage { percent: 888.0 }),
        false,
        2000,
    )?;
    println!("done");
    Ok(())
}

static CONSOLE_STOP: OnceLock<Arc<Event>> = OnceLock::new();

unsafe extern "system" fn on_console_ctrl(_ctrl_type: u32) -> BOOL {
    if let Some(stop) = CONSOLE_STOP.get() {
        stop.set();
    }
    1
}

fn run_foreground() -> Result {
    warn_if_official_app_running();
    log::init(None, true);

    let config_path = paths::config_file();
    let config = Config::load(&config_path)?;
    let engine = Arc::new(Engine::new(config, Some(config_path))?);

    let ipc_engine = Arc::clone(&engine);
    ipc::serve(move |request| ipc_engine.handle_request(request)).map_err(|e| {
        format!("cannot open the control pipe ({e}); is the service already running?")
    })?;

    let stop = Arc::new(Event::new(true)?);
    let _ = CONSOLE_STOP.set(Arc::clone(&stop));
    unsafe { SetConsoleCtrlHandler(Some(on_console_ctrl), 1) };

    engine.run(&stop);
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
