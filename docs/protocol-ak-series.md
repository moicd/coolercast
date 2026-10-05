# AK series display protocol

Applies to the DeepCool coolers with the small two-segment display (numeric value + 10-step bar):

| PID | Model |
|---|---|
| `0x0001` | AK400 DIGITAL, AK400 DIGITAL SE |
| `0x0002` | AK620 DIGITAL, AK620 DIGITAL SE |
| `0x0003` | AK500 DIGITAL |
| `0x0004` | AK500S DIGITAL, AK500S DIGITAL SE |

All of them enumerate as a USB HID device with vendor ID `0x3633` (DeepCool) and a vendor-defined
top-level collection (usage page `0xFF00`, usage `0x0001`). The SE variants report a product string
without the `K` (for example `A400-DIGITAL`) and declare no report ID.

## Transport

The host sends one 64-byte output report per update over the interrupt OUT endpoint
(`WriteFile` on the HID handle). The device never answers. The official app refreshes the display
about once per second; the display keeps the last value it received.

## Packet layout

| Byte | Value | Meaning |
|---|---|---|
| D0 | `16` | Report ID |
| D1 | `170` | Init: plays the status bar animation |
|    | `19` | Temperature mode, °C symbol |
|    | `35` | Temperature mode, °F symbol |
|    | `76` | Usage mode, % symbol |
| D2 | `1`–`10` | Status bar level |
| D3 | `0`–`9` | Hundreds digit |
| D4 | `0`–`9` | Tens digit |
| D5 | `0`–`9` | Units digit |
| D6 | `0`/`1` | Alarm (blinking warning) |
| D7–D63 | `0` | Unused |

Bar level: `1` when the value is below 15, otherwise `round(value / 10)`, clamped to `1..=10`.
CoolerCast computes the bar from the Celsius temperature in both temperature units, so the bar
means the same thing regardless of the unit shown.

The official app uses an alarm threshold of 90 °C (194 °F).

### Examples

```
init            10 aa 00 00 00 00 00 ...
41 °C           10 13 04 00 04 01 00 ...
106 °F (41 °C)  10 23 04 01 00 06 00 ...
7 % usage       10 4c 01 00 00 07 00 ...
92 °C + alarm   10 13 09 00 09 02 01 ...
```

## SE variants

The *SE* variants use the same packet but their report descriptor has no report ID, so every byte
is shifted one position to the left on the wire (64 bytes starting with the mode byte). On Windows
the HID stack hides that difference: a device without report IDs takes a 65-byte buffer whose first
byte is `0`, followed by the same payload. CoolerCast reads the report ID from the device's
report descriptor (`HidP_GetValueCaps`) and uses it as D0, which covers both variants with a single
code path.

For example, an `A400-DIGITAL` (PID `0x0001`) reports report ID `0` and an `OutputReportByteLength`
of 65.

## Unknown values

Nothing is known about values outside the ranges above: digit values above 9, other mode bytes,
bar level 0 or above 10. CoolerCast never sends them, except through `coolercast probe`, which
exists to find out what the firmware does with them (for example blank digits, letters or other
symbols). Results will be added here.

CoolerCast's `custom` mode only uses the documented values: a number from 0 to 999, one of the
three symbols (modes `19`, `35` and `76`) and a bar level from 1 to 10.

## Display modes of the official app

The official app stores a `mode` value per device: `1` temperature, `2` usage and `3` "dynamic"
(alternates between both). The temperature unit is a global setting and the "warning" switch
(`rgbOptical`) enables the alarm byte.

## Sources

- Observed behaviour and the renderer code of the official DeepCool app (v1.2.14).
- The community-maintained mapping table in
  [Nortank12/deepcool-digital-linux](https://github.com/Nortank12/deepcool-digital-linux/blob/main/device-list/tables/ak-series.md).
