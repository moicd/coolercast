# CoolerCast

A tiny, native driver for the displays of DeepCool CPU coolers, for Windows and Linux. It shows the
CPU temperature or usage on the cooler without the official DeepCool app.

The official app is an Electron application that, on a typical setup, keeps **16 processes and about
1 GB of RAM** busy to send one 64-byte USB packet per second (see
[the analysis](docs/official-app-analysis.md)). CoolerCast does the same job with a single small
service written in Rust, plus an optional tray icon and settings window on Windows.

| | Official DeepCool app 1.2.14 | CoolerCast 0.5 |
|---|---|---|
| Processes | 16 (plus 3 services and 4 drivers) | 1 service + optional tray icon |
| RAM | ~1 GB working set | 0.8 MB private (service), 2.1 MB (app with its window open) |
| CPU while idle | several Chromium processes awake | 0 % (wakes once per second) |
| Install size | ~1.2 GB | 1.1 MB |
| Platforms | Windows | Windows and Linux |

<sub>Measured on a Windows 11 test system. RAM for CoolerCast is the private working set shown by
Task Manager.</sub>

> [!NOTE]
> This is an independent community project. It is not affiliated with, endorsed by or supported by
> DeepCool. Use it at your own risk.

## Download

| System | Download |
|---|---|
| Windows 10 / 11 | [**coolercast-windows-x64.msi**](https://github.com/moicd/coolercast/releases/latest/download/coolercast-windows-x64.msi) (installer, recommended) |
| Windows, portable | [coolercast-windows-x64.zip](https://github.com/moicd/coolercast/releases/latest/download/coolercast-windows-x64.zip) |
| Linux (x86-64, systemd) | [coolercast-linux-x86_64.tar.gz](https://github.com/moicd/coolercast/releases/latest/download/coolercast-linux-x86_64.tar.gz) |

These links always point to the latest version. Older versions, release notes and
`SHA256SUMS.txt` are on the [releases page](https://github.com/moicd/coolercast/releases).

## Features

- CPU temperature (°C or °F), CPU usage, both alternating, or a fixed number of your choice.
- CPU power and frequency for the displays that show them (LS, LD, LQ and DIGITAL PRO models),
  read only while such a display is connected.
- GPU temperature, usage, power and clock (NVIDIA, AMD and Intel) for the CH series cases, and on
  any display with `source = "gpu"`, `"auto"` (alternates CPU and GPU) or `"smart"` (the GPU while
  it is busy, for example while gaming, and the CPU otherwise).
- Temperature on the digits and usage on the bar at the same time (`bar = "usage"`).
- Turns the display off while the PC is locked or its screen is off (Windows), or every night.
- Temperature alarm (the display blinks above a threshold, 90 °C by default).
- Runs as a service: starts with the PC, no window needed.
- Windows: optional tray icon that shows the temperature, and a settings window in the style of the
  Windows 11 settings, with a live preview of the cooler display and the last two minutes of
  temperature and usage (translucent Acrylic glass on Windows 11). It follows the Windows text
  size, high contrast and transparency effects settings.
- Reconnects automatically after unplugging the cooler or resuming from sleep.
- Windows efficiency mode (EcoQoS): the service and the tray icon ask Windows for efficiency cores
  and clock speeds; the settings window runs normally while it is open.
- No runtime and no telemetry. The only network access is the **Check for updates** button, which
  asks GitHub for the latest version when you click it. The Linux build is a single static binary.

## Supported hardware

| Cooler | USB ID | Status |
|---|---|---|
| AK400 DIGITAL / DIGITAL SE | `3633:0001` | Tested (`A400-DIGITAL`) |
| AK620 DIGITAL / DIGITAL SE | `3633:0002` | Same protocol, untested |
| AK500 DIGITAL | `3633:0003` | Same protocol, untested |
| AK500S DIGITAL / DIGITAL SE | `3633:0004` | Same protocol, untested |
| LS520 SE / LS720 SE DIGITAL | `3633:0006` | Experimental, untested |
| AG400 / AG620 DIGITAL | `3633:0008` | Experimental, untested |
| LD240 / LD360 | `3633:000A` | Experimental, untested |
| LQ240 / LQ360 | `3633:000D` | Experimental, untested |
| ASSASSIN IV VC VISION | `3633:000F`, `3633:001F` | Experimental, untested |
| AK400 DIGITAL PRO | `3633:0010` | Experimental, untested |
| AK500 DIGITAL PRO | `3633:0011` | Experimental, untested |
| AK620 DIGITAL PRO | `3633:0012` | Experimental, untested |
| AK620 G2 / AK400 G2 / AK500 G2 DIGITAL NYX | `3633:0029`, `3633:002B`, `3633:002C` | Experimental, untested |
| AK700 DIGITAL NYX | `3633:002A` | Experimental, untested |
| CH510 MESH DIGITAL (case) | `34D3:1100` | Experimental, untested |
| CH560 DIGITAL / CH360 DIGITAL / MORPHEUS (cases) | `3633:0005`, `3633:0015`, `3633:0007` | Experimental, untested |
| CH170 / CH270 / CH690 DIGITAL (cases) | `3633:0013`, `3633:0016`, `3633:001B` | Experimental, untested |

The experimental models are implemented from community protocol notes and have not been tried on
real hardware yet. If you own one, `coolercast list` and `coolercast test` (see [Usage](#usage))
tell quickly whether it works; please [report the result](https://github.com/moicd/coolercast/issues),
good or bad. The LP pixel displays are not supported yet. See
[docs/protocol-ak-series.md](docs/protocol-ak-series.md) and
[docs/protocol-other-series.md](docs/protocol-other-series.md) for how the displays work.

| Platform | CPU temperature from | Status |
|---|---|---|
| Windows, Intel | PawnIO, package thermal MSR | Tested |
| Windows, AMD Ryzen | PawnIO, Tctl over SMN | Experimental |
| Linux | kernel hwmon (`k10temp`, `coretemp`) | Installs in CI, untested on a cooler |

The CPU power comes from the RAPL energy counter (PawnIO on Windows, powercap on Linux) and the
frequency from the Windows performance counters or Linux cpufreq.

| GPU | Windows | Linux |
|---|---|---|
| NVIDIA | NVML (from the driver): all values | `nvidia-smi` (proprietary driver), or the `nouveau` sensors |
| AMD | Temperature, usage and clock from the Windows graphics kernel (as in Task Manager); no power | All values from `amdgpu` |
| Intel | As AMD | Clock, plus temperature and power on discrete cards; no usage |

The AMD path on Windows is tested with a Radeon RX 550; the rest is untested.

## Install on Windows

### 1. Before you start

- **Remove the official DeepCool app.** Both apps write to the cooler and would fight over the
  display. Uninstall it from **Settings → Apps → Installed apps → DeepCool**, or at least close it
  from its tray icon and turn it off in **Settings → Apps → Startup**.
- **Make sure the cooler's USB cable is connected** to a USB 2.0 header on the motherboard. The
  display only shows what the PC sends through that cable.

### 2. Install PawnIO (for the CPU temperature)

Windows does not let programs read the CPU temperature without a driver. CoolerCast uses
[PawnIO](https://pawnio.eu/), a small signed open-source driver that LibreHardwareMonitor and
FanControl also use. If you had the official DeepCool app, PawnIO is probably installed already.

1. Right-click the Start button and open **Terminal**.
2. Run this command and accept the prompts:

   ```bash
   winget install namazso.PawnIO
   ```

Without PawnIO, CoolerCast still works but shows the CPU usage instead of the temperature.

### 3. Install CoolerCast

1. Download [**coolercast-windows-x64.msi**](https://github.com/moicd/coolercast/releases/latest/download/coolercast-windows-x64.msi).
2. Double-click the file. CoolerCast is not code-signed yet, so Windows may show
   **"Windows protected your PC"**: click **More info → Run anyway**.
3. Accept the license, click **Install** and answer **Yes** when Windows asks for permission.

That is all: the service starts right away and starts with Windows from now on. Within a few
seconds the cooler shows the CPU temperature.

### 4. Change the settings (optional)

Open **CoolerCast** from the Start menu. From there you can choose what the display shows, the unit,
the alarm, and turn on **Start with Windows** to keep the CoolerCast icon in the taskbar's
notification area (click it to open the window again).

### Update

Click **Check for updates** in the CoolerCast window. If there is a new version, click
**Download** and run the new `.msi`: it replaces the old version and keeps your settings.

### Uninstall

**Settings → Apps → Installed apps → CoolerCast → Uninstall.** Your settings stay in
`C:\ProgramData\CoolerCast`; delete that folder too if you want to remove everything.

### If something does not work

| Problem | What to do |
|---|---|
| The window says **No cooler connected** | Check the USB cable to the motherboard header and that the official app is closed. |
| **Temperature unavailable** | Install PawnIO (step 2), then restart the PC. |
| **Service not running** | Reinstall the `.msi`, or run `coolercast start` in a terminal opened as administrator. |
| Something else | The service log is at `C:\ProgramData\CoolerCast\coolercast.log`. [Open an issue](https://github.com/moicd/coolercast/issues) and attach it. |

<details>
<summary>Portable version (zip) instead of the installer</summary>

1. Extract the zip to a permanent folder, for example `C:\Program Files\CoolerCast`.
2. Open a terminal **as administrator** in that folder and run `.\coolercast.exe install`.
3. Start `coolercast-app.exe` for the settings window.

To remove it, run `.\coolercast.exe uninstall` as administrator and delete the folder.
</details>

## Install on Linux

### 1. Before you start

- You need a distribution with **systemd** (Ubuntu, Debian, Fedora, Arch, openSUSE, Mint…) on an
  x86-64 PC, and `sudo` rights.
- **Make sure the cooler's USB cable is connected** to a USB 2.0 header on the motherboard.
- There is no settings window on Linux: everything is done with the `coolercast` command.

### 2. Install

Open a terminal and run these commands one by one:

```bash
curl -LO https://github.com/moicd/coolercast/releases/latest/download/coolercast-linux-x86_64.tar.gz
```

```bash
tar xzf coolercast-linux-x86_64.tar.gz
```

```bash
sudo coolercast/install.sh
```

The script copies the program to `/usr/local/bin/coolercast`, adds a udev rule so your user can talk
to the cooler, and installs and starts a systemd service that also starts at boot. You can delete the
downloaded files afterwards.

### 3. Check that it works

```bash
coolercast status
```

It should list your cooler and the CPU temperature. Change what the display shows with, for
example, `coolercast set mode=auto` or `coolercast set unit=fahrenheit` (see [Settings](#settings)).

### Update

Repeat the three commands of step 2: the script replaces the old version and keeps your settings.

### Uninstall

```bash
sudo /usr/local/share/coolercast/install.sh --uninstall
```

Your settings stay in `/etc/coolercast`; delete that folder too if you want to remove everything.

### If something does not work

| Problem | What to do |
|---|---|
| `coolercast status` lists no cooler | Check the USB cable, then run `coolercast list` to see whether the cooler is detected. |
| Temperature unavailable | Your CPU's temperature driver is not loaded. Run `sudo modprobe k10temp` (AMD) or `sudo modprobe coretemp` (Intel). |
| Something else | Read the log with `journalctl -u coolercast` and [open an issue](https://github.com/moicd/coolercast/issues). |

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
administrator; the `.msi` installer already does this for you). On Linux, use `systemctl` (`sudo systemctl restart coolercast`) and
`journalctl -u coolercast` for the log.

### Settings

Settings live in `C:\ProgramData\CoolerCast\config.toml` on Windows and
`/etc/coolercast/config.toml` on Linux. Change them with `coolercast set`, from the Windows app, or
by editing the file (as administrator or root); the service picks up changes immediately.

| Key | Values | Default |
|---|---|---|
| `mode` | `temperature`, `usage`, `auto` (alternates), `power` (LS series), `custom` | `temperature` |
| `source` | `cpu`, `gpu`, `auto` (alternates every `auto_interval_s`), `smart` (GPU above 50 % usage, CPU below 30 %) | `cpu` |
| `bar` | `value` (follows the number), `usage` (usage of the component shown) | `value` |
| `unit` | `celsius`, `fahrenheit` | `celsius` |
| `alarm` | `true`, `false` | `true` |
| `alarm_threshold` | 40–110 (°C) | `90` |
| `interval_ms` | 250–10000 | `1000` |
| `auto_interval_s` | 1–3600 | `5` |
| `custom_value` | 0–999, shown in `custom` mode | `0` |
| `custom_symbol` | `celsius`, `fahrenheit`, `percent` | `celsius` |
| `custom_bar` | 1–10 | `1` |
| `off_when_locked` | `true`, `false` (Windows) | `false` |
| `off_when_screen_off` | `true`, `false` (Windows) | `false` |
| `off_at_night` | `true`, `false` | `false` |
| `night_start`, `night_end` | `HH:MM`, local time | `23:00`, `07:00` |

In `custom` mode the temperature alarm still blinks the display when the CPU gets hot. A display
that cannot show the chosen value shows the temperature instead (for example, `power` on an AK
cooler or `usage` on an LS one). Displays that show several values at once (LD, LQ, DIGITAL PRO,
CH510, CH 2nd generation) ignore `mode` and the `custom_*` settings. `source` picks what the
single-value displays, the CH510 and the CH 2nd generation show; the CH series always shows the
CPU and the GPU, and the LD, LQ and DIGITAL PRO displays only the CPU. Without GPU readings the
CPU is shown.

### Exploring the display

The AK series display has fixed segments: three digits, the °C/°F/% symbols and a 10-step bar
(the LS series uses the same report layout, so `probe` works with both).
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
table at the end. If the display stops responding, unplug the cooler's USB cable. What is known
so far is in [the protocol notes](docs/protocol-ak-series.md#undocumented-digit-values): a few
letters (C, L, h) and dashes, not enough to write words. Findings are welcome as issues.

The Windows service log is at `C:\ProgramData\CoolerCast\coolercast.log`.

## How it works

```text
coolercast (service)
 ├─ CPU usage ........ GetSystemTimes (Windows), /proc/stat (Linux)
 ├─ CPU temperature .. PawnIO + IntelMSR / AMDFamily17 module (Windows), hwmon (Linux)
 ├─ GPU (on demand) .. NVML or D3DKMT + PDH (Windows), sysfs or nvidia-smi (Linux)
 ├─ Cooler display ... HID output report once per second (WriteFile / /dev/hidraw)
 └─ control channel .. \\.\pipe\coolercast (Windows), /run/coolercast/coolercast.sock (Linux)
                        ◄── coolercast-app.exe / coolercast status|set
```

On Windows the service runs as LocalSystem only because reading the CPU temperature needs it; the
app runs as a normal user and talks to the service over the control channel.

Both opt into EcoQoS (`SetProcessInformation` with `ProcessPowerThrottling`), so on CPUs with
efficiency cores Windows schedules them there, at the most efficient clock speed. The app turns it
off while its window or menu is open. Their priority stays normal on purpose: at idle priority the
display would stop updating under full load, exactly when the temperature matters.

## Building

Requires the Rust toolchain and, on Windows, the Visual Studio Build Tools (MSVC). CoolerCast
targets x86-64 only: DeepCool's digital coolers are made for desktop CPU sockets.

```bash
cargo build --release
cargo test
```

The binaries end up in `target/release`. On Windows, copy the `third_party/pawnio-modules/*.bin`
files to a `modules` folder next to them. The release files, including the MSI installer (WiX,
`packaging/windows`) and the Linux tarball (`packaging/linux`), are produced by
[the CI workflow](.github/workflows/ci.yml).

## Credits

- [PawnIO](https://github.com/namazso/PawnIO) and its
  [modules](https://github.com/namazso/PawnIO.Modules) by namazso (LGPL-2.1), shipped unmodified in
  `third_party/pawnio-modules`.
- The AK series protocol is also documented by the community in
  [deepcool-digital-linux](https://github.com/Nortank12/deepcool-digital-linux).

## License

[MIT](LICENSE). The PawnIO modules keep their own license (LGPL-2.1).
