# PawnIO modules

Prebuilt [PawnIO](https://pawnio.eu/) modules used to read CPU temperature sensors.

| File | Used for | SHA-256 |
|---|---|---|
| `IntelMSR.bin` | Intel package/core thermal MSRs | `d6ed85d65ab17a22f813ef98207d6d537155ee2ded5976a21cb48413c9b92e5f` |
| `AMDFamily17.bin` | AMD Zen (family 17h–1Ah) SMN thermal register | `dae74615761b78bdf064dfb3e136252ddcc6fc727d88f14738d0e5800d427a91` |

- Source: [namazso/PawnIO.Modules](https://github.com/namazso/PawnIO.Modules), release `0.2.11` (`release_0_2_11.zip`).
- License: GNU LGPL-2.1-or-later, see [`COPYING`](COPYING).

The modules are shipped as separate files next to the executable and loaded at runtime, so they can be
replaced with any compatible build (for example, one you compiled yourself from the sources above).
They are interpreted by the PawnIO driver, which must be installed separately
(`winget install namazso.PawnIO`).
