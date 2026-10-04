# AGENTS.md

Guidance for coding agents working on this repository.

## Project

deepcool-native is a lightweight, native replacement for the official DeepCool app. It drives the
display of DeepCool CPU coolers (AK series for now) with CPU temperature and usage. Windows only.

- `crates/core` (`deepcool-core`): HID transport, device protocols, sensors, config, IPC, engine.
- `crates/service` (`deepcool-native.exe`): CLI and Windows service (runs as LocalSystem).
- `crates/tray` (`deepcool-tray.exe`): optional tray icon that talks to the service.
- `third_party/pawnio-modules`: prebuilt PawnIO modules (LGPL-2.1), loaded at runtime.
- `docs/`: protocol notes and the analysis of the official app.

## Commands

```bash
cargo build                          # debug build
cargo test                           # unit tests (no hardware needed)
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo build --release                # optimized binaries in target/release
```

Hardware checks (need a connected cooler and the official DeepCool app closed):

```bash
target/debug/deepcool-native list    # detected devices and the temperature sensor
target/debug/deepcool-native test    # test pattern on the display
target/debug/deepcool-native run     # foreground loop; run elevated for the CPU temperature
```

## Skills

Project skills live in `.agents/skills` and are managed with
[autoskills](https://github.com/midudev/autoskills) (`npx autoskills`); `skills-lock.json` pins them.
Read `.agents/skills/rust-best-practices/SKILL.md` before writing or reviewing Rust code.

## Rules

- Everything in the repository is written in English: code, comments, docs, commit messages.
- Footprint is the main feature. Do not add async runtimes, GUI frameworks or C dependencies.
  New crates need a strong reason; prefer `windows-sys` calls.
- The engine wakes up once per refresh interval and must not allocate or poll in between.
- Keep `unsafe` blocks small and next to the Win32 call they wrap.
- Protocol logic (packet builders, sensor decoding, config parsing) is pure and unit tested;
  update `docs/protocol-*.md` when a protocol changes.
- Do not copy code from GPL projects. Protocol facts are fine; cite the source in `docs/`.
- Do not ship DeepCool assets (logos, fonts, images, videos).
- Conventional commits (`feat(core): ...`, `fix(tray): ...`, `docs: ...`).
