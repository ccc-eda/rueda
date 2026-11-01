# 13. Verification and development

This chapter is for people who want to check the tools' trustworthiness, or modify rueda itself.

## How we know the tools are right

| Check | What it proves | Where |
|---|---|---|
| RTL simulation vs official | `reda sim`'s semantics match the official `veryl test` / iverilog | the examples' testbenches |
| Gate-level simulation | the same testbench passes on the generic-gate, iCE40, vlib, and SKY130 netlists | `cargo test`, `reda gatesim` |
| **Formal equivalence** | the synthesized netlist is **mathematically equivalent** to the SystemVerilog produced by the official `veryl build` (combinational circuits fully proven; sequential circuits by k-induction or bounded checking) | `veryl/synth/test.sh` (yosys + slang) |
| SKY130 netlist | yosys reads the cells with the PDK's Liberty functions and matches the RTL | `veryl/synth/test.sh` |
| Bitstream | the `.asc` from our own place & route is converted back to a circuit with IceStorm's `icebox_vlog` and compared cycle by cycle against the netlist with random inputs; the `.bin` matches icepack byte for byte | `veryl/synth/fpga/pnr_test.sh`, `veryl/synth/fpga/cosim.py` |
| FPGA flow | the RISC-V SoC is simulated once from the mapped netlist and once from the routed bitstream | `veryl/synth/fpga/flow.sh` |
| Layout | DRC with 0 violations; KLayout reads the GDS back and checks cell counts, non-overlap, widths and spacings | the `asic` flow, `eda/tools/gds_check.py` |
| C compiler | `riscv/rvtools/tests/*.c` produce the same output as the host's `cc`; RISC-V programs match spike | `cargo test` |
| Virtual board | the board engine matches netsim; bitstream decoding matches nextpnr's result | `cargo test board`, `cargo test bitstream` |
| Web | every example is run through the same wasm + TypeScript API the web page uses | `npm test` |

## Running the tests

```sh
./test.sh            # what CI runs: cargo test of the workspace, reda sim, the tutorial's flow, reda emu's check,
                     # the Fluxa processors, rvemu's JIT, npm test
./test.sh --full     # additionally veryl/synth/test.sh (needs OSS CAD Suite, tens of minutes)
```

Each step keeps running after failures and prints a summary at the end. Individually:

```sh
cargo test --release                            # the whole workspace: eda, riscv, veryl, fluxa, reda
cargo test --release -p rueda-veryl-synth       # synthesizes all examples, runs testbenches on netlists
cargo test --release -p rueda --test cli        # only the reda subcommands
veryl/synth/test.sh                             # formal equivalence + FPGA flow + SKY130 (incl. GDS check)
veryl/synth/fpga/pnr_test.sh                    # our own place & route: each design's bitstream vs netlist + icetime
cd web && npm test                              # web core and IDE logic (needs npm run build first)
cd web && npm run e2e                           # the IDE end to end in Chrome (Playwright): flow diagrams, steps, Fluxa
```

Environment variables used by `veryl/synth/test.sh`:

