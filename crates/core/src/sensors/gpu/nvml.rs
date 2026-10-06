//! NVIDIA Management Library (`nvml.dll`, installed in System32 by the NVIDIA driver), loaded at
//! runtime. Reads the first NVIDIA GPU.

use std::ffi::{CStr, c_char, c_void};
use std::io;

use super::Values;
use crate::win::Library;

const NVML_SUCCESS: u32 = 0;
const NVML_TEMPERATURE_GPU: u32 = 0;
const NVML_CLOCK_GRAPHICS: u32 = 0;

type Device = *mut c_void;

/// `nvmlUtilization_t`.
#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: u32,
    memory: u32,
}

type InitFn = unsafe extern "C" fn() -> u32;
type ShutdownFn = unsafe extern "C" fn() -> u32;
type HandleByIndexFn = unsafe extern "C" fn(u32, *mut Device) -> u32;
type NameFn = unsafe extern "C" fn(Device, *mut c_char, u32) -> u32;
type TemperatureFn = unsafe extern "C" fn(Device, u32, *mut u32) -> u32;
type UtilizationFn = unsafe extern "C" fn(Device, *mut Utilization) -> u32;
type PowerFn = unsafe extern "C" fn(Device, *mut u32) -> u32;
type ClockFn = unsafe extern "C" fn(Device, u32, *mut u32) -> u32;

pub struct Nvml {
    device: Device,
    shutdown: ShutdownFn,
    name: NameFn,
    temperature: TemperatureFn,
    utilization: UtilizationFn,
    power: PowerFn,
    clock: ClockFn,
    // Last: the functions above live in this library.
    _library: Library,
}

impl Nvml {
    pub fn open() -> io::Result<Self> {
        let library = Library::system("nvml.dll")?;
        // SAFETY: the types match the signatures in nvml.h.
        let symbols = unsafe {
            (|| {
                Some((
                    library.symbol::<InitFn>(c"nvmlInit_v2")?,
                    library.symbol::<ShutdownFn>(c"nvmlShutdown")?,
                    library.symbol::<HandleByIndexFn>(c"nvmlDeviceGetHandleByIndex_v2")?,
                    library.symbol::<NameFn>(c"nvmlDeviceGetName")?,
                    library.symbol::<TemperatureFn>(c"nvmlDeviceGetTemperature")?,
                    library.symbol::<UtilizationFn>(c"nvmlDeviceGetUtilizationRates")?,
                    library.symbol::<PowerFn>(c"nvmlDeviceGetPowerUsage")?,
                    library.symbol::<ClockFn>(c"nvmlDeviceGetClockInfo")?,
                ))
            })()
        };
        let (init, shutdown, by_index, name, temperature, utilization, power, clock) =
            symbols.ok_or_else(|| io::Error::other("nvml.dll is missing expected exports"))?;

        check(unsafe { init() }, "nvmlInit")?;
        let mut device: Device = std::ptr::null_mut();
        if let Err(e) = check(
            unsafe { by_index(0, &mut device) },
            "nvmlDeviceGetHandleByIndex",
        ) {
            unsafe { shutdown() };
            return Err(e);
        }
        Ok(Self {
            device,
            shutdown,
            name,
            temperature,
            utilization,
            power,
            clock,
            _library: library,
        })
    }

    pub fn name(&self) -> Option<String> {
        let mut buf = [0 as c_char; 96];
        let status = unsafe { (self.name)(self.device, buf.as_mut_ptr(), buf.len() as u32) };
        (status == NVML_SUCCESS).then(|| {
            // SAFETY: NVML writes a null-terminated string that fits the buffer.
            unsafe { CStr::from_ptr(buf.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        })
    }

    /// Fields NVML cannot read on this card are `None`.
    pub fn read(&self) -> Values {
        let read = |f: &dyn Fn(*mut u32) -> u32| {
            let mut value = 0u32;
            (f(&mut value) == NVML_SUCCESS).then_some(value)
        };
        let mut utilization = Utilization::default();
        let usage = (unsafe { (self.utilization)(self.device, &mut utilization) } == NVML_SUCCESS)
            .then_some(utilization.gpu as f32);
        Values {
            temp: read(&|v| unsafe { (self.temperature)(self.device, NVML_TEMPERATURE_GPU, v) })
                .map(|c| c as f32),
            usage,
            power: read(&|v| unsafe { (self.power)(self.device, v) }).map(milliwatts_to_watts),
            freq: read(&|v| unsafe { (self.clock)(self.device, NVML_CLOCK_GRAPHICS, v) })
                .map(|mhz| mhz as f32),
        }
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        unsafe { (self.shutdown)() };
    }
}

fn milliwatts_to_watts(mw: u32) -> f32 {
    mw as f32 / 1000.0
}

fn check(status: u32, what: &str) -> io::Result<()> {
    if status == NVML_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{what} failed (NVML error {status})"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_units() {
        assert_eq!(milliwatts_to_watts(87_500), 87.5);
    }

    #[test]
    fn missing_library_is_an_error() {
        // Machines without an NVIDIA driver have no nvml.dll.
        if !std::path::Path::new(r"C:\Windows\System32\nvml.dll").is_file() {
            assert!(Nvml::open().is_err());
        }
    }
}
