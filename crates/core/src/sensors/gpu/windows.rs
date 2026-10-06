//! GPU sensors on Windows.
//!
//! The adapter is picked with D3DKMT (`gdi32.dll`, loaded at runtime): hardware render adapters
//! only, so virtual displays and the Basic Render Driver are skipped, discrete vendors first.
//! NVIDIA cards are read through NVML. Other cards use the per-adapter performance data that
//! WDDM 2.4+ drivers report (temperature, engine clock) and the `GPU Engine` PDH counters for
//! the usage, computed like Task Manager: the busiest engine. Their power is only reported as a
//! percentage of the board limit, so it is not available in watts.

use std::ffi::c_void;
use std::io;

use super::nvml::Nvml;
use super::{Values, Vendor};
use crate::sensors::pdh::{Counter, Query};
use crate::win::{Library, from_wide};

const KMTQAITYPE_ADAPTERTYPE: i32 = 15;
const KMTQAITYPE_PHYSICALADAPTERDEVICEIDS: i32 = 31;
const KMTQAITYPE_NODEPERFDATA: i32 = 61;
const KMTQAITYPE_ADAPTERPERFDATA: i32 = 62;
const KMTQAITYPE_DRIVER_DESCRIPTION: i32 = 65;

/// `D3DKMT_ADAPTERTYPE` bits.
const RENDER_SUPPORTED: u32 = 1 << 0;
const SOFTWARE_DEVICE: u32 = 1 << 2;
const HYBRID_INTEGRATED: u32 = 1 << 5;
const INDIRECT_DISPLAY_DEVICE: u32 = 1 << 6;

const ENGINE_USAGE: &str = r"\GPU Engine(*)\Utilization Percentage";
/// Engines are numbered per adapter; more than this many would be unusual.
const MAX_ENGINES: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Luid {
    low: u32,
    high: i32,
}

/// `D3DKMT_ADAPTERINFO`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AdapterInfo {
    handle: u32,
    luid: Luid,
    sources: u32,
    precise_present_regions: i32,
}

/// `D3DKMT_ENUMADAPTERS2`.
#[repr(C)]
struct EnumAdapters2 {
    count: u32,
    adapters: *mut AdapterInfo,
}

/// `D3DKMT_QUERYADAPTERINFO`.
#[repr(C)]
struct QueryAdapterInfo {
    adapter: u32,
    kind: i32,
    data: *mut c_void,
    size: u32,
}

/// `D3DKMT_QUERY_DEVICE_IDS`.
#[repr(C)]
#[derive(Default)]
struct DeviceIds {
    physical_adapter: u32,
    vendor: u32,
    device: u32,
    sub_vendor: u32,
    sub_system: u32,
    revision: u32,
    bus_type: u32,
}

/// `D3DKMT_ADAPTER_PERFDATA`.
#[repr(C)]
#[derive(Default)]
struct AdapterPerf {
    physical_adapter: u32,
    memory_frequency: u64,
    max_memory_frequency: u64,
    max_memory_frequency_oc: u64,
    memory_bandwidth: u64,
    pcie_bandwidth: u64,
    fan_rpm: u32,
    /// Tenths of a percent of the board power limit.
    power: u32,
    /// Tenths of a degree Celsius.
    temperature: u32,
    power_state_override: u8,
}

/// `D3DKMT_NODE_PERFDATA`, without the `Reserved` member of newer headers: the kernel rejects
/// the longer struct.
#[repr(C)]
#[derive(Default)]
struct NodePerf {
    node: u32,
    physical_adapter: u32,
    /// Documented in Hz, but see [`engine_mhz`].
    frequency: u64,
    max_frequency: u64,
    max_frequency_oc: u64,
    voltage: u32,
    voltage_max: u32,
    voltage_max_oc: u32,
    max_transition_latency: u64,
}

type EnumAdapters2Fn = unsafe extern "system" fn(*mut EnumAdapters2) -> i32;
type QueryAdapterInfoFn = unsafe extern "system" fn(*const QueryAdapterInfo) -> i32;
type CloseAdapterFn = unsafe extern "system" fn(*const u32) -> i32;

/// The D3DKMT functions of `gdi32.dll`.
struct Kmt {
    query_info: QueryAdapterInfoFn,
    close: CloseAdapterFn,
    // Last: the functions above live in this library.
    _gdi32: Library,
}

