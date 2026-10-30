# AGENTS.md

Fluxa: behavioral simulator (Kahn process network interpreter) for the Fluxa HLS language, one of rueda's languages (`reda sim FILE.fx`, and Fluxa projects in the web IDE). The crate is `sim/` (`rueda-fluxa`, lib `fluxa`). Pure Rust, **zero external dependencies** — do not add crates. Docs (incl. README) are in Traditional Chinese; the language spec is `docs/Fluxa語言規格v0.1.md` (README calls it 〈Fluxa 語言規格 v0.1〉; "spec §x" below refers to it). Simulator version history: `docs/v0.1.md`, `docs/v0.2.md`, `docs/v0.3.md`. `docs/Fluxa發想-gemini.md` / `docs/Fluxa設計報告-claude.md` are early design notes, not normative.

## Commands

```
cargo build --release -p rueda                      # reda, at the workspace root (target/release/reda)
reda sim examples/fir.fx                            # run all tests in file
reda sim a.fx b.fx                                  # multi-file: merged like `iverilog a.v b.v`
reda sim examples/fir.fx --test fir_impulse
reda list examples/fir.fx                           # list tests
reda sim examples/mac.fx --trace                    # print every send/recv
reda sim examples/gemm.fx --stats                   # per-channel token counts / max FIFO occupancy
reda sim examples/fir.fx --coop                     # the cooperative scheduler (the browser's)
./test.sh                                           # everything: build + cargo test + rv32i.sh + rv64.sh
./rv32i.sh                                          # RV32I core example: rv32i.fx + smoke.fx + tests_basic.fx
./rv64.sh                                           # RV64G core (v0.3 M6: RV64IMAFD + Zicsr + M/S/U + Sv39 + counters/timer interrupts): rv64.fx + fpu.fx + each tests_*.fx (109 tests); design + per-milestone notes in docs/v0.3.md
python3 rv64/fpgen.py > rv64/tests_fpu.fx          # regenerate F/D reference vectors (deterministic seed)
cargo test --release -p rueda-fluxa                 # full regression suite (examples/ + rv32i/ + rv64/), both schedulers
```

- Exit codes: 0 = all pass, 1 = test failure, 2 = load/syntax error.
- Multi-file loading: fn names must be unique across files (merge is an error otherwise); `--list`/`--test` see the merged program. Error line numbers encode file index internally (`ast::FILE_STRIDE` = 1_000_000) and are decoded for display via `Program::loc` — don't compare raw line numbers.
- `rv32i.sh` / `rv64.sh` run every file even if one fails, and exit non-zero if any failed.
- Verify with `cargo test --release -p rueda-fluxa` (the workspace CI runs it too). `cargo clippy --release -p rueda-fluxa --all-targets` is expected to be warning-free. MSRV is Rust 1.82 (`rust-version` in Cargo.toml).

## Testing

- `sim/tests/examples.rs` is the regression suite (every test runs on both schedulers, and their pass/fail and first message line must agree): every `.fx` in `examples/`, plus the RV32I (`rv32i.fx` + each test file) and RV64 (`rv64.fx` + `fpu.fx` + each `tests_*.fx`) suites via `load_many`. Adding a new `rv*/tests_*.fx` means adding a `#[test]` line there and a line in the matching `.sh`. `err_*.fx` examples are **expected-to-fail** and assert error message substrings (e.g. `"does not fit i24"`, `"deadlock"`, `"does not own the rx endpoint"`). Changing an error message's wording breaks these tests — update `tests/examples.rs` in the same change.
- `no_false_deadlocks_under_repetition` runs gemm+fanout 100× checking for scheduling race flakiness. Run with `--release`; the deadlock-detection test is timing-sensitive in debug builds.
- To run a single example's tests by name: `reda sim examples/fir.fx --test <name>`.
- `rv32i/err_*.fx` are expected-to-fail demos, asserted in `sim/tests/examples.rs` (`err_wb_depth` → "deadlock", `err_bad_immediate` → "does not fit i12"). The rv32i harness drains the dump channel before `io`, so a harness channel must be deep enough for a program's total MMIO output or the pipeline deadlocks before reaching ECALL.

## Deliberate scope boundaries (do not "fix")

These are intentionally unimplemented per README / spec; encountering them should produce a clear error or be parsed-and-ignored, not implemented:

- `stage` layer and `;;` (cycle-accurate semantics are a different simulator).
- Static checks in the simulator: II reachability (E0412: synthesis checks it, `synth/src/lower.rs`), bank conflicts (E0401), SDF rate balance (E0501).
- `pipeline(ii = n)` and `schedule`: parsed, no behavioral effect in the simulator (synthesis pipelines `ii` loops).
- Non-blocking channel ops (`try_recv`/`empty`) are forbidden — they would break KPN determinism.

## Core semantics (breaking these breaks the spec)

- Determinism (spec §3.10 property 5): output token sequences must be independent of scheduling and FIFO depth. The interpreter is async (channel operations are its await points): natively every task is an OS thread whose waits block; with `Options::coop` (`--coop`, and always on wasm32, which has no threads) all tasks are futures on one thread, scheduled by `Shared::run_coop`, and a full round without progress is a deadlock. Channels are bounded FIFOs with blocking send/recv.
- Channel endpoints (`Tx`/`Rx`) are linear resources: `spawn` transfers ownership; using an endpoint after transfer is an error. Tasks close owned `Tx` on exit (end-of-stream) so `for x in rx` terminates.
- Arithmetic uses exact 120-bit integers with dynamic range checks at typed landing sites (`let`, `carry`, params, channel/array elements) → E0301. Narrowing must be explicit: `as`, `.to::<T>(round=, ovf=)`, `.sat::<T>()`, `.wrap::<T>()`.
- `@reg`: reads inside an iteration see the old value; writes commit at iteration end (matches hardware shift-register behavior in `unroll`).
- `fixed<I,F>`: exact arithmetic — addition aligns fractional bits, multiplication adds them.

## Layout

Synthesis (`synth/`, crate rueda-fluxa-synth): `lower.rs` (unique names, CFG blocks = FSM states; channel ops, `mem` accesses and `pipeline` loops become terminators), `gen.rs` (type pass with widening, then gates: registers, state, stream ports, `$mem:` nets for block RAM), `ty.rs` (widths, fixed point), `graph.rs` (a task that spawns: elaborated by the interpreter with a recording provider, one netlist with FIFOs), `hw.rs` (`HwSim` / `Provider`: the netlist run through `runtime::HwProvider` for `reda gatesim`). Tests: `synth/tests/gatesim.rs` checks every gate-level run against the behavioral one test by test — a difference is a synthesis bug.

`sim/src/lexer.rs` → `parser.rs` → `ast.rs` → `interp.rs` (async tree-walking interpreter; `eval`, `eval_type`, `exec_body`, `match_pat` return boxed futures to break the recursion) with `runtime.rs` (threads or the cooperative scheduler, channels, deadlock detection), `value.rs` (values/types/fixed-point). Entry points: `lib.rs` (`load`/`load_many`/`run_test`, used by `cli.rs`, `sim/tests/examples.rs` and the web core `web/wasm`) and `cli.rs` (`reda sim` / `reda list` for .fx files).
