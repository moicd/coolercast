# Other DeepCool display protocols

Protocols of the displays supported besides the AK series ([protocol-ak-series.md](protocol-ak-series.md)).
None of them has been tested on real hardware by CoolerCast yet: they are implemented from the
community mapping tables listed under [Sources](#sources). Reports from owners are welcome.

| USB ID | Model | Family |
|---|---|---|
| `3633:0005` | CH560 DIGITAL (case) | [CH](#ch-series) |
| `3633:0006` | LS520 SE DIGITAL, LS720 SE DIGITAL | [LS](#ls-series) |
| `3633:0007` | MORPHEUS (case) | [CH](#ch-series) |
| `3633:0008` | AG400 DIGITAL, AG620 DIGITAL | [AG](#ag-series) |
| `3633:000A` | LD240, LD360 | [Framed](#framed-reports-ld-pro-and-lq), LD |
| `3633:000D` | LQ240, LQ360 | Framed, LQ |
| `3633:000F`, `3633:001F` | ASSASSIN IV VC VISION | Framed, LQ |
| `3633:0010` | AK400 DIGITAL PRO | Framed, AK400 PRO |
| `3633:0011` | AK500 DIGITAL PRO | Framed, AK500/AK620 PRO |
| `3633:0012` | AK620 DIGITAL PRO | Framed, AK500/AK620 PRO |
| `3633:0013` | CH170 DIGITAL (case) | [CH 2nd generation](#ch-series-2nd-generation) |
| `3633:0015` | CH360 DIGITAL (case) | [CH](#ch-series) |
| `3633:0016` | CH270 DIGITAL (case) | CH 2nd generation |
| `3633:001B` | CH690 DIGITAL (case) | CH 2nd generation |
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

## CH series

Two AK-like sections: the CPU above, the GPU below. Start-up report and symbols as in the AK
series; there is no alarm byte.

| Byte | Value | Meaning |
|---|---|---|
| D1 | `170` / `19` / `35` / `76` | CPU section: init, °C, °F, % |
| D2 | `1`–`10` | CPU bar |
| D3–D5 | `0`–`9` | CPU digits |
| D6 | `19` / `35` / `76` | GPU section: °C, °F, % |
| D7 | `1`–`10` | GPU bar |
| D8–D10 | `0`–`9` | GPU digits |

Each bar follows the usage of its component (`1` below 15 %, otherwise `round(usage / 10)`).
Both sections show the same kind of value: the usage in the `usage` mode, the temperature
otherwise (the `auto` mode alternates them). A `custom` value goes to the CPU section.

## CH series 2nd generation

A framed report like the [framed reports](#framed-reports-ld-pro-and-lq), with a fixed length
(the checksum of D1–D39 is always D40, followed by `22` in D41):

| Bytes | Field |
|---|---|
| D1–D5 | Header `104 1 6 35 1` |
| D6 | Page: `1` display test, `2` CPU + clock, `3` CPU + fan, `4` GPU + clock, `5` PSU + fan, `6` PSU + power |
| D7–D8 | CPU power, W (`u16`) |
| D9 | Temperature unit for every field: `0` °C, `1` °F |
| D10–D13 | CPU temperature (`f32`) |
| D14 | CPU usage, % |
| D15–D16 | CPU clock, MHz (`u16`) |
| D17–D18 | CPU fan, RPM (`u16`) |
| D19–D20 | GPU power, W (`u16`) |
| D21–D24 | GPU temperature (`f32`) |
| D25 | GPU usage, % |
| D26–D27 | GPU clock, MHz (`u16`) |
| D28–D38 | PSU power, temperature, usage, power and fan |
| D39 | Unused |

CoolerCast sends page `2` or `4` depending on the `source` setting (`auto` alternates them) and
fills both the CPU and the GPU fields. The fan and PSU fields are sent as 0: there is no
vendor-neutral way to read them.

## CH510 MESH DIGITAL

A case display with its own vendor ID. The report is ASCII text after the report ID:

```text
HLXDATA(<usage>,<temperature>,0,0,<C|F>)\r\n
```

The usage (0–100) drives the bar and the temperature (whole degrees, up to 999) the three digits,
for example `HLXDATA(30,36,0,0,C)\r\n`. The two `0` fields are not used for the CPU. With
`source = "gpu"` CoolerCast sends the GPU usage and temperature instead.

## Sensors

The framed displays and the LS power mode need the CPU power, the AK500/AK620 PRO, LQ and CH
2nd generation displays the frequency, and the CH series (or any display with `source` set to
`gpu` or `auto`) the GPU. CoolerCast only reads them while such a display is connected.

| Value | Windows | Linux |
|---|---|---|
| CPU power | RAPL package energy through PawnIO: Intel MSR `0x611` (unit `0x606`), AMD MSR `0xC001029B` (unit `0xC0010299`). Energy unit = 1/2^ESU J, ESU = bits 12:8 of the unit register; the counter is 32 bits wide and wraps. | `/sys/class/powercap/intel-rapl:0/energy_uj` (µJ), wrapping at `max_energy_range_uj`. Readable by root only. |
| CPU frequency | PDH: `\Processor Information(_Total)\Processor Frequency` × `% Processor Performance` / 100 | Average of `/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq` (kHz) |
| GPU, NVIDIA | NVML (`nvml.dll` from the driver): temperature, utilization, power, graphics clock | Proprietary driver: `nvidia-smi --query-gpu=... -lms 1000`, one long-lived process. `nouveau`: hwmon temperature and power |
| GPU, AMD | D3DKMT adapter and node performance data (temperature, engine clock) and the `\GPU Engine(*)\Utilization Percentage` counters (usage of the busiest engine, as Task Manager shows it). No power: D3DKMT reports it as a share of the board limit | `amdgpu`: `gpu_busy_percent`, hwmon `temp1_input`, `power1_average` or `power1_input`, `freq1_input` |
| GPU, Intel | As AMD | `i915`/`xe`: GT clock (`gt_act_freq_mhz` or `tile0/gt0/freq0/act_freq`), hwmon temperature and energy on discrete cards. No usage |

With several GPUs, CoolerCast uses a discrete NVIDIA or AMD card before an Intel one (and, on
Linux, the one with the most video memory). Virtual display adapters are skipped.

D3DKMT documents the node clock in Hz, but the AMD driver reports it in 10 kHz steps (an RX 550
reads `21400` at 214 MHz idle, with a `135000` maximum). CoolerCast picks the unit (Hz, kHz,
10 kHz or MHz) that puts the maximum clock between 500 and 4000 MHz. The node data struct must
also be passed without the `Reserved` member of newer headers, or the call fails with
`STATUS_INVALID_PARAMETER`.

Power is the energy used between two refreshes divided by the time between them, so the first
refresh after start-up has no power value (sent as 0).

## Sources

- The community-maintained device list and mapping tables in
  [Nortank12/deepcool-digital-linux](https://github.com/Nortank12/deepcool-digital-linux/tree/main/device-list):
  `ag-series.md`, `ls-series.md`, `ld-series.md`, `ak400-pro.md`, `ak620-pro.md`, `lq-series.md`,
  `ch-series.md`, `ch-series-gen2.md` and `ch510.md`. Byte order (big-endian), the LS and CH bar
  rules and the CH 2nd generation page values are from the same project. Only these protocol
  facts are used; no code was copied.
- RAPL registers: Intel SDM vol. 4 (`MSR_RAPL_POWER_UNIT`, `MSR_PKG_ENERGY_STATUS`) and the AMD
  family 17h PPR (`MSR C001_0299`, `MSR C001_029B`); allowed by the PawnIO `IntelMSR` and
  `AMDFamily17` modules.
- GPU: Microsoft's [`D3DKMT_ADAPTER_PERFDATA`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/d3dkmthk/ns-d3dkmthk-_d3dkmt_adapter_perfdata)
  and [`D3DKMT_NODE_PERFDATA`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/d3dkmthk/ns-d3dkmthk-_d3dkmt_node_perfdata)
  reference, NVIDIA's NVML API reference, and the kernel documentation of the
  [amdgpu](https://docs.kernel.org/gpu/amdgpu/thermal.html) sysfs files.
