# ScriptHookVI

ScriptHookVI, for Grand Theft Auto VI. 

ScriptHookVI is a library loaded into the game's process that runs custom scripts there. Scripts are written in Lua. They read and write the process's memory, call functions in it, draw over its window and build menus.

This repository holds the engine only. It contains no data about any game: no addresses, no native function names and no build tables. As published it can be injected into a process and run scripts there, but it cannot call the game's native functions. It ships with a preview mode that needs no other software.

For ScriptHookVI in use, see [Native Trainer VI](https://github.com/jamubc/NativeTrainerVI), a trainer menu written for it.

## What is in this repository

| Piece | Source | What it does |
| --- | --- | --- |
| `ScriptHookVI.dll` | `src/` | The engine. Injected into a process, it runs the scripts there and draws through its own transparent window over the process's window. |
| `ScriptHookVI-inject.exe` | `src/bin/inject.rs` | Loads the DLL into a process, named by its executable name or its pid. |
| `ScriptHookVI-external.exe` | `src/bin/external.rs` | The same engine attached from outside. It reads, writes and scans memory but cannot call functions in the process. |
| Lua library | `src/prelude.lua` | `on_frame`, `script` and `wait`, the memory helpers, signatures, and the menu library. |
| Preview | `examples/preview.rs`, `preview.sh` | Loads a scripts folder with no process attached and writes what the scripts draw to a PNG. |
| Example script | `examples/scripts/example.lua` | A small menu. |

`build.rs` compiles one optional source file when it is present; it is not in this repository.

## Build

The project is developed and run under Wine on Linux. Building and running on Windows is unverified at time of writing this.

You need Rust 1.88 or newer with the `x86_64-pc-windows-msvc` target, [cargo-xwin](https://github.com/rust-cross/cargo-xwin), clang and lld, Wine, and ImageMagick 7 for the preview.

```bash
rustup target add x86_64-pc-windows-msvc
cargo install --locked cargo-xwin
cargo xwin build --release --examples --bins --lib --target x86_64-pc-windows-msvc
```

Success: the build ends with a `Finished` line, and `target/x86_64-pc-windows-msvc/release/` holds `ScriptHookVI.dll`, `ScriptHookVI-inject.exe` and `ScriptHookVI-external.exe`.

## Try it with no other program

```bash
SCRIPTS=examples/scripts ./preview.sh /tmp/example.png
```

Success: the terminal shows `ScriptHookVI: loaded example.lua` and the path of the picture, which shows the example menu.

## Documentation

The pages in `docs/` cover [installing and running](docs/getting-started.mdx), [scripts](docs/scripts.mdx), [menus](docs/menus.mdx), [drawing](docs/drawing.mdx), [memory](docs/memory.mdx) and the [Lua API](docs/lua-api.mdx).

## Legal

See [LEGAL.md](LEGAL.md), [SECURITY.md](SECURITY.md) and [CONTRIBUTING.md](CONTRIBUTING.md). 

The code is under the [MIT licence](LICENSE); third-party components are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
