//! The RTL backend's expressions: they are compiled and run (iverilog), and the values must be
//! the ones the simulator computes. The test is skipped when iverilog is not installed.

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
