//! Runtime binding to the PDH performance counters (`pdh.dll`). The library is loaded only when a
//! sensor that needs it is opened.

use std::ffi::c_void;
use std::io;
use std::ptr;

use crate::win::{Library, wide};

pub const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
/// Lets percentages go above 100 (for example `% Processor Performance` while boosting).
pub const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
const PDH_MORE_DATA: u32 = 0x8000_07D2;

type Handle = *mut c_void;
type OpenQueryFn = unsafe extern "system" fn(*const u16, usize, *mut Handle) -> u32;
type AddCounterFn = unsafe extern "system" fn(Handle, *const u16, usize, *mut Handle) -> u32;
type CollectFn = unsafe extern "system" fn(Handle) -> u32;
type FormattedValueFn = unsafe extern "system" fn(Handle, u32, *mut u32, *mut CounterValue) -> u32;
type FormattedArrayFn =
    unsafe extern "system" fn(Handle, u32, *mut u32, *mut u32, *mut CounterItem) -> u32;
type CloseQueryFn = unsafe extern "system" fn(Handle) -> u32;

/// `PDH_FMT_COUNTERVALUE` with the double member of its union.
#[repr(C)]
struct CounterValue {
    status: u32,
    value: f64,
}

/// `PDH_FMT_COUNTERVALUE_ITEM_W`.
#[repr(C)]
struct CounterItem {
    name: *const u16,
    value: CounterValue,
}

/// A counter added to a [`Query`]; valid while the query is.
#[derive(Clone, Copy, Debug)]
pub struct Counter(Handle);

/// A PDH query with its counters, closed on drop.
pub struct Query {
    add_counter: AddCounterFn,
    collect: CollectFn,
    formatted_value: FormattedValueFn,
    formatted_array: FormattedArrayFn,
    close_query: CloseQueryFn,
    query: Handle,
    /// Reused by [`Query::values`] so wildcard counters do not allocate on every read.
    buffer: Vec<u64>,
    // Last: the functions above live in this library.
    _pdh: Library,
}

impl Query {
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
                    pdh.symbol::<FormattedArrayFn>(c"PdhGetFormattedCounterArrayW")?,
                    pdh.symbol::<CloseQueryFn>(c"PdhCloseQuery")?,
                ))
            })()
        };
        let (open_query, add_counter, collect, formatted_value, formatted_array, close_query) =
            symbols.ok_or_else(|| io::Error::other("pdh.dll is missing expected exports"))?;

        let mut query: Handle = ptr::null_mut();
        check(
            unsafe { open_query(ptr::null(), 0, &mut query) },
            "PdhOpenQuery",
        )?;
        Ok(Self {
            add_counter,
            collect,
            formatted_value,
            formatted_array,
            close_query,
            query,
            buffer: Vec::new(),
            _pdh: pdh,
        })
    }

    /// Adds a counter by its English path; `*` in the instance matches every instance.
    pub fn add(&mut self, path: &str) -> io::Result<Counter> {
        let path = wide(path);
        let mut counter: Handle = ptr::null_mut();
        check(
            unsafe { (self.add_counter)(self.query, path.as_ptr(), 0, &mut counter) },
            "PdhAddEnglishCounter",
        )?;
        Ok(Counter(counter))
    }

    /// Takes a sample of every counter. Rate counters need two samples before they have a value.
    pub fn collect(&self) -> io::Result<()> {
        check(unsafe { (self.collect)(self.query) }, "PdhCollectQueryData")
    }

    /// The value of a single-instance counter at the last sample.
    pub fn value(&self, counter: Counter, flags: u32) -> io::Result<f64> {
        let mut value = CounterValue {
            status: 0,
            value: 0.0,
        };
        let status = unsafe {
            (self.formatted_value)(
                counter.0,
                PDH_FMT_DOUBLE | flags,
                ptr::null_mut(),
                &mut value,
            )
        };
        check(status, "PdhGetFormattedCounterValue")?;
        check(value.status, "PDH counter")?;
        Ok(value.value)
    }

    /// Calls `each` with the instance name (UTF-16, no terminator) and value of every instance
    /// of a wildcard counter. Instances without a valid value yet are skipped.
    pub fn values(
        &mut self,
        counter: Counter,
        flags: u32,
        mut each: impl FnMut(&[u16], f64),
    ) -> io::Result<()> {
        let (mut bytes, mut count) = (0u32, 0u32);
        loop {
            let capacity = (self.buffer.len() * 8) as u32;
            bytes = bytes.max(capacity);
            let status = unsafe {
                (self.formatted_array)(
                    counter.0,
                    PDH_FMT_DOUBLE | flags,
                    &mut bytes,
                    &mut count,
                    self.buffer.as_mut_ptr().cast(),
                )
            };
            match status {
                PDH_MORE_DATA if bytes == 0 => return Ok(()),
                PDH_MORE_DATA if bytes as usize > self.buffer.len() * 8 => {
                    self.buffer.resize((bytes as usize).div_ceil(8), 0);
                }
                0 => break,
                other => return check(other, "PdhGetFormattedCounterArray"),
            }
        }
        let items = self.buffer.as_ptr().cast::<CounterItem>();
        for i in 0..count as usize {
            // SAFETY: PDH wrote `count` items, and the names they point to, into the buffer.
            let item = unsafe { &*items.add(i) };
            if item.value.status != 0 || item.name.is_null() {
                continue;
            }
            let name = unsafe {
                let len = (0..).take_while(|&n| *item.name.add(n) != 0).count();
                std::slice::from_raw_parts(item.name, len)
            };
            each(name, item.value.value);
        }
        Ok(())
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // Also removes the counters.
        unsafe { (self.close_query)(self.query) };
    }
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
    fn wildcard_counter_lists_instances() {
        let mut query = Query::open().unwrap();
        let counter = query
            .add(r"\Processor Information(*)\% Processor Time")
            .unwrap();
        query.collect().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        query.collect().unwrap();
        let mut names = Vec::new();
        query
            .values(counter, 0, |name, _| {
                names.push(String::from_utf16_lossy(name))
            })
            .unwrap();
        assert!(names.iter().any(|n| n == "_Total"), "{names:?}");
    }
}
