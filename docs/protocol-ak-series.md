# AK series display protocol

Applies to the DeepCool coolers with the small two-segment display (numeric value + 10-step bar):
the bar runs across the top of the display, above the three digits and the unit symbols.

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
(`WriteFile` on the HID handle). The device never answers. The display goes blank a few seconds
after the last report, so the host has to keep sending: the official app refreshes it about once
per second, and so do CoolerCast and `coolercast probe`.

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

## Undocumented digit values

CoolerCast never sends digit values above 9, except through `coolercast probe`. A scan of every
value from 10 to 255 on an AK400 DIGITAL SE (`A400-DIGITAL`), with the same value in the three
digits, found these shapes; every other value showed nothing recognizable (segments are named
a–g, a at the top, clockwise, g in the middle):

| Value | Shape |
|---|---|
| 10 | Blank |
| 27 | L |
| 34, 46, 50 | h |
| 43 | C |
| 55 | a (top dash) |
| 57, 95 | d (bottom dash) |
| 90 | One vertical stroke |
| 93 | a and d |
| 99 | d and g |
| 124 | a, d, e and g (an E without its upper left stroke) |
| 211 | g (middle dash) |
| 228 | 3 (a, b, c, d and g) |
| 250 | h |

- The shapes look like data read past the end of the digit table rather than a font: they have no
  order, the same shape repeats, and there is no G, P or U, so words such as `CPU` or `GPU`
  cannot be written. Each digit takes one value, so segments cannot be combined either.
- The shape does not depend on the mode byte: values 43, 27 and 124 look the same with the °C,
  °F and % symbols.
- [Another user](https://github.com/raghulkrishna/deepcool-ak620-digital-linux/issues/9) reports
  that 180 draws a minus sign. It was not noticed in this scan, so firmware versions may differ.
- Values 10 to 222 were scanned with a `probe` that sent each value once; since the display blanks
  a few seconds later, a shape may have been missed there. `probe` now refreshes the display while
  it waits.
- Other mode bytes and bar levels 0 or above 10 are still unexplored.

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
