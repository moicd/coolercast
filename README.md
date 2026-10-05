# CoolerCast

A tiny, native driver for the displays of DeepCool CPU coolers, for Windows and Linux. It shows the
CPU temperature or usage on the cooler without the official DeepCool app.

The official app is an Electron application that, on a typical setup, keeps **16 processes and about
1 GB of RAM** busy to send one 64-byte USB packet per second (see
[the analysis](docs/official-app-analysis.md)). CoolerCast does the same job with a single small
service written in Rust, plus an optional tray icon and settings window on Windows.

| | Official DeepCool app 1.2.14 | CoolerCast 0.2 |
|---|---|---|
| Processes | 16 (plus 3 services and 4 drivers) | 1 service + optional tray icon |
| RAM | ~1 GB working set | 0.8 MB private (service), 2.1 MB (app with its window open) |
| CPU while idle | several Chromium processes awake | 0 % (wakes once per second) |
| Install size | ~1.2 GB | 0.9 MB |
| Platforms | Windows x64 | Windows and Linux, x64 and ARM64 |

<sub>Measured on a Windows 11 test system. RAM for CoolerCast is the private working set shown by
Task Manager.</sub>

> [!NOTE]
> This is an independent community project. It is not affiliated with, endorsed by or supported by
> DeepCool. Use it at your own risk.

## Download

