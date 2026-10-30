//! Fluxa 行為模擬器（Kahn 程序網路直譯器）。
//!
//! 類比 `iverilog` + `vvp`：`load` 負責解析與編譯期常數求值，
//! `run_test` 為每個 `test` 區塊建立獨立的模擬世界並執行。

pub mod ast;
pub mod cli;
pub mod interp;
pub mod lexer;
pub mod parser;
pub mod runtime;
pub mod value;

use ast::{Program, TestDecl};
use interp::Interp;
use runtime::{block_on, RtErr, Shared};
use std::sync::Arc;
use value::coerce;

pub struct Options {
    pub trace: bool,
    pub stats: bool,
    /// 協作式排程：所有任務在同一條執行緒上輪流執行，`print` / `--trace` 的輸出收進
    /// `TestResult::output`（WebAssembly 一律如此，因為沒有執行緒）
    pub coop: bool,
}

pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
    pub secs: f64,
    pub stats: String,
    /// 協作式排程時 `print` / `--trace` 的輸出（原生排程直接印出，這裡是空的）
    pub output: String,
}

/// 解析單一檔案並求值編譯期常數
pub fn load(file: &str, src: &str) -> Result<Arc<Program>, String> {
    load_many(&[(file.to_string(), src.to_string())])
}

/// 解析多個檔案（依序），合併後求值編譯期常數。類比 `iverilog a.v b.v`。
pub fn load_many(files: &[(String, String)]) -> Result<Arc<Program>, String> {
    let mut prog: Option<Program> = None;
    for (idx, (file, src)) in files.iter().enumerate() {
        let p = parser::parse_program_at(file, src, idx as u32).map_err(|e| {
            format!("{}:{}: syntax error: {}", file, e.line % ast::FILE_STRIDE, e.msg)
        })?;
        match prog.as_mut() {
            None => prog = Some(p),
            Some(acc) => acc.merge(p).map_err(|m| format!("{}: {}", file, m))?,
        }
    }
    let prog = Arc::new(prog.ok_or_else(|| "no input files".to_string())?);
    let sh = Shared::new(prog.clone(), false, false);
    let mut it = Interp::new(sh, 0, "const".to_string());
    for c in &prog.consts_decl {
        let fail = |line: u32, m: String| format!("{}: in const `{}`: {}", prog.loc(line), c.name, m);
        it.line = c.line;
        let mut v = block_on(it.eval(&c.value)).map_err(|e| match e {
            RtErr::Msg(l, m) => fail(l, m),
            other => fail(c.line, format!("{:?}", other)),
        })?;
        if let Some(t) = &c.ty {
            let rt = block_on(it.eval_type(t)).map_err(|e| match e {
                RtErr::Msg(l, m) => fail(l, m),
                other => fail(c.line, format!("{:?}", other)),
            })?;
            v = coerce(v, &rt).map_err(|m| fail(c.line, m))?;
        }
        prog.consts.write().unwrap().insert(c.name.clone(), v);
    }
    Ok(prog)
}

pub fn run_test(prog: &Arc<Program>, t: &TestDecl, opts: &Options) -> TestResult {
    let coop = opts.coop || cfg!(target_arch = "wasm32");
    let sh = Shared::new(prog.clone(), opts.trace, coop);
    let mut it = Interp::new(sh.clone(), 0, format!("test:{}", t.name));
    let start = Clock::now();
    let r = if coop { sh.run_coop(Box::pin(it.exec_block(&t.body))) } else { block_on(it.exec_block(&t.body)) };
    sh.finish_test();
    sh.join_all();
    let secs = start.secs();
    let (child_err, deadlock) = {
        let w = sh.lock();
        (w.error.clone(), w.deadlock_report.clone())
    };
    let failure: Option<String> = match r {
        Ok(_) | Err(RtErr::Ret(_)) => child_err,
        Err(RtErr::Msg(l, m)) => Some(format!("{}: {}", prog.loc(l), m)),
        Err(RtErr::Deadlock) => deadlock,
        Err(RtErr::Shutdown) => child_err.or(deadlock),
    };
    TestResult {
        name: t.name.clone(),
        passed: failure.is_none(),
        message: failure.unwrap_or_default(),
        secs,
        stats: if opts.stats { sh.stats_report() } else { String::new() },
        output: sh.take_output(),
    }
}

/// 測試的執行時間（WebAssembly 沒有時鐘：0）
struct Clock(#[cfg(not(target_arch = "wasm32"))] std::time::Instant);

impl Clock {
    fn now() -> Clock {
        Clock(
            #[cfg(not(target_arch = "wasm32"))]
            std::time::Instant::now(),
        )
    }
    fn secs(&self) -> f64 {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.elapsed().as_secs_f64();
        #[cfg(target_arch = "wasm32")]
        0.0
    }
}
