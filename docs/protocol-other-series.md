# Other DeepCool display protocols

Protocols of the displays supported besides the AK series ([protocol-ak-series.md](protocol-ak-series.md)).
None of them has been tested on real hardware by CoolerCast yet: they are implemented from the
community mapping tables listed under [Sources](#sources). Reports from owners are welcome.

| USB ID | Model | Family |
|---|---|---|
| `3633:0006` | LS520 SE DIGITAL, LS720 SE DIGITAL | [LS](#ls-series) |
| `3633:0008` | AG400 DIGITAL, AG620 DIGITAL | [AG](#ag-series) |
| `3633:000A` | LD240, LD360 | [Framed](#framed-reports-ld-pro-and-lq), LD |
| `3633:000D` | LQ240, LQ360 | Framed, LQ |
| `3633:000F`, `3633:001F` | ASSASSIN IV VC VISION | Framed, LQ |
| `3633:0010` | AK400 DIGITAL PRO | Framed, AK400 PRO |
| `3633:0011` | AK500 DIGITAL PRO | Framed, AK500/AK620 PRO |
| `3633:0012` | AK620 DIGITAL PRO | Framed, AK500/AK620 PRO |
| `3633:0029` | AK620 G2 DIGITAL NYX | Framed, LQ |
| `3633:002A` | AK700 DIGITAL NYX | Framed, LQ |
| `3633:002B` | AK400 G2 DIGITAL NYX | Framed, LQ |
| `3633:002C` | AK500 G2 DIGITAL NYX | Framed, LQ |
| `34D3:1100` | CH510 MESH DIGITAL | [CH510](#ch510-mesh-digital) |

The transport is the same as for the AK series: one output report per update, padded with zeros
to the report length the device declares. D0 is the report ID (`16`, or `0` for devices without
report IDs); the tables below list the bytes after it.

## AG series

Two digits and no bar. There is no °F symbol, so the temperature is always sent in °C.

| Byte | Value | Meaning |
|---|---|---|
| D1 | `19` | Temperature, °C |
|    | `76` | Usage, % |
| D2 | `0` | Unused |
| D3 | `0`–`9` | Tens digit |
| D4 | `0`–`9` | Units digit |
| D5 | `0`/`1` | Alarm |

Values of 100 and above are shown as 99. The power mode falls back to the temperature.

## LS series

Same layout and start-up report (`D1 = 170`) as the AK series, but:

- Mode `76` shows the CPU power in watts instead of the usage. CoolerCast sends it in the `power`
  mode; the `usage` mode falls back to the temperature, since there is no % symbol.
- The bar (D2) always follows the CPU usage: `1` below 15 %, otherwise `round(usage / 10)`.

## Framed reports: LD, PRO and LQ

These displays show the CPU power, temperature and usage (and, on some models, the frequency) at
once, so the configured mode does not apply. Each report is a fixed header, the values and a
checksum, closed by `22`. Multi-byte values are big-endian.

| Field | Size | Meaning |
|---|---|---|
| Header | 6–7 bytes | Fixed per model, see below |
| Power | `u16` | CPU package power in W |
| Unit | 1 byte | `0` °C, `1` °F |
| Temperature | `f32` | CPU temperature in that unit (CoolerCast sends whole degrees) |
| Usage | 1 byte | CPU usage, 0–100 % |
| Frequency | `u16` | CPU frequency in MHz (AK500/AK620 PRO and LQ only) |
| Checksum | 1 byte | Sum of every byte from D1 up to the checksum, modulo 256 |
| Terminator | 1 byte | `22` |

| Model | Header (D1…) | Frequency |
|---|---|---|
| LD series | `104 1 1 11 1 2 5` | no |
| AK400 DIGITAL PRO | `104 1 2 11 1 2 5` | no |
| AK500 / AK620 DIGITAL PRO | `104 1 4 13 1 2 8` | yes |
| LQ family | `104 1 8 12 1 2` | yes |

Example, LD series at 500 W, 42 °C and 37 %:
`16 | 104 1 1 11 1 2 5 | 1 244 | 0 | 66 40 0 0 | 37 | 1 | 22`.

The LD series also expects two reports after connecting, in the same framing:
`104 1 1 2 3 1 | 112 | 22` and `104 1 1 2 2 0 | 110 | 22`. The last header byte of the second one
turns leading zeros off (`1` would turn them on). The other framed models need no start-up report.

## CH510 MESH DIGITAL

A case display with its own vendor ID. The report is ASCII text after the report ID:

```text
HLXDATA(<usage>,<temperature>,0,0,<C|F>)\r\n
```

The usage (0–100) drives the bar and the temperature (whole degrees, up to 999) the three digits,
for example `HLXDATA(30,36,0,0,C)\r\n`. The two `0` fields are not used for the CPU.

## Sensors

The framed displays and the LS power mode need the CPU power, and the AK500/AK620 PRO and LQ
displays the frequency. CoolerCast only reads them while such a display is connected.

| Value | Windows | Linux |
|---|---|---|
| CPU power | RAPL package energy through PawnIO: Intel MSR `0x611` (unit `0x606`), AMD MSR `0xC001029B` (unit `0xC0010299`). Energy unit = 1/2^ESU J, ESU = bits 12:8 of the unit register; the counter is 32 bits wide and wraps. | `/sys/class/powercap/intel-rapl:0/energy_uj` (µJ), wrapping at `max_energy_range_uj`. Readable by root only. |
| CPU frequency | PDH: `\Processor Information(_Total)\Processor Frequency` × `% Processor Performance` / 100 | Average of `/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq` (kHz) |

Power is the energy used between two refreshes divided by the time between them, so the first
refresh after start-up has no power value (sent as 0).

## Sources

- The community-maintained device list and mapping tables in
  [Nortank12/deepcool-digital-linux](https://github.com/Nortank12/deepcool-digital-linux/tree/main/device-list):
  `ag-series.md`, `ls-series.md`, `ld-series.md`, `ak400-pro.md`, `ak620-pro.md`, `lq-series.md`
  and `ch510.md`. Byte order (big-endian) and the LS bar rule are from the same project. Only these
  protocol facts are used; no code was copied.
- RAPL registers: Intel SDM vol. 4 (`MSR_RAPL_POWER_UNIT`, `MSR_PKG_ENERGY_STATUS`) and the AMD
  family 17h PPR (`MSR C001_0299`, `MSR C001_029B`); allowed by the PawnIO `IntelMSR` and
  `AMDFamily17` modules.
