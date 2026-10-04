//! Supported coolers and the protocol each one speaks.

pub mod ak;

use std::io;

use crate::config::Unit;
use crate::hid::{self, DeviceInfo, HidDevice};

/// USB vendor ID used by DeepCool.
pub const DEEPCOOL_VID: u16 = 0x3633;

/// Protocol family of a cooler display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Two-segment display: three digits plus a 10-step bar. See `docs/protocol-ak-series.md`.
    AkSeries,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Model {
    pub product_id: u16,
    pub name: &'static str,
    pub family: Family,
}

pub const MODELS: &[Model] = &[
    Model {
        product_id: 0x0001,
        name: "AK400 DIGITAL",
        family: Family::AkSeries,
    },
    Model {
        product_id: 0x0002,
        name: "AK620 DIGITAL",
        family: Family::AkSeries,
    },
    Model {
        product_id: 0x0003,
        name: "AK500 DIGITAL",
        family: Family::AkSeries,
    },
    Model {
        product_id: 0x0004,
        name: "AK500S DIGITAL",
        family: Family::AkSeries,
    },
];

pub fn model(product_id: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.product_id == product_id)
}

/// A DeepCool HID collection that can receive output reports.
#[derive(Clone, Debug)]
pub struct Detected {
    pub info: DeviceInfo,
    /// `None` for DeepCool devices this version does not support yet.
    pub model: Option<&'static Model>,
}

/// Lists every DeepCool collection with an output report, supported or not.
pub fn detect() -> io::Result<Vec<Detected>> {
    Ok(hid::enumerate(DEEPCOOL_VID)?
        .into_iter()
        .filter(|info| info.output_report_len > 0)
        .map(|info| Detected {
            model: model(info.product_id),
            info,
        })
        .collect())
}

/// What a display should show on its next update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reading {
    Temperature { celsius: f32, unit: Unit },
    Usage { percent: f32 },
}

/// An opened, supported cooler.
#[derive(Debug)]
pub struct Cooler {
    model: &'static Model,
    report_id: u8,
    serial: String,
    hid: HidDevice,
}

impl Cooler {
    pub fn open(detected: &Detected) -> io::Result<Self> {
        let model = detected
            .model
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unsupported device"))?;
        Ok(Self {
            model,
            report_id: detected.info.report_id,
            serial: detected.info.serial.clone(),
            hid: HidDevice::open(&detected.info)?,
        })
    }

    pub fn name(&self) -> &'static str {
        self.model.name
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// Sends the start-up sequence (the AK series plays its bar animation).
    pub fn init(&mut self) -> io::Result<()> {
        match self.model.family {
            Family::AkSeries => self.hid.write(&ak::init_packet(self.report_id)),
        }
    }

    pub fn show(&mut self, reading: Reading, alarm: bool) -> io::Result<()> {
        match self.model.family {
            Family::AkSeries => self.hid.write(&ak::packet(self.report_id, reading, alarm)),
        }
    }
}