impl Kmt {
    fn load() -> io::Result<(Self, EnumAdapters2Fn)> {
        let gdi32 = Library::system("gdi32.dll")?;
        // SAFETY: the types match the documented signatures of these exports.
        let symbols = unsafe {
            (|| {
                Some((
                    gdi32.symbol::<EnumAdapters2Fn>(c"D3DKMTEnumAdapters2")?,
                    gdi32.symbol::<QueryAdapterInfoFn>(c"D3DKMTQueryAdapterInfo")?,
                    gdi32.symbol::<CloseAdapterFn>(c"D3DKMTCloseAdapter")?,
                ))
            })()
        };
        let (enumerate, query_info, close) =
            symbols.ok_or_else(|| io::Error::other("gdi32.dll has no D3DKMT exports"))?;
        Ok((
            Self {
                query_info,
                close,
                _gdi32: gdi32,
            },
            enumerate,
        ))
    }

    /// Fills `data` with the adapter information of type `kind`.
    fn query<T>(&self, adapter: u32, kind: i32, data: &mut T) -> io::Result<()> {
        let request = QueryAdapterInfo {
            adapter,
            kind,
            data: (data as *mut T).cast(),
            size: size_of::<T>() as u32,
        };
        ntstatus(
            unsafe { (self.query_info)(&request) },
            "D3DKMTQueryAdapterInfo",
        )
    }

    fn close(&self, adapter: u32) {
        unsafe { (self.close)(&adapter) };
    }
}

/// A hardware adapter found by D3DKMT.
struct Candidate {
    info: AdapterInfo,
    vendor: Vendor,
    integrated: bool,
    name: String,
}

/// Reads the main GPU.
pub struct Gpu {
    name: String,
    backend: Backend,
}

enum Backend {
    Nvml(Nvml),
    Kmt {
        kmt: Kmt,
        adapter: u32,
        usage: Option<EngineUsage>,
    },
}

impl Gpu {
    pub fn open() -> io::Result<Self> {
        let (kmt, enumerate) = Kmt::load()?;
        let best = pick_adapter(&kmt, enumerate)?;
        if best.vendor == Vendor::Nvidia
            && let Ok(nvml) = Nvml::open()
        {
            kmt.close(best.info.handle);
            return Ok(Self {
                name: nvml.name().unwrap_or(best.name),
                backend: Backend::Nvml(nvml),
            });
        }
        // Without PDH the temperature and clock still work.
        let usage = EngineUsage::open(best.info.luid).ok();
        let mut gpu = Self {
            name: best.name,
            backend: Backend::Kmt {
                kmt,
                adapter: best.info.handle,
                usage,
            },
        };
        gpu.read()?;
        Ok(gpu)
    }

    pub fn read(&mut self) -> io::Result<Values> {
        match &mut self.backend {
            Backend::Nvml(nvml) => Ok(nvml.read()),
            Backend::Kmt {
                kmt,
                adapter,
                usage,
            } => {
                let mut perf = AdapterPerf::default();
                kmt.query(*adapter, KMTQAITYPE_ADAPTERPERFDATA, &mut perf)?;
                // Node 0 is the 3D engine.
                let mut node = NodePerf::default();
                let freq = kmt
                    .query(*adapter, KMTQAITYPE_NODEPERFDATA, &mut node)
                    .ok()
                    .and_then(|()| engine_mhz(node.frequency, node.max_frequency));
                Ok(Values {
                    temp: deci_celsius(perf.temperature),
                    usage: usage.as_mut().and_then(|u| u.read().ok()),
                    power: None,
                    freq,
                })
            }
        }
    }

    /// The adapter name, e.g. "Radeon RX550/550 Series".
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Human-readable description of the interface in use.
    pub fn source(&self) -> &'static str {
        match self.backend {
            Backend::Nvml(_) => "NVML",
            Backend::Kmt { .. } => "D3DKMT performance data and PDH GPU Engine",
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Backend::Kmt { kmt, adapter, .. } = &self.backend {
            kmt.close(*adapter);
        }
    }
}

