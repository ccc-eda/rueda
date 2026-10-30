# rueda
**English** | [繁體中文](README.tw.md)

An EDA for IC design, written in Rust: hardware languages (Veryl, Fluxa) → simulation → synthesis →
FPGA bitstream or ASIC layout, with RISC-V software tools and a web IDE that runs all of it in the browser.

**Try it online** (no install): [IDE](https://ccc-eda.github.io/rueda/web/dist/index.html) ·
user manual: [docs/manual/](docs/manual/README.md)

## One command: `reda`

Every tool is a subcommand of `reda`, like `git`:

```sh
reda sim counter.veryl                 # run the testbenches (Veryl); the language comes from the extension
reda sim fir.fx                        # run the tests of a Fluxa program
reda synth -o adder.json adder.veryl   # synthesize to a gate netlist (yosys JSON / Verilog)
reda fpga --pnr veryl --pcf pins.pcf --bin top.bin top.veryl   # iCE40: map, place & route, bitstream
reda asic --pdk sky130 --gds alu.gds alu.veryl                 # standard cells to a DRC-clean GDS
reda gatesim ice40 fifo.veryl          # the testbenches on the synthesized netlist
reda emu --top Rv64Soc -o soc.wasm *.veryl   # compile a design to WebAssembly (fast simulation)
reda cc --run hello.c                  # C for the RISC-V SoC, run on the instruction-set simulator
reda rvemu fw_payload.bin              # the RV64 board emulator (OpenSBI + Linux)
reda help [SUBCOMMAND]
```

Install: `cargo install --git https://github.com/ccc-eda/rueda rueda`, or build from a checkout (below).

## Layout

The gate netlist (`eda::netlist::Netlist`) separates the languages from the back end: a language front end
produces it, and nothing after it knows the source language.

```
rueda/
├── reda/            the reda command (one subcommand per tool)
├── eda/             language-independent back end
│   ├── src/           gate netlist, block RAM, writers, gate-level simulation, timing constraints, power
│   ├── src/fpga/      iCE40 / ECP5 mapping, own iCE40 place & route + bitstream, the virtual iCEBreaker
│   ├── src/asic/      standard-cell flow: place, clock tree, route, timing, DRC / LVS, GDS
│   ├── src/pdk/       Liberty / LEF / DEF, SKY130 binding;  pdk/sky130hd: the bundled cells
│   ├── bits/          bit vectors shared with the front ends
│   └── tools/         SKY130 download, KLayout GDS / LVS / antenna checks
├── veryl/           the Veryl language
│   ├── sim/           event-driven simulator
│   ├── synth/         Veryl → eda's gate netlist, post-synthesis simulation
│   ├── emu/           Veryl → WebAssembly (cycle-based, like Verilator)
│   ├── examples/      designs, RISC-V SoCs, the RV64 board (xv6, Linux), C programs
│   └── tutorial/      from-zero tutorial and the virtual lab course
├── fluxa/           the Fluxa language (HLS: tasks and channels, a Kahn process network)
│   ├── sim/           simulator (threads, or a cooperative scheduler in the browser)
│   ├── examples/      FIR, systolic array, …;  rv32i/, rv64/: processors written in Fluxa
│   └── docs/          language specification and version notes
├── riscv/           RISC-V software
│   ├── rvtools/       C compiler, assembler, RV32IMA instruction-set simulator
│   └── rvemu/         RV64IMAC board emulator with a WebAssembly JIT
├── web/             the web IDE (TypeScript);  wasm/: its WebAssembly core;  dist/: the built page
└── docs/manual/     user manual (English, 繁體中文, 简体中文)
```

| Language | Simulation | Synthesis, FPGA, ASIC | Web IDE |
|---|---|---|---|
| Veryl (`.veryl`) | `reda sim`, `reda emu` | `reda synth / fpga / asic / gatesim` | yes |
| Fluxa (`.fx`) | `reda sim` | not yet (the next front end for eda) | yes (tests) |

## Building and testing

Needs Rust; the web page also needs the `wasm32-unknown-unknown` target and Node.js.

```sh
./build.sh             # target/release/reda and web/dist (--no-web: reda only)
./test.sh              # cargo test of the workspace, the tutorial's flow, the emulators, the Fluxa processors, npm test
./test.sh --full       # + formal equivalence with yosys, the nextpnr flows (needs OSS CAD Suite)
```

Open `web/dist/index.html` in a browser for the IDE (no server needed). More: [docs/manual/en/13-develop.md](docs/manual/en/13-develop.md),
[AGENTS.md](AGENTS.md).

## History

rueda joins two earlier projects: [veryl2chip](veryl/history.md) (Veryl to chip; its version history is in
`veryl/history.md`) and Fluxa (`fluxa/docs/`).

## License

MIT
