use fluxa::{load_many, run_test, Options};

/// 多檔載入（同命令列 `fluxa a.fx b.fx`），路徑相對於專案根目錄
fn run_files(files: &[&str]) -> Vec<(String, bool, String)> {
    let srcs: Vec<(String, String)> = files
        .iter()
        .map(|f| {
            let path = format!("{}/../{}", env!("CARGO_MANIFEST_DIR"), f);
            let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {}", path, e));
            (path, src)
        })
        .collect();
    let prog = load_many(&srcs).unwrap_or_else(|e| panic!("{}", e));
    let run = |coop: bool| -> Vec<(String, bool, String)> {
        let opts = Options { coop, ..Default::default() };
        prog.tests
            .iter()
            .map(|t| {
                let r = run_test(&prog, t, &opts);
                (r.name, r.passed, r.message)
            })
            .collect()
    };
    let threads = run(false);
    // the cooperative scheduler (the browser's) gives the same results: KPN determinism
    let coop = run(true);
    for ((name, ok, msg), (_, ok2, msg2)) in threads.iter().zip(&coop) {
        assert_eq!(ok, ok2, "{}: threads {} / coop {}: {} / {}", name, ok, ok2, msg, msg2);
        assert_eq!(msg.lines().next(), msg2.lines().next(), "{}: threads / coop messages differ", name);
    }
    threads
}

fn run(file: &str) -> Vec<(String, bool, String)> {
    run_files(&[&format!("examples/{}.fx", file)])
}

fn check_all_pass(label: &str, rs: Vec<(String, bool, String)>) {
    assert!(!rs.is_empty(), "{}: no tests", label);
    for (name, ok, msg) in rs {
        assert!(ok, "{}::{} failed: {}", label, name, msg);
    }
}

fn check_all_fail_with(label: &str, rs: Vec<(String, bool, String)>, needle: &str) {
    assert!(!rs.is_empty(), "{}: no tests", label);
    for (name, ok, msg) in rs {
        assert!(!ok, "{}::{} should have failed", label, name);
        assert!(msg.contains(needle), "{}::{}: expected `{}` in `{}`", label, name, needle, msg);
    }
}

fn all_pass(file: &str) { check_all_pass(file, run(file)); }
fn all_fail_with(file: &str, needle: &str) { check_all_fail_with(file, run(file), needle); }

fn rv32i(file: &str) -> Vec<(String, bool, String)> {
    run_files(&["rv32i/rv32i.fx", &format!("rv32i/{}.fx", file)])
}

fn rv64(file: &str) -> Vec<(String, bool, String)> {
    run_files(&["rv64/rv64.fx", "rv64/fpu.fx", &format!("rv64/{}.fx", file)])
}

#[test] fn basics() { all_pass("basics"); }
#[test] fn mac() { all_pass("mac"); }
#[test] fn fir() { all_pass("fir"); }
#[test] fn gemm() { all_pass("gemm"); }
#[test] fn fanout() { all_pass("fanout"); }

#[test] fn overflow_is_reported() { all_fail_with("err_overflow", "does not fit i24"); }
#[test] fn deadlock_is_reported() { all_fail_with("err_deadlock", "deadlock"); }
#[test] fn linearity_is_reported() { all_fail_with("err_linear", "does not own the rx endpoint"); }
#[test] fn narrowing_is_reported() { all_fail_with("err_narrow", "does not fit u8"); }

/// 死結偵測不可有競態誤判：重複執行多次都必須通過
#[test]
fn no_false_deadlocks_under_repetition() {
    for _ in 0..100 {
        all_pass("gemm");
        all_pass("fanout");
    }
}

// ---- RV32I 處理器範例（rv32i/）----

#[test] fn rv32i_smoke() { check_all_pass("rv32i/smoke", rv32i("smoke")); }
#[test] fn rv32i_tests_basic() { check_all_pass("rv32i/tests_basic", rv32i("tests_basic")); }
#[test] fn rv32i_wb_depth_deadlock() { check_all_fail_with("rv32i/err_wb_depth", rv32i("err_wb_depth"), "deadlock"); }
#[test] fn rv32i_bad_immediate() { check_all_fail_with("rv32i/err_bad_immediate", rv32i("err_bad_immediate"), "does not fit i12"); }

// ---- RV64G 處理器範例（rv64/）----

#[test] fn rv64_smoke() { check_all_pass("rv64/smoke", rv64("smoke")); }
#[test] fn rv64_tests_basic() { check_all_pass("rv64/tests_basic", rv64("tests_basic")); }
#[test] fn rv64_tests_mmu() { check_all_pass("rv64/tests_mmu", rv64("tests_mmu")); }
#[test] fn rv64_tests_amo() { check_all_pass("rv64/tests_amo", rv64("tests_amo")); }
#[test] fn rv64_tests_fpu() { check_all_pass("rv64/tests_fpu", rv64("tests_fpu")); }
#[test] fn rv64_tests_fp() { check_all_pass("rv64/tests_fp", rv64("tests_fp")); }
#[test] fn rv64_tests_irq() { check_all_pass("rv64/tests_irq", rv64("tests_irq")); }
#[test] fn rv64_tests_system() { check_all_pass("rv64/tests_system", rv64("tests_system")); }
