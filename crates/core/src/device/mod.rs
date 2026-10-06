//! Supported coolers and the protocol each one speaks. See `docs/protocol-ak-series.md` and
//! `docs/protocol-other-series.md`.

pub mod ag;
pub mod ak;
pub mod ch;
pub mod ch510;
pub mod framed;
pub mod ls;

use std::io;

use crate::config::{Source, Symbol, Unit};
use crate::hid::{self, DeviceInfo, HidDevice};
use crate::sensors::Values;

/// USB vendor ID used by DeepCool.
pub const DEEPCOOL_VID: u16 = 0x3633;

/// USB vendor ID of the CH510 MESH DIGITAL case display.
pub const CH510_VID: u16 = 0x34D3;

/// Every vendor ID with a supported model.
const VENDORS: [u16; 2] = [DEEPCOOL_VID, CH510_VID];

/// Protocol family of a cooler display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Three digits plus a 10-step bar, one value at a time.
    AkSeries,
    /// Two digits, no bar, Celsius only.
    AgSeries,
    /// Like the AK series, with a power (W) mode instead of usage; the bar follows CPU usage.
    LsSeries,
    /// Framed report with power, temperature and usage, after two init reports.
    LdSeries,
    /// Framed report like the LD series with another header and no init reports.
    Ak400Pro,
    /// Framed report that adds the CPU frequency (AK500 and AK620 DIGITAL PRO).
    Ak620Pro,
    /// Framed report with frequency (LQ series, ASSASSIN IV VC VISION, AK G2 / AK700 NYX).
    LqSeries,
    /// ASCII report with usage and temperature.
    Ch510,
    /// Two AK-like sections, CPU above and GPU below (CH560, CH360 DIGITAL and MORPHEUS cases).
    ChSeries,
    /// Framed report with CPU and GPU values; shows one of them at a time (CH170, CH270 and
    /// CH690 DIGITAL cases).
    ChGen2,
}

impl Family {
    /// Whether the display needs the CPU power, so it is only measured when useful.
    pub fn uses_power(self) -> bool {
        matches!(
            self,
            Family::LsSeries
                | Family::LdSeries
                | Family::Ak400Pro
                | Family::Ak620Pro
                | Family::LqSeries
                | Family::ChGen2
        )
    }

    /// Whether the display needs the CPU frequency.
    pub fn uses_frequency(self) -> bool {
        matches!(self, Family::Ak620Pro | Family::LqSeries | Family::ChGen2)
    }

    /// Whether the display needs the GPU sensors with this `source` setting. The CH series
    /// always shows the GPU; the CPU-only displays (LD, LQ, DIGITAL PRO) never do.
    pub fn uses_gpu(self, source: Source) -> bool {
        match self {
            Family::ChSeries => true,
            Family::AkSeries
            | Family::AgSeries
            | Family::LsSeries
            | Family::Ch510
            | Family::ChGen2 => source != Source::Cpu,
            Family::LdSeries | Family::Ak400Pro | Family::Ak620Pro | Family::LqSeries => false,
        }
    }