| Variable | Purpose |
|---|---|
| `YOSYS_SLANG` | a yosys with the slang frontend (OSS CAD Suite's). Homebrew's yosys cannot read the enum casts and functions in `veryl build`'s output |
| `GDS_PY` | a Python with the KLayout module (`pip install klayout`), used to check the SKY130 GDS; if unset, `python3` is used when it has it |
| `OUT` | output directory (default: a temp directory) |
| `VERYL_EDA_CHIPDB` | IceStorm chip database; if `./test.sh` can't find it, it is extracted from `web/dist/chipdb.js` |

The equivalence check uses `-set-init-zero`, because the optimizer assumes flip-flops power up at 0 (true on iCE40).

## Adding a test design

- Synthesis test designs go in `veryl/synth/examples/` and into the `DESIGNS` list of `veryl/synth/test.sh` (format `filename:module:mode`,
  mode `comb` / `ind` / `bmcN`). These files are copied into the official Veryl project to produce the golden result, so module names must not clash with `veryl/examples/src`.
- Use `veryl check` (run inside `veryl/examples/`) to confirm syntax; `reda`'s "not supported" is not a syntax error, just a feature this project doesn't support.

## Source map

rueda is one Cargo workspace. The gate netlist (`eda::netlist::Netlist`) is the line between the languages and the back end:
a front end (Veryl today, Fluxa later) produces it, and nothing after it knows the source language.

| Directory | Content |
|---|---|
| `reda/` | the `reda` command: one subcommand per tool (`sim`, `synth`, `fpga`, `asic`, `gatesim`, `emu`, `cc`, `rvemu`, `pdk`); the language comes from the files' extension |
| `eda/src/` | the language-independent back end (crate `rueda-eda`): `netlist.rs` gate netlist and optimization, `bram.rs`, `output.rs` (yosys JSON / Verilog), `netsim.rs`, `sdc.rs`, `power.rs` |
| `eda/src/fpga/` | `ice40.rs`, `ecp5.rs` technology mapping; `pnr/` our own iCE40 place & route and bitstream; `board/` the virtual board (engine, iCEBreaker, bitstream decoding, peripherals) |
| `eda/src/asic/`, `eda/src/pdk/` | the standard-cell flow (`map` `place` `route` `detail` `sta` `layout` `drc` `gds`); Liberty / LEF / DEF readers and the SKY130 binding; `eda/pdk/sky130hd/` the bundled subset |
| `eda/bits/` | the fixed-width bit vectors shared by eda and the front ends |
| `eda/tools/` | SKY130 download and subset, KLayout GDS / LVS / antenna checks |
| `veryl/sim/` | the Veryl simulator: `elab.rs` elaboration, `sim.rs` event-driven simulation, `value.rs` 4-state values, `gates.rs` gate models |
| `veryl/synth/` | Veryl synthesis: `elab.rs` symbolic execution into eda's gates, `gatesim.rs`, `cli.rs`; `fpga/` the yosys / nextpnr comparison flows, `test.sh` |
| `veryl/emu/` | the compiled emulator: a design to WebAssembly (`ir.rs`, `gen.rs`, `wasm.rs`), `js/emu.mjs` |
| `veryl/examples/`, `veryl/tutorial/` | Veryl examples (an official Veryl project; `rv64/` the RV64 processor with xv6 / Linux, `c/` C programs, `sw/` assembly tests); the tutorial and the virtual lab course |
| `riscv/rvtools/` | the C compiler, assembler and RV32IMA instruction-set simulator |
| `riscv/rvemu/` | the RV64 board's instruction-set emulator (with a WebAssembly JIT) |
| `fluxa/` | the Fluxa language: `sim/` its simulator (a Kahn process network interpreter), `examples/`, `rv32i/`, `rv64/`, `docs/` |
| `web/` | the IDE (TypeScript); `web/wasm/` the WebAssembly core (C ABI, JSON in and out); `web/dist/` the built page (committed to git) |
| `docs/manual/` | this manual |

More detail is in `AGENTS.md` and `web/PLAN.md`.

## Building

```sh
./build.sh                 # reda + web/dist; rebuild web/dist after changing Rust or TypeScript
./build.sh --no-web        # only reda (target/release/reda)
```

`run.sh` gathers the everyday commands (it calls build.sh, test.sh, the Fluxa scripts and web's npm scripts):

```sh
./run.sh help              # every command
./run.sh build | web | web-fast | typecheck
./run.sh serve [PORT]      # web/dist on http://localhost:PORT (default 8000), opens the IDE
./run.sh open              # web/dist/index.html as a file (no server)
./run.sh reda ARGS...      # target/release/reda (built if missing)
./run.sh test [--full] | test-web | e2e | fluxa | fluxa-linux | rv64
./run.sh linux | size | clean
```

The documentation comes in three languages: `README.md` (English), `README.tw.md` (Traditional), `README.cn.md` (Simplified, converted from Traditional by `tools/tocn.py`).
Web interface text is written as `tr('繁體', 'English')`; Simplified is derived from the mapping tables automatically.

CI (`.github/workflows/ci.yml`) runs the content of `./test.sh`, and on pushes to main deploys the freshly built `web/dist` to GitHub Pages.