Get the latest version from the [releases page](https://github.com/moicd/coolercast/releases/latest):

| System | File |
|---|---|
| Windows 10/11, Intel or AMD (most PCs) | `coolercast-<version>-windows-x64.msi` |
| Windows 11 on ARM | `coolercast-<version>-windows-arm64.msi` |
| Windows, without installing | `coolercast-<version>-windows-x64.zip` / `-arm64.zip` |
| Linux, Intel or AMD | `coolercast-<version>-linux-x86_64.tar.gz` |
| Linux on ARM64 | `coolercast-<version>-linux-aarch64.tar.gz` |

`SHA256SUMS.txt` lists the checksums of every file.

## Features

- CPU temperature (°C or °F), CPU usage, both alternating, or a fixed number of your choice.
- Temperature alarm (the display blinks above a threshold, 90 °C by default).
- Runs as a service: starts with the PC, no window needed.
- Windows: optional tray icon that shows the temperature, and a settings window with a live preview
  of the cooler display and the last two minutes of temperature and usage (translucent Acrylic glass
  on Windows 11).
- Reconnects automatically after unplugging the cooler or resuming from sleep.
- No runtime, no telemetry, no network access. The Linux build is a single static binary.

## Supported hardware

| Cooler | USB ID | Status |
|---|---|---|
| AK400 DIGITAL / DIGITAL SE | `3633:0001` | Tested (`A400-DIGITAL`) |
| AK620 DIGITAL / DIGITAL SE | `3633:0002` | Same protocol, untested |
| AK500 DIGITAL | `3633:0003` | Same protocol, untested |
| AK500S DIGITAL / DIGITAL SE | `3633:0004` | Same protocol, untested |

Other DeepCool products use different protocols. Contributions are welcome; see
[docs/protocol-ak-series.md](docs/protocol-ak-series.md) for how the AK series works.

| Platform | CPU temperature from | Status |
|---|---|---|
| Windows x64, Intel | PawnIO, package thermal MSR | Tested |
| Windows x64, AMD Ryzen | PawnIO, Tctl over SMN | Experimental |
| Windows ARM64 | not available yet (usage only) | Builds, untested |
| Linux x86_64 / ARM64 | kernel hwmon (`k10temp`, `coretemp`, `cpu_thermal`) | Builds and installs in CI, untested on a cooler |

## Install on Windows

1. Close the official DeepCool app and disable it at startup (or uninstall it). Both apps write to
   the same device and would fight over the display.
2. Install [PawnIO](https://pawnio.eu/) for the CPU temperature: `winget install namazso.PawnIO`.
   Without it, CoolerCast shows CPU usage only. PawnIO is a signed, open-source driver also used by
   LibreHardwareMonitor and FanControl; if the official DeepCool app was installed, you probably
   have it already.
3. Run the `.msi` installer. It installs CoolerCast to `C:\Program Files\CoolerCast`, starts the
   service and adds **CoolerCast** to the Start menu.
4. Open CoolerCast from the Start menu to change the settings, and enable **Start with Windows** if
   you want the tray icon at sign-in.

Uninstall it from **Settings → Apps**. Your settings stay in `C:\ProgramData\CoolerCast`.

The binaries are not code-signed yet (see [CODE_SIGNING.md](CODE_SIGNING.md)), so SmartScreen may
warn the first time: choose **More info → Run anyway**.

<details>
<summary>Portable zip instead of the installer</summary>

Extract the zip to a permanent folder, for example `C:\Program Files\CoolerCast`, and in a terminal
opened **as administrator** in that folder run `coolercast install`. Remove it with
`coolercast uninstall` and delete the folder.
</details>

## Install on Linux

Requires systemd.

```bash
tar xzf coolercast-*-linux-*.tar.gz
cd coolercast-*-linux-*/
sudo ./install.sh
```

The script installs `/usr/local/bin/coolercast`, a systemd service and a udev rule that lets your
user talk to the cooler, then starts the service. Remove everything with
`sudo ./install.sh --uninstall`.

The temperature comes from the kernel's hwmon drivers: `k10temp` (AMD) and `coretemp` (Intel) are
loaded by default on most distributions. Check what CoolerCast found with `coolercast list`.

## Usage

```text
coolercast list              Show detected coolers and sensor readings
coolercast test              Play a test pattern on the connected coolers
coolercast probe <what>      Step through undocumented display values (see below)
coolercast run               Drive the coolers in the foreground (Ctrl+C to stop)
coolercast status            Show what the running service is doing
coolercast set key=value     Change a setting, e.g. `set mode=auto unit=fahrenheit`
```

On Windows, `coolercast install`, `uninstall`, `start` and `stop` manage the service (as
administrator). On Linux, use `systemctl` (`sudo systemctl restart coolercast`) and
`journalctl -u coolercast` for the log.

### Settings

Settings live in `C:\ProgramData\CoolerCast\config.toml` on Windows and
`/etc/coolercast/config.toml` on Linux. Change them with `coolercast set`, from the Windows app, or
by editing the file (as administrator or root); the service picks up changes immediately.

| Key | Values | Default |
|---|---|---|
| `mode` | `temperature`, `usage`, `auto` (alternates), `custom` | `temperature` |
| `unit` | `celsius`, `fahrenheit` | `celsius` |
| `alarm` | `true`, `false` | `true` |
| `alarm_threshold` | 40–110 (°C) | `90` |
| `interval_ms` | 250–10000 | `1000` |
| `auto_interval_s` | 1–3600 | `5` |
| `custom_value` | 0–999, shown in `custom` mode | `0` |
| `custom_symbol` | `celsius`, `fahrenheit`, `percent` | `celsius` |
| `custom_bar` | 1–10 | `1` |

In `custom` mode the temperature alarm still blinks the display when the CPU gets hot.

### Exploring the display

The AK series display has fixed segments: three digits, the °C/°F/% symbols and a 10-step bar.
Only digits 0–9 and the values in [the protocol notes](docs/protocol-ak-series.md) are known.
`coolercast probe` sends other values one at a time so you can see whether the firmware hides
letters or other symbols. Stop the service first, then run, for example:

```bash
coolercast probe digit          # digit values 10-31 in every position
coolercast probe mode 0 255     # every mode byte, with "123" on the digits
coolercast probe bar            # bar values 0-20
coolercast probe raw 19 5 1 2 3 0
```

Press Enter for the next value and type what you see to note it; the notes are printed as a
table at the end. If the display stops responding, unplug the cooler's USB cable. Findings are
welcome as issues.

The Windows service log is at `C:\ProgramData\CoolerCast\coolercast.log`.

## How it works

```text
coolercast (service)
 ├─ CPU usage ........ GetSystemTimes (Windows), /proc/stat (Linux)
 ├─ CPU temperature .. PawnIO + IntelMSR / AMDFamily17 module (Windows), hwmon (Linux)
 ├─ Cooler display ... HID output report once per second (WriteFile / /dev/hidraw)
 └─ control channel .. \\.\pipe\coolercast (Windows), /run/coolercast/coolercast.sock (Linux)
                        ◄── coolercast-app.exe / coolercast status|set
```

On Windows the service runs as LocalSystem only because reading the CPU temperature needs it; the
app runs as a normal user and talks to the service over the control channel.

## Building

Requires the Rust toolchain. On Windows, also the Visual Studio Build Tools (MSVC); add the ARM64
build tools to cross-compile for Windows on ARM.

```bash
cargo build --release
cargo test
```

The binaries end up in `target/release`. On Windows, copy the `third_party/pawnio-modules/*.bin`
files to a `modules` folder next to them. Release builds for every platform, the MSI installers
(WiX, `packaging/windows`) and the Linux tarballs (`packaging/linux`) are produced by
[the CI workflow](.github/workflows/ci.yml).

## Credits

- [PawnIO](https://github.com/namazso/PawnIO) and its
  [modules](https://github.com/namazso/PawnIO.Modules) by namazso (LGPL-2.1), shipped unmodified in
  `third_party/pawnio-modules`.
- The AK series protocol is also documented by the community in
  [deepcool-digital-linux](https://github.com/Nortank12/deepcool-digital-linux).

## License

[MIT](LICENSE). The PawnIO modules keep their own license (LGPL-2.1).
