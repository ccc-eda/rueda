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
  RV64G processor (M/S/U modes, Sv39, an IEEE-754 FPU, a mini OS; 93 tests).

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
- A `--param` value that is not an integer is a Fluxa expression evaluated in the program, so an array parameter can
  be a constant too: read with an index it is a ROM (folded into gates), and a memory initialized from it starts
  with its words. RV32I's `Core` with its program and data built in has only its two output streams as ports, runs
  the program out of reset, and is placed and routed on the ECP5 (21,163 LUT4, 5,516 FF, 72 IO, about 16 MHz):
  ```
  reda fpga --arch ecp5 --bit core.bit --top Core --param WBD=8 --param 'data=dimage([])' \
       --param 'prog=image([addi(1, 0, 0), addi(2, 0, 1), addi(3, 0, 11), add(1, 1, 2), addi(2, 2, 1), bne(2, 3, -8), ecall()])' \
       fluxa/rv32i/rv32i.fx
  ```
- Ranges: in each arm of an `if` (and on the right of `&&` / `||`) the variables the condition bounds have the
  narrower range, so `if sh <= 0 { x << (0 - sh) }` and `if b != 0 { a / b }` synthesize; function results keep a
  range narrower than their declared type.
- `serial for` without channel operations takes a clock per iteration (states); other such loops are unrolled into
  gates. It is how a long computation becomes a multi-cycle unit, and the end of an iteration is a clock boundary.
- A memory initialized from an array parameter (`let m: mem<T, N> = data`) starts with its words: with
  `--param data=…` they are the block RAM's initial contents (iCE40 `INIT_*`, ECP5 `INITVAL_*`); otherwise a
  gate-level simulation loads them (no ports). All reads of a memory share one read port.
- The RV64G processor (`fluxa/rv64`) synthesizes as one netlist and is placed and routed on the ECP5-85: 72,834 LUT
  slots (87%), 12,179 FF, 32 DP16KD, 8.35 MHz. Its FPU is the multi-cycle task `Fpu` in `fpu.fx` (multiplication
  53 clocks, division 116, square root 60; one exact adder and one rounder shared), bit for bit the same as the
  `comb fn` reference `fp_op` on every vector of `tests_fpu.fx`; the M extension is the 64-clock task `MulDiv`.
  ```
  reda fpga --arch ecp5 --clock 5 --bit rv64.bit --top Core --param WBD=8 --param TLBN=8 --param 'data=dimage([])' \
       --param 'prog=image([addi(1, 0, 0), addi(2, 0, 1), addi(3, 0, 11), add(1, 1, 2), addi(2, 2, 1), bne(2, 3, -8), simexit()])' \
       fluxa/rv64/rv64.fx fluxa/rv64/fpu.fx
  ```
- In the web IDE a Fluxa project has **⚙ Synthesize** too: ① shows the task and its instances, ②–⑦ are the same as
  for Veryl. With the simulation target set to generic gates / iCE40 / standard cells, **▶ Run** runs the tests with
  the synthesis top replaced by its netlist, and **✔ Verify all** compares every implementation side by side. The
  virtual board does not take Fluxa (its ports are streams, not the board's pins). The example "FIR filter (4 taps)"
  fits an iCE40; "A network of tasks as one circuit" shows a network.

Design and progress: `fluxa/docs/synthesis-plan.md`.
