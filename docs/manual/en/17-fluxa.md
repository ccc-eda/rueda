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
  matrix multiply, a broadcast fan-out, a deadlock (on purpose), an RV32I processor written as four pipelined tasks, and an
  RV64G processor (M/S/U modes, Sv39, an IEEE-754 FPU, a mini OS; 90 tests).

## What is not there yet

Fluxa has behavioral simulation only. `reda synth / fpga / asic / gatesim` refuse `.fx` files, and the IDE turns
those buttons off for a Fluxa project. Synthesis would be a Fluxa front end producing the same gate netlist as
Veryl's (`eda::netlist::Netlist`); from there the FPGA and ASIC flows need no change.
