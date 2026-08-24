# logic_sim

A native Rust port of [Sebastian Lague's **Digital Logic Sim**](https://github.com/SebastianLague/Digital-Logic-Sim) — an interactive digital logic circuit simulator. Instead of Unity, this version renders with [wgpu](https://github.com/gfx-rs/wgpu)/[winit](https://github.com/rust-windowing/winit) and keeps the simulation core, save format, and on-disk layout compatible with the original game.

## Features

- **Project picker → chip viewer** in a single window: open an existing project or create a new one, then edit its chips.
- **Built-in components**: NAND gates, clocks, pulses, tri-state buffers, RAM/ROM, bit merge/split converters, 7-segment / RGB / dot / LED displays, buses, buzzers, and keyboard-driven input chips (`KEY`, `MOD KEYS`).
- **Buses**: placing a `BUS` chip places its linked terminus partner with it; the pair wires together like a wire passing through, other wires can tap off anywhere along a bus, and any number of inputs (and outputs) can be wired into it -- everything merges at the origin.
- **Selection & movement**: click a component to select it (faint highlight), drag to move it (translucent while carried, snapping like placement), drag on empty canvas for a rubber-band that selects everything even partially inside it, and `Delete` removes the selection.
- **Buzzer audio**: buzzers drive a 256-slot frequency table through a smoothed square-wave mix played on the real audio device (silently skipped when none is available).
- **Custom chips**: save any circuit as a chip and nest it inside other circuits, just like the original.
- **Chip customization** (save popup → *Customize*): name placement (middle/top/hidden), body colour (palette + hex field), corner-drag resizing previewed live with the chip's own edge pins for scale, and embedded display surfaces you place, move, scale and remove right on the chip body — content clips at the chip edge, and any display that doesn't fully fit is flagged in red. Everything round-trips through the original's standard `Displays` save field.
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
| `P` / `Ctrl+P` | Preferences |
| `R` | Rebuild/restart the simulation |
| `F` | Toggle fit-to-view camera |
| `G` / `Ctrl+G` | Toggle grid |
| `Ctrl+Space` | Pause/resume the simulation |
| `Space` (while paused) | Advance the simulation one step |
| `Esc` | Cancel pending action / close topmost overlay / leave editor |
| `Delete` | Remove the display being carried in Customize |

Mouse: middle-drag to pan, scroll to zoom, click pins to place wires, click a component to select/drag it, drag empty canvas to box-select, right-click for context menus (and to cancel whatever's in progress).

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
├── sim.rs              # event-driven logic simulation over a built chip tree
├── description.rs      # ChipDescription / PinDescription / ChipLibrary types
├── builtins.rs         # pin layouts for every non-custom chip type
├── json.rs             # serde models + load/save of project & chip JSON
├── structs.rs          # shared math/utility types
├── settings.rs         # AppSettings
├── bin/app.rs          # thin entry point (picker → viewer)
├── render/             # wgpu renderer, WGSL shader, camera, theme
│   ├── foundation/     # shared geometry primitives + point-in-shape hit tests
│   ├── scene/          # chip descriptions -> triangles (wires/pins/components/displays/grid)
│   ├── customize_ui.rs # chip-customization workspace (save popup -> Customize)
│   └── ...             # ui_kit, editor/menu UI, context menu, UI stack, gpu
├── viewer/             # the frontend: editor state, canvas interaction,
│                       #   save flows, popups, input routing, frame building
└── save_system/        # paths, loader/saver, versioning, orchestration
tests/                  # integration tests incl. real-project round-trips + fixtures
build.sh                # fmt/clippy/test/build driver (see above)
```

The `logic_sim` crate doubles as a library: the simulator (`Simulator`, `SimChip`), descriptions, save system, and the whole `viewer` frontend are usable headless without a GPU — see `src/lib.rs` for the public surface and `tests/` for usage examples.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). In short: run `./build.sh -y` before opening a PR, follow `rustfmt.toml`, and keep modules documented and small.

## License

MIT — see [LICENSE](LICENSE). Original Digital Logic Sim by Sebastian Lague.