    /// Whether the display shows one value with three digits and a bar, like the settings
    /// window preview.
    pub fn is_ak_like(self) -> bool {
        matches!(self, Family::AkSeries | Family::LsSeries)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Model {
    pub vendor_id: u16,
    pub product_id: u16,
    pub name: &'static str,
    pub family: Family,
    /// Not tested on real hardware yet.
    pub experimental: bool,
}

const fn deepcool(product_id: u16, name: &'static str, family: Family) -> Model {
    Model {
        vendor_id: DEEPCOOL_VID,
        product_id,
        name,
        family,
        experimental: !matches!(family, Family::AkSeries),
    }
}

pub const MODELS: &[Model] = &[
    deepcool(0x0001, "AK400 DIGITAL", Family::AkSeries),
    deepcool(0x0002, "AK620 DIGITAL", Family::AkSeries),
    deepcool(0x0003, "AK500 DIGITAL", Family::AkSeries),
    deepcool(0x0004, "AK500S DIGITAL", Family::AkSeries),
    deepcool(0x0005, "CH560 DIGITAL", Family::ChSeries),
    deepcool(0x0006, "LS520/LS720 SE DIGITAL", Family::LsSeries),
    deepcool(0x0007, "MORPHEUS", Family::ChSeries),
    deepcool(0x0008, "AG400/AG620 DIGITAL", Family::AgSeries),
    deepcool(0x000A, "LD240/LD360", Family::LdSeries),
    deepcool(0x000D, "LQ240/LQ360", Family::LqSeries),
    deepcool(0x000F, "ASSASSIN IV VC VISION", Family::LqSeries),
    deepcool(0x0010, "AK400 DIGITAL PRO", Family::Ak400Pro),
    deepcool(0x0011, "AK500 DIGITAL PRO", Family::Ak620Pro),
    deepcool(0x0012, "AK620 DIGITAL PRO", Family::Ak620Pro),
    deepcool(0x0013, "CH170 DIGITAL", Family::ChGen2),
    deepcool(0x0015, "CH360 DIGITAL", Family::ChSeries),
    deepcool(0x0016, "CH270 DIGITAL", Family::ChGen2),
    deepcool(0x001B, "CH690 DIGITAL", Family::ChGen2),
    deepcool(0x001F, "ASSASSIN IV VC VISION", Family::LqSeries),
    deepcool(0x0029, "AK620 G2 DIGITAL NYX", Family::LqSeries),
    deepcool(0x002A, "AK700 DIGITAL NYX", Family::LqSeries),
    deepcool(0x002B, "AK400 G2 DIGITAL NYX", Family::LqSeries),
    deepcool(0x002C, "AK500 G2 DIGITAL NYX", Family::LqSeries),
    Model {
        vendor_id: CH510_VID,
        product_id: 0x1100,
        name: "CH510 MESH DIGITAL",
        family: Family::Ch510,
        experimental: true,
    },
];

pub fn model(vendor_id: u16, product_id: u16) -> Option<&'static Model> {
    MODELS
        .iter()
        .find(|m| m.vendor_id == vendor_id && m.product_id == product_id)
}

/// The model with this display name, as listed in the service status.
pub fn model_named(name: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.name == name)
}

/// A DeepCool HID collection that can receive output reports.
#[derive(Clone, Debug)]
pub struct Detected {
    pub info: DeviceInfo,
    /// `None` for DeepCool devices this version does not support yet.
    pub model: Option<&'static Model>,
}

/// Lists every DeepCool collection with an output report, supported or not. Other devices of
/// the CH510 vendor are left out: that ID is not DeepCool's own.
pub fn detect() -> io::Result<Vec<Detected>> {
    Ok(hid::enumerate(&VENDORS)?
        .into_iter()
        .filter(|info| info.output_report_len > 0)
        .map(|info| Detected {
            model: model(info.vendor_id, info.product_id),
            info,
        })
        .filter(|d| d.model.is_some() || d.info.vendor_id == DEEPCOOL_VID)
        .collect())
}

/// What a single-value display should show on its next update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reading {
    Temperature {
        celsius: f32,
        unit: Unit,
    },
    Usage {
        percent: f32,
    },
    /// CPU package power, in watts (LS series only).
    Power {
        watts: f32,
    },
    /// A value chosen by the user: shown as is, with its own bar level.
    Custom {
        value: u16,
        symbol: Symbol,
        bar: u8,
    },
}

/// The part of the PC a value belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Component {
    #[default]
    Cpu,
    Gpu,
}

/// Sensor values of one refresh.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Readings {
    /// CPU temperature in °C.
    pub cpu_temp: Option<f32>,
    /// CPU usage in percent.
    pub cpu_usage: f32,
    /// CPU package power in watts.
    pub cpu_power: Option<f32>,
    /// Average CPU frequency in MHz.
    pub cpu_freq: Option<f32>,
    /// GPU values, all `None` while no display needs them.
    pub gpu: Values,
}

impl Readings {
    /// The values of one component.
    pub fn of(&self, component: Component) -> Values {
        match component {
            Component::Cpu => Values {
                temp: self.cpu_temp,
                usage: Some(self.cpu_usage),
                power: self.cpu_power,
                freq: self.cpu_freq,
            },
            Component::Gpu => self.gpu,
        }
    }
}

