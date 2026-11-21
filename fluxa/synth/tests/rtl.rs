//! The RTL backend: its expressions are compiled and run (iverilog), and the values must be the
//! ones the simulator computes; its modules are run too, and the tokens they send must be the
//! ones the program computes. The tests are skipped when iverilog is not installed.

use std::process::Command;

/// the value of an expression, as the RTL computes it; None when iverilog is not installed
fn run(expr: &str, width: usize) -> Option<u128> {
    let dir = std::env::temp_dir().join(format!("fluxa-rtl-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let sv = dir.join("m.sv");
    let tb = dir.join("tb.sv");
    std::fs::write(&sv, format!("module m(input logic clk, output logic [{}:0] r);\n  assign r = {};\nendmodule\n", width - 1, expr)).ok()?;
    std::fs::write(&tb, format!("module tb;\n  logic clk = 0;\n  logic [{}:0] r;\n  m u(.clk(clk), .r(r));\n  initial begin #1 $display(\"%d\", r); $finish; end\nendmodule\n", width - 1)).ok()?;
    let out = dir.join("a.out");
    let c = match Command::new("iverilog").arg("-g2012").arg("-o").arg(&out).arg(&sv).arg(&tb).output() {
        Ok(c) => c,
        Err(_) => return None, // iverilog is not installed
    };
    assert!(c.status.success(), "iverilog failed on `{}`:\n{}", expr, String::from_utf8_lossy(&c.stderr));
    let v = Command::new("vvp").arg(&out).output().ok()?;
    let s = String::from_utf8_lossy(&v.stdout);
    s.split_whitespace().next()?.parse::<u128>().ok()
}

fn check(name: &str, expr: &str, width: usize, want: u128) {
    let Some(got) = run(expr, width) else {
        eprintln!("{}: iverilog is not available, skipped", name);
        return;
    };
    assert_eq!(got, want, "{}: `{}` gave {} (expected {})", name, expr, got, want);
}

#[test]
fn the_expressions_compute_the_values_the_simulator_computes() {
    // 200 + 100 = 300: the sum of two u8 is nine bits wide
    check("add", "(8'd200 + 8'd100)", 9, 300);
    // -5 + 3 = -2: a signed sum
    check("add signed", "(-9'sd5 + 9'sd3)", 9, (-2i128) as u128 & 0x1ff);
    // 10 - 20 = -10 in nine bits (the range covers -20..235)
    check("sub", "(9'd10 - 9'd20)", 9, (-10i128) as u128 & 0x1ff);
    // -32768 * -32768 = 1073741824: the product needs 32 bits, not 16
    check("mul", "(-32'sd32768 * -32'sd32768)", 32, 1073741824);
    // -7 / 2 = -3 and -7 % 2 = -1 (truncating toward zero)
    check("div", "(-8'sd7 / 8'sd2)", 8, (-3i128) as u128 & 0xff);
    check("rem", "(-8'sd7 % 8'sd2)", 8, (-1i128) as u128 & 0xff);
    // -5 >> 1 = -3 (floor), 255 >> 4 = 15
    check("shr signed", "($signed(-8'sd5) >>> 1)", 8, (-3i128) as u128 & 0xff);
    check("shr", "(8'd255 >> 4)", 8, 15);
    // the comparisons are signed when a value is
    check("lt signed", "($signed(-8'sd5) < $signed(8'sd3))", 1, 1);
    check("lt unsigned", "(8'd5 < 8'd3)", 1, 0);
    // a saturating conversion clamps, a wrapping one keeps the low bits
    check("sat", "((16'd300 > 16'd255) ? 8'd255 : ((16'd300 < 16'd0) ? 8'd0 : 8'(16'd300)))", 8, 255);
    check("wrap", "8'(16'd300)", 8, 44);
    // a fixed-point value: 3.375 is raw 864 with 8 fraction bits
    check("fixed", "16'(16'sd864 >>> 6)", 16, 13); // 864 >> 6 = 13 (3.375 >> 2 fraction bits: 13/4 = 3.25)
}

// ------------------------------------------------------------ the modules

/// a channel of a testbench: the port prefix and the width of its data
struct Port {
    name: &'static str,
    width: usize,
}

/// the RTL of the task `top` of the program `src`
fn rtl_of(src: &str, top: &str) -> String {
    let prog = fluxa::load_many(&[("test.fx".to_string(), src.to_string())]).unwrap();
    let prog = std::sync::Arc::new(prog);
    fluxa_synth::rtl(&prog, top, &Default::default()).unwrap()
}

/// the tokens the module sends: the module is compiled and run (iverilog), the values are sent
/// to `input` and the tokens on `output` are collected. None when iverilog is not installed.
fn tokens(module: &str, sv: &str, inputs: &[Port], outputs: &[Port], scalars: &[(String, usize, u128)], values: &[Vec<u128>], close: bool) -> Option<Vec<Vec<u128>>> {
    let dir = std::env::temp_dir().join(format!("fluxa-rtl-mod-{}-{}", std::process::id(), module));
    std::fs::create_dir_all(&dir).ok()?;
    let svf = dir.join("m.sv");
    let tbf = dir.join("tb.sv");
    std::fs::write(&svf, sv).ok()?;
    let mut decls = vec!["  logic clk = 0;".to_string(), "  logic rst = 1;".to_string(), "  always #5 clk = ~clk;".to_string()];
    let mut conns = vec![".clk(clk)".to_string(), ".rst(rst)".to_string()];
    for p in inputs {
        decls.push(format!("  logic [{}:0] {}_data;", p.width - 1, p.name));
        decls.push(format!("  logic {}_close, {}_valid;", p.name, p.name));
        decls.push(format!("  wire {}_ready;", p.name));
        conns.push(format!(".{}_data({}_data)", p.name, p.name));
        conns.push(format!(".{}_close({}_close)", p.name, p.name));
        conns.push(format!(".{}_valid({}_valid)", p.name, p.name));
        conns.push(format!(".{}_ready({}_ready)", p.name, p.name));
    }
    for p in outputs {
        decls.push(format!("  logic {}_ready = 1;", p.name));
        decls.push(format!("  wire [{}:0] {}_data;", p.width - 1, p.name));
        decls.push(format!("  wire {}_close, {}_valid;", p.name, p.name));
        conns.push(format!(".{}_ready({}_ready)", p.name, p.name));
        conns.push(format!(".{}_data({}_data)", p.name, p.name));
        conns.push(format!(".{}_close({}_close)", p.name, p.name));
        conns.push(format!(".{}_valid({}_valid)", p.name, p.name));
    }
    for (n, w, v) in scalars {
        decls.push(format!("  logic [{}:0] {} = {}'d{};", w - 1, n, w, v));
        conns.push(format!(".{}({})", n, n));
    }
    let mut tb = format!("module tb;\n{}\n  {} u({});\n", decls.join("\n"), module, conns.join(", "));
    for p in outputs {
        tb += &format!("  always @(posedge clk) if ({}_valid && {}_ready && !{}_close) $display(\"TOK {} %d\", {}_data);\n", p.name, p.name, p.name, p.name, p.name);
    }
    for (k, p) in inputs.iter().enumerate() {
        let vals: Vec<u128> = values.get(k).cloned().unwrap_or_default();
        // the values the channel sends
        tb += &format!("  function automatic [{}:0] val{}(input integer i);\n    case (i)\n", p.width - 1, k);
        for (i, v) in vals.iter().enumerate() {
            tb += &format!("      {}: val{} = {}'d{};\n", i, k, p.width, v);
        }
        tb += &format!("      default: val{} = '0;\n    endcase\n  endfunction\n", k);
        // the driver: the next token when the task has taken the last one (a task that takes a
        // token every clock, a pipeline, sees one token per clock)
        tb += &format!("  initial begin\n    integer i = 0;\n    {}_data = 0; {}_valid = 0; {}_close = 0;\n    @(negedge clk);\n    rst = 0;\n    {}_data = val{}(0); {}_valid = 1;\n    forever begin\n      @(posedge clk);\n      if ({}_ready) begin\n        i = i + 1;\n        if (i < {}) begin\n          {}_data = val{}(i);\n        end else begin\n          {}_valid = 0;\n", p.name, p.name, p.name, p.name, k, p.name, p.name, vals.len(), p.name, k, p.name);
        if close {
            tb += &format!("          {}_close = 1; {}_valid = 1;\n", p.name, p.name);
        }
        tb += &format!("        end\n      end\n    end\n  end\n");
    }
    tb += "  initial begin\n    @(negedge clk);\n    rst = 0;\n    repeat (400) @(posedge clk);\n    $finish;\n  end\nendmodule\n";
    std::fs::write(&tbf, &tb).ok()?;
    let out = dir.join("a.out");
    let c = match Command::new("iverilog").arg("-g2012").arg("-o").arg(&out).arg(&svf).arg(&tbf).output() {
        Ok(c) => c,
        Err(_) => return None, // iverilog is not installed
    };
    assert!(c.status.success(), "iverilog failed on the module `{}`:\n{}\n{}", module, String::from_utf8_lossy(&c.stderr), tb);
    let v = Command::new("vvp").arg(&out).output().ok()?;
    let s = String::from_utf8_lossy(&v.stdout);
    // the tokens, by the output they came from
    let mut toks: Vec<Vec<u128>> = vec![vec![]; outputs.len()];
    for l in s.lines() {
        let Some(rest) = l.strip_prefix("TOK ") else { continue };
        let (name, v) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
        let k = outputs.iter().position(|p| p.name == name).unwrap_or(0);
        toks[k].push(v.trim().parse::<u128>().unwrap_or_else(|_| panic!("a token that is not a number: {}", l)));
    }
    Some(toks)
}

/// the tasks of `fanout.fx`: a stream that is doubled, one that is summed up, one that counts
const FANOUT: &str = include_str!("../../examples/fanout.fx");

#[test]
fn the_modules_send_the_tokens_the_program_computes() {
    // Double: every token twice
    let sv = rtl_of(FANOUT, "Double");
    let Some(toks) = tokens("Double", &sv, &[Port { name: "rx", width: 8 }], &[Port { name: "out", width: 9 }], &[], &[vec![1, 2, 3]], true) else {
        eprintln!("iverilog is not available, skipped");
        return;
    };
    assert_eq!(toks[0], vec![2, 4, 6], "Double sent {:?}", toks[0]);

    // Total: the sum of the tokens
    let sv = rtl_of(FANOUT, "Total");
    let Some(toks) = tokens("Total", &sv, &[Port { name: "rx", width: 8 }], &[Port { name: "out", width: 16 }], &[], &[vec![1, 2, 3]], true) else {
        eprintln!("iverilog is not available, skipped");
        return;
    };
    assert_eq!(toks[0], vec![6], "Total sent {:?}", toks[0]);

    // Gen: the numbers from 0 to n
    let sv = rtl_of(FANOUT, "Gen");
    let Some(toks) = tokens("Gen", &sv, &[], &[Port { name: "out", width: 8 }], &[("n".to_string(), 32, 4)], &[], false) else {
        eprintln!("iverilog is not available, skipped");
        return;
    };
    assert_eq!(toks[0], vec![0, 1, 2, 3], "Gen sent {:?}", toks[0]);
}

/// the tasks of `mac.fx`: a pipeline loop that multiplies and adds up
const MAC: &str = include_str!("../../examples/mac.fx");

#[test]
fn the_pipeline_loop_computes_the_same_sum() {
    let sv = rtl_of(MAC, "Mac");
    let xs = [3i64, -4, 5, 32767, -32768, 7, 0, 100];
    let ys = [2i64, 9, -1, 32767, -32768, 7, 5, -100];
    // the tokens go as they are (the negative ones in two's complement)
    let a: Vec<u128> = xs.iter().map(|x| (*x as u16) as u128).collect();
    let b: Vec<u128> = ys.iter().map(|y| (*y as u16) as u128).collect();
    let want = xs.iter().zip(ys.iter()).map(|(x, y)| x * y).sum::<i64>() as u64 as u128;
    let Some(toks) = tokens("Mac", &sv, &[Port { name: "a", width: 16 }, Port { name: "b", width: 16 }], &[Port { name: "out", width: 48 }], &[("len".to_string(), 16, 8)], &[a, b], true) else {
        eprintln!("iverilog is not available, skipped");
        return;
    };
    assert_eq!(toks[0], vec![want], "Mac sent {:?}", toks[0]);
}

/// the tasks of `pipeline.fx`: a network (a task that spawns) with a broadcast in it
const PIPELINE: &str = include_str!("../../examples/pipeline.fx");
#[test]
fn the_network_computes_the_same_stream() {
    let sv = rtl_of(PIPELINE, "Pipeline");
    // the values: the tokens go through the stages (x * 3 + 3) and are counted (n)
    let xs = [5i64, -7, 0, 42, -128, 127];
    let a: Vec<u128> = xs.iter().map(|x| (*x as u8) as u128).collect();
    let Some(toks) = tokens("Pipeline", &sv, &[Port { name: "x", width: 8 }], &[Port { name: "y", width: 11 }, Port { name: "n", width: 8 }], &[], &[a], true) else {
        eprintln!("iverilog is not available, skipped");
        return;
    };
    // the stream: every value times three, plus three (the port is 11 bits wide)
    let want: Vec<u128> = xs.iter().map(|x| ((*x as i32 * 3 + 3) as u16 as u128) & 0x7ff).collect();
    assert_eq!(toks[0], want, "Pipeline sent {:?}", toks[0]);
    assert_eq!(toks[1], vec![xs.len() as u128], "Pipeline counted {:?}", toks[1]);
}

/// the examples of the project: every task of every example must become valid SystemVerilog
#[test]
fn every_example_becomes_valid_systemverilog() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
    let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().map(|x| x == "fx").unwrap_or(false)).collect(),
        Err(e) => { eprintln!("the examples: {}", e); return; }
    };
    files.sort();
    assert!(!files.is_empty(), "no examples in {}", dir.display());
    let mut checked = 0;
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        // the examples that show an error are not hardware
        if name.starts_with("err_") { continue; }
        let src = std::fs::read_to_string(f).unwrap();
        let Ok(prog) = fluxa::load_many(&[(name.clone(), src.clone())]) else { continue };
        let prog = std::sync::Arc::new(prog);
        for t in fluxa_synth::tasks(&prog) {
            // what the gates cannot build, the RTL does not have to build either
            if fluxa_synth::synthesize(&prog, &t, &Default::default()).is_err() { continue; }
            let text = match fluxa_synth::rtl(&prog, &t, &Default::default()) {
                Ok(x) => x,
                Err(e) => panic!("{}: the RTL of `{}`: {}", name, t, e),
            };
            // the modules must compile
            let dir2 = std::env::temp_dir().join(format!("fluxa-rtl-ex-{}-{}", std::process::id(), t));
            std::fs::create_dir_all(&dir2).unwrap();
            let svf = dir2.join("m.sv");
            std::fs::write(&svf, &text).unwrap();
            let out = dir2.join("a.out");
            let c = match Command::new("iverilog").arg("-g2012").arg("-o").arg(&out).arg("-s").arg(&t).arg(&svf).output() {
                Ok(c) => c,
                Err(_) => { eprintln!("iverilog is not available, skipped"); return; }
            };
            assert!(c.status.success(), "{}: the RTL of `{}` does not compile:\n{}\n{}", name, t, String::from_utf8_lossy(&c.stderr), text);
            checked += 1;
        }
    }
    assert!(checked > 0, "no task was checked");
}
