# AGENTS.md

rueda: an EDA in Rust. One Cargo workspace; one command, `reda` (git style), for every tool.

## Layout and the one rule

The gate netlist `eda::netlist::Netlist` (yosys internal cells: `$_AND_`, `$_DFF_P_`, …) is the boundary
between the languages and the back end. **`eda/` must not depend on any language front end** (no
`veryl-parser`, no `veryl_*`, no `fluxa`); front ends depend on `eda`, never the reverse.

| Path | Crate (lib) | What |
|---|---|---|
| `reda/` | `rueda` (bin `reda`) | the command; `src/main.rs` dispatches subcommands to each crate's `cli` module, choosing the language by file extension (`.veryl` / `.fx`) |
| `eda/` | `rueda-eda` (`eda`) | netlist, bram, output, netsim, sdc, power; `fpga/` (ice40, ecp5, pnr, board); `asic/`, `pdk/`; `cli.rs` (`reda pdk`) |
| `eda/bits/` | `rueda-bits` (`bits`) | the `Bits` vector shared by eda and the front ends |
| `veryl/sim/` | `rueda-veryl-sim` (`veryl_sim`) | Veryl event-driven simulator |
| `veryl/synth/` | `rueda-veryl-synth` (`veryl_synth`) | Veryl → netlist (`elab.rs`), `gatesim.rs`, `cli.rs` (`reda synth/fpga/asic/gatesim`); `test.sh`, `fpga/` yosys/nextpnr comparison flows |
| `veryl/emu/` | `rueda-veryl-emu` (`veryl_emu`) | Veryl → WebAssembly |
| `fluxa/sim/` | `rueda-fluxa` (`fluxa`) | Fluxa simulator; see `fluxa/AGENTS.md` (zero dependencies, async interpreter, two schedulers) |
| `riscv/rvtools/` | `rueda-rvtools` (`rvtools`) | C compiler, assembler, RV32 ISS; `cli.rs` (`reda cc`) |
| `riscv/rvemu/` | `rueda-rvemu` (`riscv_emu`) | RV64 board emulator, wasm JIT (`.cargo/config.toml` holds its wasm32 link flags) |
| `web/wasm/` | `rueda-wasm` | the web core: C ABI, JSON in / out (`src/api.rs` `dispatch`) |
| `web/` | — | TypeScript IDE; `build.mjs` builds `web/dist/` (committed) |

Each tool crate exposes `cli::main(args)` (args without the program name); there are no other binaries.
A new tool = a `cli` module in its crate + an entry in `COMMANDS` and the `match` of `reda/src/main.rs`
+ a case in `reda/tests/cli.rs`.

## Commands

```
cargo build --release -p rueda          # target/release/reda
cargo test --release                    # the whole workspace (veryl/synth's tests take ~5 min)
./test.sh                               # what CI runs (cargo test, tutorial, emulators, Fluxa CPUs, npm test)
./test.sh --full                        # + veryl/synth/test.sh (yosys / nextpnr / KLayout; OSS CAD Suite)
./build.sh                              # reda + web/dist (cargo --profile web for the wasm32 crates)
cd web && npm test                      # the web core and IDE logic on every example (set VERYL_EDA_CHIPDB,
                                        #   e.g. from `node web/test/chipdb.mjs FILE`, for the P&R comparisons)
cd web && npm run e2e                   # the IDE end to end in the installed Chrome (playwright-core, no browser
                                        #   download): stage diagrams drawn without a click, steps bar, Fluxa runs
```

## Conventions

- Tests read examples by paths relative to `CARGO_MANIFEST_DIR` (`../examples`, `../../riscv/rvtools/tests`, …);
  moving files means updating those paths and the scripts (`*.sh` call `$REDA`, i.e. `target/release/reda`).
- Environment variables keep their old names: `VERYL_EDA_CHIPDB`, `VERYL_EDA_SKY130`, `VERYL_EDA_PROFILE`.
- Web UI text is `tr('繁體中文', 'English')`; docs come in English, 繁體中文 and 简体中文 (`tools/tocn.py`).
- Fluxa has no synthesis yet: `reda synth/fpga/asic/gatesim` refuse `.fx` files, and the IDE turns those
  actions off for Fluxa projects with a reason. A Fluxa front end would produce an `eda::netlist::Netlist`.
- Per-area notes: `veryl/AGENTS.md` (Veryl tools, from veryl2chip), `fluxa/AGENTS.md`, `web/PLAN.md`.