/// Everything a display may need for one refresh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Update {
    /// What single-value displays show, chosen from the configured mode.
    pub reading: Reading,
    /// The component [`Update::reading`] belongs to, and the page of displays that show one
    /// component at a time.
    pub component: Component,
    /// All sensor values, for displays that show several at once.
    pub readings: Readings,
    pub unit: Unit,
    pub alarm: bool,
}

impl Update {
    /// The reading a single-value display of `family` can show: [`Update::reading`] when the
    /// display has a symbol for it, otherwise the temperature (or, without a temperature
    /// sensor, the usage on AK and AG displays and the power on LS displays).
    pub fn reading_for(&self, family: Family) -> Reading {
        let supported = match self.reading {
            Reading::Temperature { .. } | Reading::Custom { .. } => true,
            Reading::Usage { .. } => family != Family::LsSeries,
            Reading::Power { .. } => family == Family::LsSeries,
        };
        if supported {
            return self.reading;
        }
        let values = self.readings.of(self.component);
        match (values.temp, values.power, family) {
            (Some(celsius), ..) => Reading::Temperature {
                celsius,
                unit: self.unit,
            },
            (None, Some(watts), Family::LsSeries) => Reading::Power { watts },
            // Shows 0 °C: the LS display has no usage symbol.
            (None, None, Family::LsSeries) => Reading::Temperature {
                celsius: 0.0,
                unit: self.unit,
            },
            (None, ..) => Reading::Usage {
                percent: values.usage.unwrap_or(0.0),
            },
        }
    }
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

