# Third-party notices

The MIT licence in `LICENSE` covers only the original work of the ScriptHookVI contributors. The components below keep their own licences.

## Fonts

Embedded in the engine and kept in `assets/`:

- Inter (`regular.ttf`, `bold.ttf`). Copyright (c) 2016 The Inter Project Authors (https://github.com/rsms/inter). SIL Open Font License 1.1.
- JetBrains Mono (`mono.ttf`). Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono). SIL Open Font License 1.1.

Full licence text: `assets/LICENSE.txt`.

## Lua

Lua 5.4.8, compiled in from the source vendored by the `lua-src` crate, through the `mlua` crate. Copyright (C) 1994-2025 Lua.org, PUC-Rio. MIT licence.

## Rust crates

The crates resolved for the `x86_64-pc-windows-msvc` target, from `Cargo.lock`, with the licence each one declares. Where a crate offers a choice, this project takes MIT, or Zlib where MIT is not offered.

| Crate | Version | Licence |
| --- | --- | --- |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 |
| autocfg | 1.5.1 | Apache-2.0 OR MIT |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| bstr | 1.14.0 | MIT OR Apache-2.0 |
| cc | 1.11.3 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| core_maths | 0.1.1 | MIT |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 |
| either | 1.19.0 | MIT OR Apache-2.0 |
| equivalent | 1.0.3 | Apache-2.0 OR MIT |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 |
| find-msvc-tools | 0.1.15 | MIT OR Apache-2.0 |
| flate2 | 1.1.10 | MIT OR Apache-2.0 |
| foldhash | 0.1.5 | Zlib |
| fontdue | 0.9.5 | MIT OR Apache-2.0 OR Zlib |
| hashbrown | 0.15.5 | MIT OR Apache-2.0 |
| iced-x86 | 1.22.0 | MIT |
| lazy_static | 1.5.1 | MIT OR Apache-2.0 |
| libc | 0.2.191 | MIT OR Apache-2.0 |
| libm | 0.2.16 | MIT |
| lock_api | 0.4.14 | MIT OR Apache-2.0 |
| luajit-src | 210.6.6+707c12b | MIT |
| lua-src | 550.0.0 | MIT |
| memchr | 2.8.3 | Unlicense OR MIT |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| mlua | 0.11.6 | MIT |
| mlua-sys | 0.10.0 | MIT |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 |
| png | 0.18.1 | MIT OR Apache-2.0 |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |
| smallvec | 1.16.6 | MIT OR Apache-2.0 |
| ttf-parser | 0.25.1 | MIT OR Apache-2.0 |
| which | 8.0.6 | MIT |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| zlib-rs | 0.6.8 | Zlib |

This project publishes no binaries. An archive that contains a build of it must carry this file, `LICENSE`, `assets/LICENSE.txt` and the licence text of each crate above.
