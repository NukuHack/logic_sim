# Logic Sim

A native Rust port of [Sebastian Lague's **Digital Logic Sim**](https://github.com/SebastianLague/Digital-Logic-Sim) — an interactive digital logic circuit simulator. Rendering runs on [wgpu](https://github.com/gfx-rs/wgpu)/[winit](https://github.com/rust-windowing/winit) instead of Unity, but the simulation core, save format, and on-disk project layout stay fully compatible with the original game.

## Features

It is the same as the original with my tweaks, and extensions  
i made sure anything that you save in this version is compatible with the original  

## Requirements

- [Rust](https://rustup.rs) (stable toolchain)
- A GPU with Vulkan, Metal, DX12, or GL support — there's no software-fallback or headless mode for the app itself

## Building & running

```sh
cargo run                # build and launch the app
cargo build --release    # binary lands at target/release/app
```

Or use the all-in-one script:

| Command | Runs |
| --- | --- |
| `./build.sh -y` | fmt + clippy + full test suite (+ Miri if available) + release build |
| `./build.sh -c` | same as `-y`, without Miri |
| `./build.sh -q` | unit tests only, then release build |
| `./build.sh -n` | just build |
| `./build.sh -r` | everything, but will run it too |

## Controls

| Key | Action |
| --- | --- |
| `Ctrl+L` | Open/close the chip library |
| `Ctrl+F` | Search |
| `Ctrl+S` | Save current chip |
| `Ctrl+N` | New chip |
| `Ctrl+P` | Preferences |
| `Ctrl+R` | Rebuild/restart the simulation |
| `Ctrl+F` | Toggle fit-to-view camera |
| `Ctrl+G` | Toggle grid |
| `(Ctrl/Shift)+D` | Duplicate selected |
| `Ctrl+Space` | Pause/resume the simulation |
| `Space` (while paused) | Advance the simulation one step |
| `Esc` | Cancel pending action / close topmost overlay / leave viewed chip / leave editor |
| `Delete` | Remove/Delete current selected |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo the current chip's edit history |

**Mouse:** middle-drag to pan, scroll to zoom, click pins to place wires, click a component to select or drag it, drag on empty canvas to box-select, right-click for context menus (and to cancel whatever's in progress).

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

Projects stamped `DLSVersion >= 2.0.0` can be opened. Chips last saved by versions at or before 2.1.4 are migrated automatically on load (the pre-2.1.5 ORANGE palette-index shift and default LED colour data), and every save is re-stamped with the current version, `2.1.6`.

## Contributing

- [CONTRIBUTING.md](CONTRIBUTING.md). In short: run `./build.sh -y` before opening a PR, follow `rustfmt.toml`, and keep modules documented and small.  

- there are alwas a lot stuff to do   

## License

- [LICENSE](LICENSE). Original Digital Logic Sim by Sebastian Lague.
