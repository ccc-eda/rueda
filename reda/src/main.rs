//! `reda`: rueda's one command, git style: `reda <subcommand> [options] FILE...`.
//! Every tool of the workspace is a subcommand: the simulators (Veryl, Fluxa, by the
//! files' extension), Veryl synthesis and the FPGA / ASIC flows (eda), the WebAssembly
//! emulator compiler, the RISC-V C compiler and the RV64 board emulator.

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// (name, one-line summary, help text)
const COMMANDS: &[(&str, &str, &str)] = &[
    ("sim", "run the tests (Veryl #[test] testbenches, Fluxa tests)", "\
usage: reda sim [--top MODULE] [--vcd FILE.vcd] [--max-time N] FILE.veryl...
       reda sim [--test NAME]... [--stats] [--trace] FILE.fx...

The language comes from the files' extension: .veryl (Veryl) or .fx (Fluxa).

Simulates the testbenches: every module with #[test(...)] (or --top MODULE; with no
#[test] module, the last module). $assert failures make the exit code 1.
  --top MODULE      simulate this module (alias -s)
  --vcd FILE.vcd    write the waveforms (one file per testbench: FILE_<top>.vcd)
  --max-time N      stop at simulation time N (default 1000000)
Fluxa: runs the program's tests (each task a thread, channels bounded FIFOs; the files are
merged into one program, like iverilog a.v b.v). A failing test makes the exit code 1.
  --test NAME       run only this test (repeatable)
  --stats           per channel: tokens and the largest FIFO occupancy
  --trace           every send / recv
examples: reda sim --vcd wave.vcd counter.veryl     reda sim fluxa/examples/fir.fx"),
    ("list", "list the modules (and which are #[test])", "\
usage: reda list FILE.veryl...|FILE.fx...

Veryl: prints every module, marking the #[test] testbenches. Fluxa: prints the tests."),
    ("synth", "synthesize to a gate netlist (yosys JSON / Verilog)", "\
usage: reda synth [--top MODULE] [-o OUT.json] [--verilog OUT.v] FILE.veryl...

Synthesizes MODULE (default: the last non-#[test] module) into a flat gate-level netlist
of yosys internal cells, optimizes it and prints its statistics.
  --top MODULE      the module to synthesize (alias -s)
  -o OUT.json       yosys JSON (alias --json)
  --verilog OUT.v   structural Verilog
Fluxa (FILE.fx...): synthesizes a task (--top TASK, default the last one) into an FSM and its
datapath; each channel parameter c becomes a valid / ready stream (c_data, c_close, c_valid,
c_ready), every other parameter an input port unless fixed with --param NAME=VALUE. The same
flags then work for fpga, asic and the outputs (e.g. reda fpga --bin fir4.bin fir4.fx).
pipeline(ii = 1) loops start an iteration every clock (else error E0412); a mem used as m[i]
becomes block RAM; a task that spawns a network becomes one netlist (channels: FIFOs).
  --param NAME=VALUE  a parameter or const generic as a constant (Fluxa): an integer, or a
                      Fluxa expression evaluated in the program (an array is a ROM: e.g.
                      --param 'prog=image([addi(1, 0, 5), ecall()])'); a network's array
                      parameters not given are input ports passed on to its instances
  --list              the tasks of a Fluxa program
examples: reda synth -o adder.json adder.veryl     reda synth fluxa/examples/mac.fx"),
    ("fpga", "FPGA flow (iCE40UP5K, ECP5): LUT mapping, place & route, bitstream", "\
usage: reda fpga [--top MODULE] [--pcf PINS.pcf] [--asc OUT.asc] [--bin OUT.bin]
                [--pnr veryl|nextpnr] [--seed N] [--no-timing-driven] [--program PROG.bin]
                [-o OUT.json] [--verilog OUT.v] FILE.veryl...
       reda fpga --arch ecp5 [--device 12k|25k|45k|85k] [--package CABGA381] [--lpf PINS.lpf]
                [--config OUT.config] [--bit OUT.bit] [--seed N] [-o OUT.json] [-v] FILE.veryl...

iCE40 (the default, --arch ice40): maps to SB_LUT4 / SB_CARRY / SB_DFF* / SB_RAM40_4K; with
--asc or --bin, places and routes (nextpnr-ice40, or rueda's own with --pnr veryl) and
writes the bitstream.
  --pcf PINS.pcf          pin constraints (iCEBreaker pins)
  --asc OUT.asc           routed configuration (IceStorm text format)
  --bin OUT.bin           bitstream for iceprog (the own P&R writes it without icepack)
  --pnr veryl|nextpnr     place & route tool (default nextpnr)
  --seed N                placement seed
  --no-timing-driven      wirelength-only placement and routing (own P&R)
  --program PROG.bin      put a program into the block RAMs (e.g. RvBoard, see reda cc)
  --device up5k --package sg48   (the defaults)
ECP5 (--arch ecp5): maps to LUT4 / CCU2C / TRELLIS_FF; with --config or --bit, places and
routes with nextpnr-ecp5 (default --device 85k --package CABGA381: the ULX3S) and packs the
bitstream with ecppack.
  --lpf PINS.lpf          pin constraints (LOCATE COMP / IOBUF PORT)
  --config OUT.config     routed configuration (Trellis text format)
  --bit OUT.bit           bitstream (openFPGALoader / ecpprog)
examples: reda fpga --pnr veryl --pcf icebreaker.pcf --bin blinky.bin blinky.veryl
          reda fpga --arch ecp5 --lpf ulx3s.lpf --bit blinky.bit blinky.veryl"),
    ("asic", "ASIC flow: standard cells, place, route, timing, power (vlib or SKY130)", "\
usage: reda asic [--top MODULE] [--pdk vlib|sky130|sky130:DIR] [--seed N]
                [--verilog OUT.v] [--def OUT.def] [--gds OUT.gds] [--map-only] [-v] FILE.veryl...

Maps to standard cells, places them in rows, builds the clock tree and power grid, routes
(global and detailed), and reports timing, power and (a PDK) the design rule check.
  --pdk vlib              the built-in educational library (default)
  --pdk sky130            SkyWater SKY130 sky130_fd_sc_hd (the cells come with rueda)
  --pdk sky130:DIR        SKY130 from a full PDK directory (see reda pdk)
  --verilog OUT.v         netlist with the library's cell names and cell models
  --def OUT.def           placement and routing (DEF)
  --gds OUT.gds           layout as GDSII (SKY130)
  --map-only              only the technology mapping (a netlist for --verilog)
  -v                      details (critical path, DRC examples)
example: reda asic --pdk sky130 --gds alu.gds --def alu.def alu.veryl"),
    ("gatesim", "run the testbenches on the synthesized netlist", "\
usage: reda gatesim opt|ice40|ecp5|asic|sky130 [--top MODULE] FILE.veryl...|FILE.fx...

Post-synthesis simulation: MODULE (default: the last non-#[test] module) is replaced by its
synthesized netlist (opt: generic gates, ice40: LUTs / block RAM, ecp5: ECP5 LUTs / carries,
asic: vlib cells, sky130: SKY130 cells) and its #[test] testbenches run against it.
Fluxa (FILE.fx...): the program's tests run with TASK (--top) replaced by its synthesized
netlist, its channels connected to the netlist's stream ports; without --top every task that
synthesizes runs as gates.
examples: reda gatesim ice40 fifo.veryl     reda gatesim asic fluxa/examples/mac.fx"),
    ("emu", "compile a Veryl design to WebAssembly (fast cycle-based simulation)", "\
usage: reda emu [--top MODULE] [--stop SIGNAL,...] [-o OUT.wasm] FILE.veryl...

Compiles MODULE (RTL, one clock) into a WebAssembly module OUT.wasm (default emu.wasm) and
OUT.json (the signals' offsets), for a harness in Node or the browser that drives its ports.
  --stop SIGNAL,...  signals whose rising value stops a run
example: reda emu --top Rv64Soc -o rv64.wasm rv64core.veryl rv64soc.veryl"),
    ("cc", "compile C (or assemble .s) for the RISC-V SoC, run it", "\
usage: reda cc PROG.c [-o PROG.bin] [-S PROG.s] [-L PROG.lst] [--run [N]] [--mem BYTES]
       reda cc PROG.c --linux [-o PROG] [-S PROG.s] [-L PROG.lst]

Compiles a C program (a C subset, with rueda's runtime: printf, LED, UART, timer,
interrupts, atomics, VGA text) for RvSoc (examples/src/rvsoc.veryl); a .s / .S file is
assembled. --run runs it on the instruction-set simulator (N instructions, default 10 M),
printing what it sends to the UART.
  -o PROG.bin     the binary (load it with reda fpga --program)
  -S PROG.s       the generated assembly
  -L PROG.lst     listing with addresses and source lines
  --mem BYTES     RAM size (default 8192)
  --linux         a static executable for Linux on the RV64 board (examples/rv64): the same
                  C (int, long, pointers 4 bytes), system calls for files, time and GPIO
example: reda cc --run veryl/examples/c/hello.c"),
    ("pdk", "check a SKY130 PDK directory (Liberty + LEF)", "\
usage: reda pdk [check] [DIR]

Reads sky130_fd_sc_hd__tt_025C_1v80.lib, sky130_fd_sc_hd.tlef and sky130_fd_sc_hd_merged.lef
from DIR (default: $VERYL_EDA_SKY130, /Users/Shared/ccc/sky130hd, ~/sky130hd; tools/sky130.sh
downloads them) and prints the layers and the cell bindings the ASIC flow uses."),
    ("rvemu", "run a RISC-V program on the RV64 board emulator (OpenSBI, Linux)", "\
usage: reda rvemu [-t] [-r] [-m N] [-l ADDR:FILE]... [-d ADDR:LEN:FILE]... [-s SCRIPT] PROG

Runs PROG (an ELF; otherwise a binary at 0x8000_0000) on the instruction-set emulator of the
Rv64Soc board (RV64IMAC, M/S/U, SV39, UART, GPIO); UART output to stdout, keys to the UART.
  -t              a line per instruction (spike -l --log-commits format)
  -m N            at most N instructions (default 10 G; 0 = no limit)
  -l ADDR:FILE    another file into the RAM
  -d ADDR:LEN:FILE  the RAM from ADDR into FILE at the end
  -s SCRIPT       type the script's lines at each shell prompt
  -r              real time (the default at a terminal)
exit code: 0 pass, 1 fail, 2 out of instructions"),
    ("version", "print the version", "usage: reda version"),
    ("help", "help on a subcommand", "usage: reda help [SUBCOMMAND]"),
];

fn help(cmd: Option<&str>) -> ! {
    match cmd.and_then(|c| COMMANDS.iter().find(|x| x.0 == c)) {
        Some((_, _, text)) => println!("{}", text),
        None => {
            if let Some(c) = cmd { eprintln!("reda: no subcommand '{}'\n", c); }
            println!("reda {} -- rueda: HDL to chip (Veryl, Fluxa; simulation, synthesis, FPGA, ASIC, RISC-V)\n", VERSION);
            println!("usage: reda <subcommand> [options] FILE...\n");
            for (n, s, _) in COMMANDS { println!("  {:9} {}", n, s); }
            println!("\n'reda help <subcommand>' (or 'reda <subcommand> -h') shows its options.");
            println!("examples: reda sim counter.veryl     reda sim fir.fx     reda fpga --pnr veryl --pcf pins.pcf --bin top.bin top.veryl");
            if cmd.is_some() { std::process::exit(2); }
        }
    }
    std::process::exit(0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(|s| s.as_str()) else { help(None) };
    let rest: Vec<String> = args[1..].to_vec();
    if rest.iter().any(|a| a == "-h" || a == "--help") { help(Some(cmd)); }
    // argument errors in the tools point back to `reda help <subcommand>`
    let name: &'static str = COMMANDS.iter().find(|x| x.0 == cmd).map(|x| x.0).unwrap_or("help");
    let _ = veryl_sim::cli::CMD.set(name);
    let _ = veryl_synth::cli::CMD.set(name);
    let _ = fluxa_synth::cli::CMD.set(name);
    let with = |flag: &[&str]| -> Vec<String> { flag.iter().map(|s| s.to_string()).chain(rest.iter().cloned()).collect() };
    match cmd {
        "sim" | "test" => match lang(&rest) {
            Lang::Veryl => veryl_sim::cli::main(&rest),
            Lang::Fluxa => fluxa::cli::main(&rest),
        },
        "list" => match lang(&rest) {
            Lang::Veryl => veryl_sim::cli::main(&with(&["--list"])),
            Lang::Fluxa => fluxa::cli::main(&with(&["--list"])),
        },
        "synth" => synth(&rest, rest.clone()),
        "fpga" => {
            // --arch ice40 (default) | ecp5
            let mut arch = "ice40".to_string();
            let mut a = vec![];
            let mut it = rest.iter();
            while let Some(x) = it.next() {
                if x == "--arch" { arch = it.next().cloned().unwrap_or_default(); } else { a.push(x.clone()); }
            }
            let flag = match arch.as_str() { "ice40" => "--ice40", "ecp5" => "--ecp5", o => { eprintln!("reda fpga: --arch is ice40 or ecp5, not '{}'", o); std::process::exit(2) } };
            a.insert(0, flag.to_string());
            synth(&rest, a)
        }
        "asic" => synth(&rest, with(&["--asic"])),
        "gatesim" => {
            let Some(kind) = rest.first() else { help(Some("gatesim")) };
            if !matches!(kind.as_str(), "opt" | "ice40" | "ecp5" | "asic" | "sky130") { eprintln!("reda gatesim: the netlist is opt, ice40, ecp5, asic or sky130, not '{}'", kind); std::process::exit(2); }
            let mut a = vec!["--gatesim".to_string(), kind.clone()];
            a.extend(rest[1..].iter().cloned());
            synth(&rest, a)
        }
        "emu" => veryl_emu::cli::main(&rest),
        "cc" => rvtools::cli::cc(&rest),
        "rvemu" => riscv_emu::cli::main(&rest),
        "pdk" => {
            let dir: Vec<String> = rest.iter().filter(|a| *a != "check").cloned().collect();
            eda::cli::pdk_check(dir.first())
        }
        "version" | "--version" | "-V" => println!("reda {}", VERSION),
        "help" | "-h" | "--help" => help(rest.first().map(|s| s.as_str())),
        c => help(Some(c)),
    }
}

/// the source language of a command's files, from their extension
#[derive(PartialEq)]
enum Lang { Veryl, Fluxa }

fn lang(args: &[String]) -> Lang {
    let fx = args.iter().filter(|a| a.ends_with(".fx")).count();
    let veryl = args.iter().filter(|a| a.ends_with(".veryl")).count();
    if fx > 0 && veryl > 0 { eprintln!("reda: Veryl (.veryl) and Fluxa (.fx) files cannot be mixed"); std::process::exit(2); }
    if fx > 0 { Lang::Fluxa } else { Lang::Veryl }
}

/// synthesis and the flows after it, by the language of the files (`args`: as given, for the
/// language; `flow`: the arguments for the front end)
fn synth(args: &[String], flow: Vec<String>) {
    match lang(args) {
        Lang::Veryl => veryl_synth::cli::main(flow),
        Lang::Fluxa => fluxa_synth::cli::main(flow),
    }
}