/// Opens every adapter, keeps the best hardware one and closes the others.
fn pick_adapter(kmt: &Kmt, enumerate: EnumAdapters2Fn) -> io::Result<Candidate> {
    let mut request = EnumAdapters2 {
        count: 0,
        adapters: std::ptr::null_mut(),
    };
    ntstatus(unsafe { enumerate(&mut request) }, "D3DKMTEnumAdapters2")?;
    let mut adapters = vec![AdapterInfo::default(); request.count as usize];
    request.adapters = adapters.as_mut_ptr();
    ntstatus(unsafe { enumerate(&mut request) }, "D3DKMTEnumAdapters2")?;
    adapters.truncate(request.count as usize);

    let mut best: Option<Candidate> = None;
    for info in adapters {
        let candidate = describe(kmt, info);
        let better = |c: &Candidate| {
            best.as_ref()
                .is_none_or(|b| (c.integrated, c.vendor.rank()) < (b.integrated, b.vendor.rank()))
        };
        match candidate {
            Some(c) if better(&c) => {
                if let Some(old) = best.replace(c) {
                    kmt.close(old.info.handle);
                }
            }
            _ => kmt.close(info.handle),
        }
    }
    best.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no GPU found"))
}

/// The adapter as a candidate, or `None` for software and virtual display adapters.
fn describe(kmt: &Kmt, info: AdapterInfo) -> Option<Candidate> {
    let mut flags = 0u32;
    kmt.query(info.handle, KMTQAITYPE_ADAPTERTYPE, &mut flags)
        .ok()?;
    if !is_hardware_gpu(flags) {
        return None;
    }
    let mut ids = DeviceIds::default();
    kmt.query(info.handle, KMTQAITYPE_PHYSICALADAPTERDEVICEIDS, &mut ids)
        .ok()?;
    let mut description = Box::new([0u16; 4096]);
    let name = match kmt.query(
        info.handle,
        KMTQAITYPE_DRIVER_DESCRIPTION,
        &mut *description,
    ) {
        Ok(()) => from_wide(&*description),
        Err(_) => "GPU".to_owned(),
    };
    Some(Candidate {
        info,
        vendor: Vendor::from_pci_id(ids.vendor),
        integrated: flags & HYBRID_INTEGRATED != 0,
        name,
    })
}

/// A render-capable adapter that is neither software nor an indirect (virtual) display.
fn is_hardware_gpu(adapter_type: u32) -> bool {
    adapter_type & RENDER_SUPPORTED != 0
        && adapter_type & (SOFTWARE_DEVICE | INDIRECT_DISPLAY_DEVICE) == 0
}

/// `None` for 0, which drivers without a sensor report.
fn deci_celsius(value: u32) -> Option<f32> {
    (value != 0).then(|| value as f32 / 10.0)
}

/// The engine clock in MHz. The documented unit is Hz, but drivers differ (AMD reports 10 kHz
/// steps), so the unit is the one that puts the maximum clock in a plausible GPU range. No two
/// candidate units can both fit: they are 10× apart and the range spans 8×.
fn engine_mhz(frequency: u64, max_frequency: u64) -> Option<f32> {
    const PLAUSIBLE_MAX_MHZ: std::ops::RangeInclusive<f64> = 500.0..=4000.0;
    // Hz, kHz, 10 kHz, MHz.
    const DIVISORS_TO_MHZ: [f64; 4] = [1e6, 1e3, 1e2, 1.0];
    if frequency == 0 {
        return None;
    }
    let divisor = DIVISORS_TO_MHZ
        .into_iter()
        .find(|d| PLAUSIBLE_MAX_MHZ.contains(&(max_frequency as f64 / d)))?;
    Some((frequency as f64 / divisor) as f32)
}

fn ntstatus(status: i32, what: &str) -> io::Result<()> {
    if status >= 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{what} failed (NTSTATUS 0x{:08X})",
            status as u32
        )))
    }
}

/// Usage of one adapter from the per-process `GPU Engine` counters.
struct EngineUsage {
    query: Query,
    counter: Counter,
    /// `luid_0x........_0x........`, as in the instance names.
    luid: String,
}

impl EngineUsage {
    fn open(luid: Luid) -> io::Result<Self> {
        let mut query = Query::open()?;
        let counter = query.add(ENGINE_USAGE)?;
        query.collect()?;
        Ok(Self {
            query,
            counter,
            luid: format!("luid_0x{:08x}_0x{:08x}", luid.high as u32, luid.low),
        })
    }