    pub fn family(&self) -> Family {
        self.model.family
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// Sends the start-up sequence (AK and LS displays play their bar animation).
    pub fn init(&mut self) -> io::Result<()> {
        match self.model.family {
            Family::AkSeries | Family::LsSeries | Family::ChSeries => {
                self.hid.write(&ak::init_packet(self.report_id))
            }
            Family::LdSeries => {
                for packet in framed::ld_init_packets(self.report_id) {
                    self.hid.write(&packet)?;
                }
                Ok(())
            }
            Family::AgSeries
            | Family::Ak400Pro
            | Family::Ak620Pro
            | Family::LqSeries
            | Family::Ch510
            | Family::ChGen2 => Ok(()),
        }
    }

    pub fn show(&mut self, update: &Update) -> io::Result<()> {
        let id = self.report_id;
        let family = self.model.family;
        let telemetry = || framed::Telemetry::new(&update.readings, update.unit);
        let shown = update.readings.of(update.component);
        let packet = match family {
            Family::AkSeries => ak::packet(id, update.reading_for(family), update.alarm),
            Family::AgSeries => ag::packet(id, update.reading_for(family), update.alarm),
            Family::LsSeries => ls::packet(
                id,
                update.reading_for(family),
                shown.usage.unwrap_or(update.readings.cpu_usage),
                update.alarm,
            ),
            Family::LdSeries => framed::packet(id, &framed::LD, &telemetry()),
            Family::Ak400Pro => framed::packet(id, &framed::AK400_PRO, &telemetry()),
            Family::Ak620Pro => framed::packet(id, &framed::AK620_PRO, &telemetry()),
            Family::LqSeries => framed::packet(id, &framed::LQ, &telemetry()),
            Family::Ch510 => ch510::packet(id, &shown, update.unit),
            Family::ChSeries => ch::packet(id, update),
            Family::ChGen2 => framed::ch_gen2_packet(
                id,
                update.component,
                &telemetry(),
                &framed::Telemetry::from_values(&update.readings.gpu, update.unit),
            ),
        };
        self.hid.write(&packet)
    }

    /// Sends the payload bytes as they are, without any validation. Only for exploring
    /// undocumented values of AK-like displays (`coolercast probe`).
    pub fn send_raw(&mut self, payload: [u8; ak::PAYLOAD_LEN]) -> io::Result<()> {
        if !self.model.family.is_ak_like() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("{} does not use the AK report layout", self.model.name),
            ));
        }
        self.hid.write(&ak::raw_packet(self.report_id, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(reading: Reading, readings: Readings) -> Update {
        Update {
            reading,
            component: Component::Cpu,
            readings,
            unit: Unit::Celsius,
            alarm: false,
        }
    }

    const GPU: Values = Values {
        temp: Some(70.0),
        usage: Some(90.0),
        power: Some(180.0),
        freq: Some(2500.0),
    };

    const SENSORS: Readings = Readings {
        cpu_temp: Some(50.0),
        cpu_usage: 20.0,
        cpu_power: Some(65.0),
        cpu_freq: Some(4200.0),
        gpu: GPU,
    };

    const TEMP: Reading = Reading::Temperature {
        celsius: 50.0,
        unit: Unit::Celsius,
    };

    #[test]
    fn product_ids_are_unique_per_vendor() {
        for (i, a) in MODELS.iter().enumerate() {
            for b in &MODELS[i + 1..] {
                assert!(
                    (a.vendor_id, a.product_id) != (b.vendor_id, b.product_id),
                    "{} and {} share an ID",
                    a.name,
                    b.name
                );
            }
        }
    }

    #[test]
    fn models_are_found_by_vendor_and_product() {
        assert_eq!(model(DEEPCOOL_VID, 1).unwrap().family, Family::AkSeries);
        assert_eq!(model(DEEPCOOL_VID, 0x2C).unwrap().family, Family::LqSeries);
        assert_eq!(model(CH510_VID, 0x1100).unwrap().family, Family::Ch510);
        assert_eq!(model(CH510_VID, 1), None);
        assert_eq!(model(DEEPCOOL_VID, 0x1100), None);
        assert!(!model(DEEPCOOL_VID, 1).unwrap().experimental);
        assert!(model(DEEPCOOL_VID, 0x10).unwrap().experimental);
    }

    #[test]
    fn supported_readings_pass_through() {
        let usage = Reading::Usage { percent: 20.0 };
        let power = Reading::Power { watts: 65.0 };
        assert_eq!(update(usage, SENSORS).reading_for(Family::AkSeries), usage);
        assert_eq!(update(power, SENSORS).reading_for(Family::LsSeries), power);
        assert_eq!(update(TEMP, SENSORS).reading_for(Family::AgSeries), TEMP);
    }

    #[test]
    fn unsupported_readings_fall_back_to_temperature() {
        let power = Reading::Power { watts: 65.0 };
        let usage = Reading::Usage { percent: 20.0 };
        assert_eq!(update(power, SENSORS).reading_for(Family::AkSeries), TEMP);
        assert_eq!(update(power, SENSORS).reading_for(Family::AgSeries), TEMP);
        assert_eq!(update(usage, SENSORS).reading_for(Family::LsSeries), TEMP);
    }

    #[test]
    fn fallback_without_a_temperature_sensor() {
        let no_temp = Readings {
            cpu_temp: None,
            ..SENSORS
        };
        let power = Reading::Power { watts: 65.0 };
        let usage = Reading::Usage { percent: 20.0 };
        assert_eq!(update(power, no_temp).reading_for(Family::AkSeries), usage);
        assert_eq!(update(usage, no_temp).reading_for(Family::LsSeries), power);
        let nothing = Readings {
            cpu_power: None,
            ..no_temp
        };
        assert_eq!(
            update(usage, nothing).reading_for(Family::LsSeries),
            Reading::Temperature {
                celsius: 0.0,
                unit: Unit::Celsius
            }
        );
    }

    #[test]
    fn gpu_component_falls_back_to_gpu_values() {
        let gpu_update = |reading| Update {
            component: Component::Gpu,
            ..update(reading, SENSORS)
        };
        let power = Reading::Power { watts: 180.0 };
        assert_eq!(
            gpu_update(power).reading_for(Family::AkSeries),
            Reading::Temperature {
                celsius: 70.0,
                unit: Unit::Celsius
            }
        );
        assert_eq!(gpu_update(power).reading_for(Family::LsSeries), power);
    }

    #[test]
    fn gpu_sensors_only_when_needed() {
        assert!(Family::ChSeries.uses_gpu(Source::Cpu));
        assert!(!Family::AkSeries.uses_gpu(Source::Cpu));
        assert!(Family::AkSeries.uses_gpu(Source::Auto));
        assert!(Family::ChGen2.uses_gpu(Source::Gpu));
        assert!(!Family::LqSeries.uses_gpu(Source::Gpu));
    }
}
