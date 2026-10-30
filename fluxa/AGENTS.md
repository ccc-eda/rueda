# AGENTS.md

Fluxa: behavioral simulator (Kahn process network interpreter) for the Fluxa HLS language. Pure Rust, **zero external dependencies** — do not add crates. Docs (incl. README) are in Traditional Chinese; the language spec is `docs/Fluxa語言規格v0.1.md` (README calls it 〈Fluxa 語言規格 v0.1〉; "spec §x" below refers to it). Simulator version history: `docs/v0.1.md`, `docs/v0.2.md`, `docs/v0.3.md`. `docs/Fluxa發想-gemini.md` / `docs/Fluxa設計報告-claude.md` are early design notes, not normative.

## Commands

```
cargo build --release
./target/release/fluxa examples/fir.fx              # run all tests in file
./target/release/fluxa a.fx b.fx                    # multi-file: merged like `iverilog a.v b.v`
./target/release/fluxa examples/fir.fx --test fir_impulse
./target/release/fluxa examples/fir.fx --list       # list tests
./target/release/fluxa examples/mac.fx --trace      # print every send/recv
./target/release/fluxa examples/gemm.fx --stats     # per-channel token counts / max FIFO occupancy
./test.sh                                           # everything: build + cargo test + rv32i.sh + rv64.sh
./rv32i.sh                                          # RV32I core example: rv32i.fx + smoke.fx + tests_basic.fx
./rv64.sh                                           # RV64G core (v0.3 M6: RV64IMAFD + Zicsr + M/S/U + Sv39 + counters/timer interrupts): rv64.fx + fpu.fx + each tests_*.fx (109 tests); design + per-milestone notes in docs/v0.3.md
python3 rv64/fpgen.py > rv64/tests_fpu.fx          # regenerate F/D reference vectors (deterministic seed)
cargo test --release                                # full regression suite (examples/ + rv32i/ + rv64/)
```

- Exit codes: 0 = all pass, 1 = test failure, 2 = load/syntax error.
- Multi-file loading: fn names must be unique across files (merge is an error otherwise); `--list`/`--test` see the merged program. Error line numbers encode file index internally (`ast::FILE_STRIDE` = 1_000_000) and are decoded for display via `Program::loc` — don't compare raw line numbers.
- `rv32i.sh` / `rv64.sh` run every file even if one fails, and exit non-zero if any failed.
- There is no CI or formatter config; verify with `cargo test --release`. `cargo clippy --release --all-targets` is expected to be warning-free. MSRV is Rust 1.82 (`rust-version` in Cargo.toml).

## Testing

- `tests/examples.rs` is the regression suite: every `.fx` in `examples/`, plus the RV32I (`rv32i.fx` + each test file) and RV64 (`rv64.fx` + `fpu.fx` + each `tests_*.fx`) suites via `load_many`. Adding a new `rv*/tests_*.fx` means adding a `#[test]` line there and a line in the matching `.sh`. `err_*.fx` examples are **expected-to-fail** and assert error message substrings (e.g. `"does not fit i24"`, `"deadlock"`, `"does not own the rx endpoint"`). Changing an error message's wording breaks these tests — update `tests/examples.rs` in the same change.
- `no_false_deadlocks_under_repetition` runs gemm+fanout 100× checking for scheduling race flakiness. Run with `--release`; the deadlock-detection test is timing-sensitive in debug builds.
- To run a single example's tests by name: `./target/release/fluxa examples/fir.fx --test <name>`.
- `rv32i/err_*.fx` are expected-to-fail demos, asserted in `tests/examples.rs` (`err_wb_depth` → "deadlock", `err_bad_immediate` → "does not fit i12"). The rv32i harness drains the dump channel before `io`, so a harness channel must be deep enough for a program's total MMIO output or the pipeline deadlocks before reaching ECALL.

## Deliberate scope boundaries (do not "fix")

These are intentionally unimplemented per README / spec; encountering them should produce a clear error or be parsed-and-ignored, not implemented:

- `stage` layer and `;;` (cycle-accurate semantics are a different simulator).
- Static checks: II reachability (E0412), bank conflicts (E0401), SDF rate balance (E0501).
- `pipeline(ii = n)` and `schedule`: parsed, no behavioral effect.
- Non-blocking channel ops (`try_recv`/`empty`) are forbidden — they would break KPN determinism.

## Core semantics (breaking these breaks the spec)

- Determinism (spec §3.10 property 5): output token sequences must be independent of scheduling and FIFO depth. Every task is an OS thread; channels are bounded FIFOs with blocking send/recv.
- Channel endpoints (`Tx`/`Rx`) are linear resources: `spawn` transfers ownership; using an endpoint after transfer is an error. Tasks close owned `Tx` on exit (end-of-stream) so `for x in rx` terminates.
- Arithmetic uses exact 120-bit integers with dynamic range checks at typed landing sites (`let`, `carry`, params, channel/array elements) → E0301. Narrowing must be explicit: `as`, `.to::<T>(round=, ovf=)`, `.sat::<T>()`, `.wrap::<T>()`.
- `@reg`: reads inside an iteration see the old value; writes commit at iteration end (matches hardware shift-register behavior in `unroll`).
- `fixed<I,F>`: exact arithmetic — addition aligns fractional bits, multiplication adds them.

## Layout

`src/lexer.rs` → `parser.rs` → `ast.rs` → `interp.rs` (tree-walking interpreter) with `runtime.rs` (scheduler, channels, deadlock detection), `value.rs` (values/types/fixed-point). Entry points: `lib.rs` (`load`/`load_many`/`run_test`, used by both `main.rs` and `tests/examples.rs`) and `main.rs` (CLI).
