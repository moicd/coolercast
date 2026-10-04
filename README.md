# deepcool-native

A tiny, native Windows driver for the displays of DeepCool CPU coolers. It shows the CPU temperature
or usage on the cooler without the official DeepCool app.

The official app is an Electron application that, on a typical setup, keeps **16 processes and about
1 GB of RAM** busy to send one 64-byte USB packet per second (see
[the analysis](docs/official-app-analysis.md)). deepcool-native does the same job with a single small
Windows service written in Rust, plus an optional tray icon.

> [!NOTE]
> This is an independent community project. It is not affiliated with, endorsed by or supported by
> DeepCool. Use it at your own risk.

## Features

- CPU temperature (°C or °F), CPU usage, or both alternating.
- Temperature alarm (the display blinks above a threshold, 90 °C by default).
- Runs as a Windows service: starts with the PC, no window, no tray icon required.
- Optional tray icon that shows the temperature and lets you switch modes.
- Reconnects automatically after unplugging the cooler or resuming from sleep.
- No runtime, no installer, no telemetry, no network access.

## Supported coolers

| Cooler | USB ID | Status |
|---|---|---|
| AK400 DIGITAL / DIGITAL SE | `3633:0001` | Tested |
| AK620 DIGITAL / DIGITAL SE | `3633:0002` | Same protocol, untested |
| AK500 DIGITAL | `3633:0003` | Same protocol, untested |
| AK500S DIGITAL / DIGITAL SE | `3633:0004` | Same protocol, untested |

Other DeepCool products use different protocols. Contributions are welcome; see
[docs/protocol-ak-series.md](docs/protocol-ak-series.md) for how the AK series works.

| CPU | Temperature sensor | Status |
|---|---|---|
| Intel Core (2nd gen and newer) | Package thermal MSR | Tested |
| AMD Ryzen (Zen 1–5) | Tctl over SMN | Experimental |

## Requirements

- Windows 10 or 11, x64.
- [PawnIO](https://pawnio.eu/) for the CPU temperature: `winget install namazso.PawnIO`.
  Without it, deepcool-native shows CPU usage only. PawnIO is a signed, open-source driver also used
  by LibreHardwareMonitor and FanControl. If the official DeepCool app is installed, PawnIO is
  probably installed already.
- Close the official DeepCool app and disable it at startup (or uninstall it). Both apps write to
  the same device and would fight over the display.

## Install

1. Download the latest release and extract it to a permanent folder, for example
   `C:\Program Files\deepcool-native`.
2. Open a terminal **as administrator** in that folder and run:

   ```bash
   deepcool-native install
   ```

   The service is installed, set to start automatically and started.
3. Optional: run `deepcool-tray.exe` and enable **Start with Windows** from its menu.

To remove it, run `deepcool-native uninstall` as administrator and delete the folder.

## Usage

```text
deepcool-native list              Show detected coolers and sensor readings
deepcool-native test              Play a test pattern on the connected coolers
deepcool-native run               Drive the coolers in the foreground (Ctrl+C to stop)
deepcool-native status            Show what the running service is doing
deepcool-native set key=value     Change a setting, e.g. `set mode=auto unit=fahrenheit`
deepcool-native install           Install and start the Windows service (administrator)
deepcool-native uninstall         Stop and remove the Windows service (administrator)
deepcool-native start | stop      Start or stop the installed service (administrator)
```

### Settings

Settings live in `C:\ProgramData\deepcool-native\config.toml`. Change them with
`deepcool-native set`, from the tray menu, or by editing the file (as administrator); the service
picks up changes immediately.

| Key | Values | Default |
|---|---|---|
| `mode` | `temperature`, `usage`, `auto` (alternates) | `temperature` |
| `unit` | `celsius`, `fahrenheit` | `celsius` |
| `alarm` | `true`, `false` | `true` |
| `alarm_threshold` | 40–110 (°C) | `90` |
| `interval_ms` | 250–10000 | `1000` |
| `auto_interval_s` | 1–3600 | `5` |

The service log is at `C:\ProgramData\deepcool-native\deepcool-native.log`.

## How it works

```text
deepcool-native.exe (service, LocalSystem)
 ├─ CPU usage ........ GetSystemTimes
 ├─ CPU temperature .. PawnIO driver + IntelMSR / AMDFamily17 module
 ├─ Cooler display ... HID output report (WriteFile), once per second
 └─ \\.\pipe\deepcool-native ◄── deepcool-tray.exe / deepcool-native status|set
```

The service needs administrator rights only because reading the CPU temperature does. The tray app
runs as a normal user and talks to the service over a local named pipe.

## Building

Requires the Rust toolchain (MSVC target) and the Visual Studio Build Tools.

```bash
cargo build --release
cargo test
```

The binaries end up in `target/release`. Copy the `third_party/pawnio-modules/*.bin` files to a
`modules` folder next to them.

## Credits

- [PawnIO](https://github.com/namazso/PawnIO) and its
  [modules](https://github.com/namazso/PawnIO.Modules) by namazso (LGPL-2.1), shipped unmodified in
  `third_party/pawnio-modules`.
- The AK series protocol is also documented by the community in
  [deepcool-digital-linux](https://github.com/Nortank12/deepcool-digital-linux).

## License

[MIT](LICENSE). The PawnIO modules keep their own license (LGPL-2.1).
