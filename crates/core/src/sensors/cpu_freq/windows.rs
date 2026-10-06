//! CPU frequency on Windows, from the same PDH counters Task Manager uses:
//! `Processor Frequency` (the nominal MHz) × `% Processor Performance` / 100.
//!
//! `pdh.dll` is loaded at runtime, only when a connected display shows the frequency.

use std::ffi::c_void;
use std::io;
use std::ptr;

use crate::win::{Library, wide};

const PERFORMANCE: &str = r"\Processor Information(_Total)\% Processor Performance";
const FREQUENCY: &str = r"\Processor Information(_Total)\Processor Frequency";

const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
/// `% Processor Performance` goes above 100 while boosting.
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;

type Handle = *mut c_void;
type OpenQueryFn = unsafe extern "system" fn(*const u16, usize, *mut Handle) -> u32;
type AddCounterFn = unsafe extern "system" fn(Handle, *const u16, usize, *mut Handle) -> u32;
type CollectFn = unsafe extern "system" fn(Handle) -> u32;
type FormattedValueFn = unsafe extern "system" fn(Handle, u32, *mut u32, *mut CounterValue) -> u32;
type CloseQueryFn = unsafe extern "system" fn(Handle) -> u32;

/// `PDH_FMT_COUNTERVALUE` with the double member of its union.
#[repr(C)]
struct CounterValue {
    status: u32,
    value: f64,
}

/// Reads the average CPU frequency in MHz.
pub struct CpuFreq {
    collect: CollectFn,
    formatted_value: FormattedValueFn,
    close_query: CloseQueryFn,
    query: Handle,
    performance: Handle,
    frequency: Handle,
    // Last: the functions above live in this library.
    _pdh: Library,
}

impl CpuFreq {
    pub fn open() -> io::Result<Self> {
        let pdh = Library::system("pdh.dll")?;
        // SAFETY: the types match the documented signatures of these exports.
        let symbols = unsafe {
            (|| {
                Some((
                    pdh.symbol::<OpenQueryFn>(c"PdhOpenQueryW")?,
                    pdh.symbol::<AddCounterFn>(c"PdhAddEnglishCounterW")?,
                    pdh.symbol::<CollectFn>(c"PdhCollectQueryData")?,
                    pdh.symbol::<FormattedValueFn>(c"PdhGetFormattedCounterValue")?,
                    pdh.symbol::<CloseQueryFn>(c"PdhCloseQuery")?,
                ))
            })()
        };
        let (open_query, add_counter, collect, formatted_value, close_query) =
            symbols.ok_or_else(|| io::Error::other("pdh.dll is missing expected exports"))?;

        let mut query: Handle = ptr::null_mut();
        check(
            unsafe { open_query(ptr::null(), 0, &mut query) },
            "PdhOpenQuery",
        )?;
        // From here on `Drop` closes the query, and with it the counters.
        let mut sensor = Self {
            collect,
            formatted_value,
            close_query,
            query,
            performance: ptr::null_mut(),
            frequency: ptr::null_mut(),
            _pdh: pdh,
        };
        for (path, counter) in [
            (PERFORMANCE, &mut sensor.performance),
            (FREQUENCY, &mut sensor.frequency),
        ] {
            let path = wide(path);
            check(
                unsafe { add_counter(query, path.as_ptr(), 0, counter) },
                "PdhAddEnglishCounter",
            )?;
        }
        // Rate counters need two samples: this is the first one.
        check(unsafe { (sensor.collect)(query) }, "PdhCollectQueryData")?;
        Ok(sensor)
    }

    pub fn read(&mut self) -> io::Result<f32> {
        check(unsafe { (self.collect)(self.query) }, "PdhCollectQueryData")?;
        let performance = self.value(self.performance, PDH_FMT_NOCAP100)?;
        let nominal = self.value(self.frequency, 0)?;
        Ok(mhz(performance, nominal))
    }

    fn value(&self, counter: Handle, flags: u32) -> io::Result<f64> {
        let mut value = CounterValue {
            status: 0,
            value: 0.0,
        };
        let status = unsafe {
            (self.formatted_value)(counter, PDH_FMT_DOUBLE | flags, ptr::null_mut(), &mut value)
        };
        check(status, "PdhGetFormattedCounterValue")?;
        check(value.status, "PDH counter")?;
        Ok(value.value)
    }

    /// Human-readable description of the sensor in use.
    pub fn source(&self) -> &'static str {
        "PDH Processor Information"
    }
}

impl Drop for CpuFreq {
    fn drop(&mut self) {
        unsafe { (self.close_query)(self.query) };
    }
}

/// Effective frequency from the performance percentage and the nominal frequency.
pub fn mhz(performance_percent: f64, nominal_mhz: f64) -> f32 {
    (nominal_mhz * performance_percent / 100.0) as f32
}

fn check(status: u32, what: &str) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!("{what} failed (0x{status:08X})")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_above_nominal() {
        assert_eq!(mhz(125.0, 3600.0), 4500.0);
        assert_eq!(mhz(0.0, 3600.0), 0.0);
    }

    #[test]
    fn live_reading() {
        let mut sensor = CpuFreq::open().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mhz = sensor.read().unwrap();
        assert!((100.0..10_000.0).contains(&mhz), "{mhz} MHz");
    }
}
