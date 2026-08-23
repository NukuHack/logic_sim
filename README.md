# logic_sim

A native Rust port of [Sebastian Lague's **Digital Logic Sim**](https://github.com/SebastianLague/Digital-Logic-Sim) — an interactive digital logic circuit simulator. Instead of Unity, this version renders with [wgpu](https://github.com/gfx-rs/wgpu)/[winit](https://github.com/rust-windowing/winit) and keeps the simulation core, save format, and on-disk layout compatible with the original game.

## Features

- **Project picker → chip viewer** in a single window: open an existing project or create a new one, then edit its chips.
- **Built-in components**: NAND gates, clocks, pulses, tri-state buffers, RAM/ROM, bit merge/split converters, 7-segment / RGB / dot / LED displays, buses, buzzers, and keyboard-driven input chips (`KEY`, `MOD KEYS`).
- **Custom chips**: save any circuit as a chip and nest it inside other circuits, just like the original.
- **Compatible saves**: reads and writes the *same* `Projects/` folder layout as the original Unity build (see below), so projects made in either program work in both. Sample projects (GOL, Snake, ZHT90, ...) load out of the box.
- **Editor UI stack**: library panel (`Tab`), search overlay, preferences, ROM editor, key binding, right-click context menus, wire/chip deletion, camera fit-to-view, grid toggle.

## Requirements

- [Rust](https://rustup.rs) (stable toolchain)
- A GPU with Vulkan/Metal/DX12/GL support — there is no software-fallback headless mode for the app

## Building & Running

```sh
cargo run                # build and launch the app
cargo build --release    # binary lands at target/release/app
```

Or use the all-in-one script:

```sh
./build.sh -y   # fmt + clippy + full test suite (+ Miri if available) + release build
./build.sh -q   # unit tests only, then release build
./build.sh -n   # just build
```

## Controls

| Key | Action |
| --- | --- |
| `Tab` | Open/close the chip library |
| `Ctrl+F` | Search |
| `Ctrl+S` | Save current chip |
| `Ctrl+N` | New chip |
| `P` | Preferences |
| `R` | Rebuild/restart the simulation |
| `F` | Toggle fit-to-view camera |
| `G` | Toggle grid |
| `Esc` | Cancel pending action / close topmost overlay / leave editor |

Mouse: drag to pan, scroll to zoom, click pins to place wires, right-click for context menus.

## Where data lives

Save data mirrors the original Unity build's persistent-data location, so both programs share one set of projects:

| Platform | Path |
| --- | --- |
| Linux | `~/.config/unity3d/SebastianLague/Digital-Logic-Sim/` |
| macOS | `~/Library/Application Support/SebastianLague/Digital-Logic-Sim/` |
| Windows | `%USERPROFILE%\AppData\LocalLow\SebastianLague\Digital-Logic-Sim\` |

Layout:

```
Projects/<name>/ProjectDescription.json   # which chips a project contains & their wiring entry points
Projects/<name>/Chips/*.json              # saved custom chip descriptions
AppSettings.json                          # window/editor preferences
```

Saved projects stamped `DLSVersion >= 2.0.0` can be opened; new saves are written as `2.1.6`.

## Project structure

```
src/
├── sim.rs            # event-driven logic simulation over a built chip tree
├── description.rs    # ChipDescription / PinDescription / ChipLibrary types
├── builtins.rs       # pin layouts for every non-custom chip type
├── json.rs           # serde models + load/save of project & chip JSON
├── structs.rs        # shared math/utility types
├── settings.rs       # AppSettings
├── bin/app.rs        # the integrated application (picker → viewer)
├── render/           # wgpu renderer, WGSL shader, camera, theme, editor/menu UI
└── save_system/      # paths, loader/saver, versioning, orchestration
tests/                # integration tests incl. real-project round-trips + fixtures
build.sh              # fmt/clippy/test/build driver (see above)
```

The `logic_sim` crate doubles as a library: the simulator (`Simulator`, `SimChip`), descriptions, and save system are all usable headless without the GUI — see `src/lib.rs` for the public surface and `tests/` for usage examples.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). In short: run `./build.sh -y` before opening a PR, follow `rustfmt.toml`, and keep modules documented and small.

## License

MIT — see [LICENSE](LICENSE). Original Digital Logic Sim by Sebastian Lague.