    fn read(&mut self) -> io::Result<f32> {
        self.query.collect()?;
        let mut engines = [0.0f64; MAX_ENGINES];
        let luid = &self.luid;
        self.query.values(self.counter, 0, |name, value| {
            let mut ascii = [0u8; 128];
            if let Some(name) = lowercase_ascii(name, &mut ascii)
                && let Some(engine) = engine_index(name, luid)
                && let Some(sum) = engines.get_mut(engine)
            {
                *sum += value;
            }
        })?;
        Ok(busiest(&engines))
    }
}

/// The instance name as lowercase ASCII in `buf`, without allocating; `None` if it does not fit.
fn lowercase_ascii<'a>(name: &[u16], buf: &'a mut [u8]) -> Option<&'a str> {
    let out = buf.get_mut(..name.len())?;
    for (o, &c) in out.iter_mut().zip(name) {
        *o = u8::try_from(c)
            .ok()
            .filter(u8::is_ascii)?
            .to_ascii_lowercase();
    }
    std::str::from_utf8(out).ok()
}

/// The engine number of an instance such as
/// `pid_1234_luid_0x00000000_0x0000bee6_phys_0_eng_3_engtype_3d`, if it belongs to `luid`.
fn engine_index(instance: &str, luid: &str) -> Option<usize> {
    if !instance.contains(luid) {
        return None;
    }
    let after = &instance[instance.find("_eng_")? + 5..];
    let digits = after.find('_').map_or(after, |end| &after[..end]);
    digits.parse().ok()
}

/// Task Manager's GPU usage: the busiest engine, summed over every process.
fn busiest(engines: &[f64]) -> f32 {
    engines.iter().copied().fold(0.0, f64::max).min(100.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_types() {
        assert!(is_hardware_gpu(RENDER_SUPPORTED | 0b10));
        assert!(!is_hardware_gpu(RENDER_SUPPORTED | SOFTWARE_DEVICE));
        assert!(!is_hardware_gpu(RENDER_SUPPORTED | INDIRECT_DISPLAY_DEVICE));
        assert!(!is_hardware_gpu(0));
    }

    #[test]
    fn perf_units() {
        assert_eq!(deci_celsius(456), Some(45.6));
        assert_eq!(deci_celsius(0), None);
    }

    #[test]
    fn engine_clock_units() {
        // Radeon RX 550 (Adrenalin 24): 10 kHz steps, idle at 214 MHz, 1350 MHz maximum.
        assert_eq!(engine_mhz(21_400, 135_000), Some(214.0));
        // As documented: Hz.
        assert_eq!(engine_mhz(2_520_000_000, 2_820_000_000), Some(2520.0));
        assert_eq!(engine_mhz(1_500_000, 2_100_000), Some(1500.0));
        assert_eq!(engine_mhz(0, 135_000), None);
        // A maximum that fits no unit: unknown.
        assert_eq!(engine_mhz(5, 7), None);
    }

    #[test]
    fn engine_instances() {
        let luid = "luid_0x00000000_0x0000bee6";
        let name = "pid_10280_luid_0x00000000_0x0000bee6_phys_0_eng_3_engtype_videodecode";
        assert_eq!(engine_index(name, luid), Some(3));
        let other = "pid_10280_luid_0x00000000_0x0000aaaa_phys_0_eng_0_engtype_3d";
        assert_eq!(engine_index(other, luid), None);
    }

    #[test]
    fn instance_names_are_lowercased_without_allocating() {
        let wide: Vec<u16> = "pid_1_LUID_0x0000BEE6".encode_utf16().collect();
        let mut buf = [0u8; 128];
        assert_eq!(
            lowercase_ascii(&wide, &mut buf),
            Some("pid_1_luid_0x0000bee6")
        );
        let long = vec![b'a' as u16; 200];
        assert_eq!(lowercase_ascii(&long, &mut buf), None);
    }

    #[test]
    fn usage_is_the_busiest_engine() {
        assert_eq!(busiest(&[12.0, 70.5, 3.0]), 70.5);
        assert_eq!(busiest(&[60.0, 80.0]), 80.0);
        assert_eq!(busiest(&[120.0]), 100.0);
        assert_eq!(busiest(&[]), 0.0);
    }

    #[test]
    #[ignore = "needs a GPU; run with --ignored --nocapture"]
    fn live_reading() {
        let mut gpu = Gpu::open().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let r = gpu.read().unwrap();
        println!("{} via {}: {r:?}", gpu.name(), gpu.source());
        assert!(r.temp.is_some() || r.usage.is_some());
    }
}
