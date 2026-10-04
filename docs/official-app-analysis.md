# What the official DeepCool app runs

Snapshot of the official DeepCool software (v1.2.14) on a Windows 11 machine with an
AK400 DIGITAL, an Intel Core i5-9400F and an AMD Radeon RX 550, measured right after start-up with
the main window closed to the tray.

## Processes

| Component | Processes | Working set |
|---|---|---|
| `DeepCool.exe` (Electron: browser, GPU, network, audio and 2 renderer processes) | 6 | ~604 MB |
| `msedgewebview2.exe` (WebView2 host used by the app) | 6 | ~366 MB |
| `SensorWorker.exe` + `JZFSSensorBridgeServer.exe` (sensor service, HWiNFO SDK) | 2 | ~72 MB |
| `JZFSDisplayService.exe`, `FanControlService.exe` (services) | 2 | ~22 MB |
| **Total** | **16** | **~1 GB** |

It also installs three Windows services, a virtual display driver (`JZFSDisplayDriver`), two
HWiNFO kernel drivers and the PawnIO driver.

## Install footprint

- `DeepCool.exe`: 162 MB (Electron runtime).
- `resources/app.asar`: 293 MB (Vue UI, Element Plus, Ant Design, ECharts, Lottie, ...).
- Unpacked native modules: `ffmpeg.exe` (148 MB), two more FFmpeg Node modules (~95 MB), OpenCV
  (~75 MB), ImageMagick (~45 MB), a screen capture module (34 MB) and one native module per product
  family.
- Sample videos and images for the LCD-equipped products (~250 MB).

Most of this exists to support the products with full LCD screens (video playback, screen mirroring,
image cropping). None of it is needed to drive the two-segment display of the AK series.

## How the AK series is driven

1. The Electron main process enumerates HID devices with `node-hid` and matches DeepCool's vendor
   ID `0x3633`.
2. Sensor values come from `SensorWorker.exe`, which embeds the HWiNFO SDK, through a named pipe
   (`\\.\pipe\jzfs_sensor_bridge_v2`).
3. About once per second the main process writes a 64-byte HID output report with the mode, the bar
   level and three digits (see [protocol-ak-series.md](protocol-ak-series.md)).

## What deepcool-native does instead

| | Official app | deepcool-native |
|---|---|---|
| Runtime | Electron + Chromium + WebView2 | Native Win32 (Rust), no runtime |
| Sensors | HWiNFO SDK in a separate worker process | `GetSystemTimes` for usage, one MSR / SMN read through PawnIO for temperature |
| HID | `node-hid` | `WriteFile` on the HID handle |
| Processes | 16 | 1 service + 1 optional tray icon |
| RAM | ~1 GB working set | 0.75 MB private working set (service), 1.1 MB (tray) |
| Install size | ~1.2 GB | 0.7 MB (two executables and two PawnIO modules) |
