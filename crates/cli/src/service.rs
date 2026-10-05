//! Windows service entry point and installation.

use std::env;
use std::ffi::{OsStr, OsString};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use coolercast_core::win;
use coolercast_core::{error, info, log, paths};
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::Result;

pub const SERVICE_NAME: &str = "coolercast";
const DISPLAY_NAME: &str = "CoolerCast";
const DESCRIPTION: &str = "Shows CPU temperature and usage on DeepCool cooler displays.";

const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_SERVICE_EXISTS: i32 = 1073;
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;

define_windows_service!(ffi_service_main, service_main);

/// Hands the process over to the service control manager.
pub fn run() -> Result {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(|e| {
        format!("{e}; this command is used by Windows to start the service, try `run`")
    })?;
    Ok(())
}

fn service_main(_arguments: Vec<OsString>) {
    log::init(Some(paths::log_file()), false);
    info!("coolercast {} starting", env!("CARGO_PKG_VERSION"));
    if let Err(e) = run_service() {
        error!("service failed: {e}");
    }
}

fn run_service() -> Result {
    let (engine, ipc) = crate::start_engine();
    if let Err(e) = ipc {
        error!("control pipe unavailable: {e}");
    }
    let handler_engine = Arc::clone(&engine);
    let status = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            handler_engine.stop();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let report = |state, controls_accepted| {
        status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(5),
            process_id: None,
        })
    };
    report(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
    )?;

    engine.run();
    report(ServiceState::Stopped, ServiceControlAccept::empty())?;
    Ok(())
}

fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
    ServiceManager::local_computer(None::<&str>, access).map_err(explain)
}

/// Adds a hint to the errors users can fix themselves.
fn explain(e: windows_service::Error) -> Box<dyn std::error::Error> {
    let code = match &e {
        windows_service::Error::Winapi(io) => io.raw_os_error(),
        _ => None,
    };
    match code {
        Some(ERROR_ACCESS_DENIED) => {
            "access denied; run this command from an elevated terminal (Run as administrator)"
                .into()
        }
        Some(ERROR_SERVICE_EXISTS) => "the service is already installed".into(),
        Some(ERROR_SERVICE_DOES_NOT_EXIST) => "the service is not installed".into(),
        _ => e.into(),
    }
}

pub fn install() -> Result {
    let manager = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: env::current_exe()?,
        launch_arguments: vec!["service".into()],
        dependencies: vec![],
        account_name: None, // LocalSystem: needed to talk to the PawnIO driver.
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .map_err(explain)?;
    service.set_description(DESCRIPTION)?;
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(5)
            };
            3
        ]),
    })?;
    service.start::<&OsStr>(&[]).map_err(explain)?;

    println!("Installed and started the '{SERVICE_NAME}' service.");
    println!("Executable: {}", info.executable_path.display());
    println!("Settings:   {}", paths::config_file().display());
    if win::process_running("DeepCool.exe") {
        println!(
            "\nThe official DeepCool app is still running and will overwrite the display.\n\
             Close it from its tray icon and disable it at startup."
        );
    }
    Ok(())
}

pub fn uninstall() -> Result {
    let manager = manager(ServiceManagerAccess::CONNECT)?;
    let service = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE,
        )
        .map_err(explain)?;
    stop_and_wait(&service)?;
    service.delete().map_err(explain)?;
    println!("Removed the '{SERVICE_NAME}' service.");
    Ok(())
}

pub fn start() -> Result {
    let manager = manager(ServiceManagerAccess::CONNECT)?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::START)
        .map_err(explain)?;
    service.start::<&OsStr>(&[]).map_err(explain)?;
    println!("Started.");
    Ok(())
}

pub fn stop() -> Result {
    let manager = manager(ServiceManagerAccess::CONNECT)?;
    let service = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
        )
        .map_err(explain)?;
    stop_and_wait(&service)?;
    println!("Stopped.");
    Ok(())
}

fn stop_and_wait(service: &windows_service::service::Service) -> Result {
    if service.query_status()?.current_state == ServiceState::Stopped {
        return Ok(());
    }
    service.stop().map_err(explain)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while service.query_status()?.current_state != ServiceState::Stopped {
        if Instant::now() > deadline {
            return Err("timed out waiting for the service to stop".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
