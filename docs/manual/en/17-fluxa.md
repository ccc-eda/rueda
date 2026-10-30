# 17. Fluxa

Fluxa is rueda's second hardware language, for high-level synthesis (HLS). Where Veryl describes registers and wires
cycle by cycle, a Fluxa program describes **tasks connected by channels**: each `task` runs on its own, channels are
bounded FIFOs, `send` waits while a FIFO is full and `recv` while it is empty. This is a Kahn process network,
so the results do not depend on how the tasks are scheduled or on the FIFO depths (only a deadlock can).
The language is specified in [`fluxa/docs/Fluxa語言規格v0.1.md`](../../../fluxa/docs/Fluxa語言規格v0.1.md) (Traditional Chinese).

```fluxa
task Double(x: in chan<i16>, y: out chan<i17>) {
    for v in x { y.send(v * 2); }          // ends when x is closed and empty
}

test doubles {
    let (tx, rx) = chan::<i16>(depth = 4);
    let (ty, ry) = chan::<i17>(depth = 4);
    spawn Double(rx, ty);                  // the endpoints move into the task
    for k in 0..3 { tx.send(k); }
    tx.close();
    for k in 0..3 { assert ry.recv() == 2 * k; }
}
```

## On the command line

`reda sim` and `reda list` take `.fx` files the way they take `.veryl` files (the extension chooses the language):

```sh
reda sim fluxa/examples/fir.fx                  # every test in the file
reda sim fluxa/rv32i/rv32i.fx fluxa/rv32i/tests_basic.fx   # several files are merged into one program
reda sim fluxa/examples/fir.fx --test fir_impulse
reda sim fluxa/examples/gemm.fx --stats         # per channel: tokens sent, received, largest FIFO occupancy
reda sim fluxa/examples/mac.fx --trace          # every send and recv
reda sim fluxa/examples/fir.fx --coop           # the browser's scheduler (below)
reda list fluxa/examples/fir.fx                 # the tests
```

The exit code is 0 when every test passes, 1 when one fails (an `assert`, a value that does not fit its type,
a deadlock), 2 for a syntax error. A deadlock is reported with what each task was waiting for.

## Two schedulers

On the command line every task is an operating-system thread. WebAssembly has no threads, so in the browser all the
tasks take turns on one thread: the interpreter is asynchronous, and every channel operation is a point where a task
can wait and let another run. `--coop` uses that scheduler on the command line too. Both give the same results (the
regression tests run every example both ways and compare).

## In the web IDE

- **＋ New Fluxa project** (welcome page) starts a project with one `.fx` file; **＋ New file** in a Fluxa project
  makes `.fx` files. The editor highlights Fluxa; syntax errors appear in Problems as you type.
- **▶ Run** runs the tests (all of them, or one from the Tests side bar). The Output panel shows each test's `print`
  output, `[PASS]` / `[FAIL]` with the reason, and the channel statistics.
- The example library has a **Fluxa** group: basics, a FIR filter, a multiply-accumulate unit, a systolic-array
  matrix multiply, a broadcast fan-out, a deadlock (on purpose), a 4-tap FIR, a network of tasks, an RV32I processor written as four pipelined tasks, and an
  RV64G processor (M/S/U modes, Sv39, an IEEE-754 FPU, a mini OS; 90 tests).

## Synthesis

`reda synth / fpga / asic / gatesim` take `.fx` files too: the front end produces the same gate netlist as Veryl's
(`eda::netlist::Netlist`), and the FPGA and ASIC flows after it are the same.

```sh
reda synth --top Fir fluxa/examples/fir.fx          # netlist statistics; -o writes yosys JSON, --verilog Verilog
reda fpga --asc fir4.asc --bin fir4.bin fluxa/examples/fir4.fx   # iCE40 place and route, bitstream
reda asic --pdk sky130 --gds fir.gds fluxa/examples/fir.fx       # SKY130 chip layout
reda gatesim ice40 fluxa/examples/fir.fx            # the tests, with the task replaced by its iCE40 netlist
reda synth --list fluxa/examples/pipeline.fx        # the tasks that can be synthesized
```

- The top is the task named by `--top` (default: the last one). Each channel parameter becomes a stream port
  `NAME_data`, `NAME_close`, `NAME_valid`, `NAME_ready` (a valid / ready handshake; close is the end of the stream);
  the other parameters are input ports, or constants with `--param NAME=VALUE`. Plus `clk` and `rst`.
- A task becomes a finite-state machine with a datapath: every channel operation is a state boundary, and the pure
  computation between them is done in one clock. Widths follow the specification's range inference; fixed-point
  rounding and overflow modes, `@reg` (reads see the old value, committed at the iteration's end), arrays,
  `unrolled()` and functions all become gates.
- A `pipeline(ii = 1)` loop becomes one state: an iteration runs on every clock where its inputs are valid and its
  outputs ready (the FIR takes a sample a clock). Each channel is received at most once and sent at most once per
  iteration, not under an `if`, and no receive comes after a send; otherwise E0412 says why (with `ii >= 2` the loop
  falls back to ordinary states).
- `let m: mem<T, N> = [0; N]` used only as `m[i]` takes a clock per access (the word read goes straight into a
  register) and becomes block RAM on the iCE40 (`SB_RAM40_4K`); other uses keep it as registers, with a warning.
- A task that builds a network (`chan`, `spawn`, `broadcast`) becomes one netlist: a circuit for each spawn and a
  FIFO of the channel's `depth` for each internal channel (`fluxa/examples/pipeline.fx`).
- Channels can carry tuples; an array parameter is one input port (all its bits); `match` / `if` arms may do
  channel operations and `return`. The four stages of the RV32I processor (Fetch, Decode, Execute, Mem) all
  synthesize, and its 24 tests pass at gate level: `reda gatesim opt fluxa/rv32i/rv32i.fx
  fluxa/rv32i/tests_basic.fx` (every task replaced by its netlist, about 7 minutes).
- A generic task gets its const generics' values from `--param NAME=VALUE` (or, in a network or a gate-level test,
  from the spawn's `::<…>`). A network's array parameter becomes one of its input ports, wired to the instances it
  is passed to. So RV32I's `Core` is one netlist (the four stages and the FIFOs between them, about 36,500 generic
  gates): `reda synth --top Core --param WBD=8 fluxa/rv32i/rv32i.fx`, and
  `reda gatesim opt --top Core fluxa/rv32i/rv32i.fx fluxa/rv32i/tests_basic.fx` runs the 24 tests on it (the watchdog test runs
  3,000 instructions and takes about half an hour; the other 23, a minute).
  Its program and data are input ports (4,096 pins), so it does not fit an FPGA's pins yet: that needs them as
  constants (ROM / initialized block RAM).
- Not synthesized yet: a `mem` initialized from a parameter (it stays registers), the RV64 processor (its FPU
  divides and shifts by amounts that are guarded only at run time, and is far too large as one clock of logic).
- In the web IDE a Fluxa project has **⚙ Synthesize** too: ① shows the task and its instances, ②–⑦ are the same as
  for Veryl. With the simulation target set to generic gates / iCE40 / standard cells, **▶ Run** runs the tests with
  the synthesis top replaced by its netlist, and **✔ Verify all** compares every implementation side by side. The
  virtual board does not take Fluxa (its ports are streams, not the board's pins). The example "FIR filter (4 taps)"
  fits an iCE40; "A network of tasks as one circuit" shows a network.

Design and progress: `fluxa/docs/synthesis-plan.md`.
