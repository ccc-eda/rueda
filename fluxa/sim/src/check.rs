//! 靜態檢查（`reda check x.fx`）：在模擬與合成之前，以區間分析找出規格 §3 的錯誤。
//!
//! - E0301：值的範圍裝不進落點的型別（`let x: T = e`、賦值、`send`、函式引數與回傳），以及
//!   `carry` 累加器依迴圈次數上界的終值範圍（T-CARRY，§3.3）；訊息給出需要的最小型別。
//! - E0102：除數的範圍包含 0（T-DIV）。
//! - E0703（暫定）：索引的範圍超出陣列。
//!
//! 分析是保守的過度近似：範圍未知（泛型、浮點、定點數、未支援的運算）就不檢查，所以不會因為
//! 「不知道」而報錯；報出來的是「無法證明裝得下」，不一定每次執行都會發生。條件式
//! `if x < c` 會收窄 `x` 的範圍。

use crate::ast::*;
use crate::interp::Interp;
use crate::runtime::{block_on, Shared};
use crate::value::{int_bits, RtType, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// 跨函式分析的深度上限
const CALL_DEPTH: usize = 4;

/// 一筆檢查結果
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub code: &'static str,
    /// 全域行號（`Program::file_line` 還原）
    pub line: u32,
    /// 所在的函式或任務
    pub func: String,
    pub msg: String,
    /// 證據：(名稱，值)，例如 ("range", "0..=300")
    pub data: Vec<(String, String)>,
    /// 可行的修正
    pub fix: Option<String>,
    /// E0301：裝得下的最小型別
    pub min_type: Option<String>,
}

/// 檢查程式裡所有的 `fn`、`comb fn`、`task` 與 `test`（`comptime` 不檢查）；依行號排序
pub fn check(prog: &Arc<Program>) -> Vec<Finding> {
    let mut out = vec![];
    let cache = Rc::new(RefCell::new(HashMap::new()));
    let mut fns: Vec<&Arc<FnDecl>> = prog.fns.values().filter(|d| d.kind != FnKind::Comptime).collect();
    fns.sort_by_key(|d| d.line);
    for d in fns {
        let mut ck = Ck::new(prog, &d.name, cache.clone());
        ck.func_body(d);
        out.extend(ck.out);
    }
    for t in &prog.tests {
        let mut ck = Ck::new(prog, &format!("test {}", t.name), cache.clone());
        collect_assigned(&t.body, &mut ck.assigned);
        ck.block(&t.body);
        ck.check_carries();
        out.extend(ck.out);
    }
    out.sort_by(|a, b| (a.line, a.code, &a.msg).cmp(&(b.line, b.code, &b.msg)));
    out.dedup();
    out
}

/// 抽象值
#[derive(Clone, Debug, PartialEq)]
enum AV {
    Top,
    Bool,
    Int(i128, i128),
    Arr(Box<AV>, Option<usize>),
    Tup(Vec<AV>),
    Chan(Box<AV>),
}

impl AV {
    fn of_ty(t: &RtType) -> AV {
        match t {
            RtType::Int { lo, hi } => AV::Int(*lo, *hi),
            RtType::Bool => AV::Bool,
            RtType::Array(e, n) | RtType::Mem(e, n) => AV::Arr(Box::new(AV::of_ty(e)), Some(*n)),
            RtType::Tuple(ts) => AV::Tup(ts.iter().map(AV::of_ty).collect()),
            RtType::Chan(e, _) => AV::Chan(Box::new(AV::of_ty(e))),
            RtType::Fixed { .. } | RtType::Any => AV::Top,
        }
    }
    fn of_value(v: &Value) -> AV {
        match v {
            Value::Int(n) => AV::Int(*n, *n),
            Value::Bool(_) => AV::Bool,
            Value::Array(xs) => {
                let e = xs.iter().map(AV::of_value).reduce(|a, b| a.join(&b)).unwrap_or(AV::Top);
                AV::Arr(Box::new(e), Some(xs.len()))
            }
            Value::Tuple(xs) => AV::Tup(xs.iter().map(AV::of_value).collect()),
            _ => AV::Top,
        }
    }
    /// 兩個可能值的聯集
    fn join(&self, o: &AV) -> AV {
        match (self, o) {
            (AV::Int(a, b), AV::Int(c, d)) => AV::Int(*a.min(c), *b.max(d)),
            (AV::Bool, AV::Bool) => AV::Bool,
            (AV::Arr(a, n), AV::Arr(b, m)) if n == m => AV::Arr(Box::new(a.join(b)), *n),
            (AV::Tup(a), AV::Tup(b)) if a.len() == b.len() => AV::Tup(a.iter().zip(b).map(|(x, y)| x.join(y)).collect()),
            _ => AV::Top,
        }
    }
    fn int(&self) -> Option<(i128, i128)> {
        if let AV::Int(a, b) = self { Some((*a, *b)) } else { None }
    }
}

/// 型別的範圍（只看整數）
fn int_range(t: &RtType) -> Option<(i128, i128)> {
    if let RtType::Int { lo, hi } = t { Some((*lo, *hi)) } else { None }
}

/// 型別的名稱：`u8`、`i24`，其他範圍寫成 `int<lo..=hi>`
pub fn type_name(lo: i128, hi: i128) -> String {
    let w = int_bits(lo, hi);
    if lo == 0 && hi == (1i128 << w) - 1 { format!("u{}", w) }
    else if w >= 1 && lo == -(1i128 << (w - 1)) && hi == (1i128 << (w - 1)) - 1 { format!("i{}", w) }
    else { format!("int<{}..={}>", lo, hi) }
}

/// 裝得下 `[lo, hi]` 的最小 `uN` / `iN`
pub fn min_type(lo: i128, hi: i128) -> String {
    let w = int_bits(lo, hi);
    format!("{}{}", if lo < 0 { "i" } else { "u" }, w)
}

fn rng(lo: i128, hi: i128) -> String { format!("{}..={}", lo, hi) }

#[derive(Clone)]
struct Var {
    /// 宣告的型別（落點）
    decl: Option<RtType>,
    val: AV,
    /// `carry` 變數：`Ck::carries` 的索引
    carry: Option<usize>,
    /// 已知不為 0（`if x != 0`、`if x == 0 { … } else { 這裡 }`）
    nonzero: bool,
}

struct Carry {
    name: String,
    ty: (i128, i128),
    init: Option<(i128, i128)>,
    /// 宣告時外層迴圈的層數：只有更內層的迴圈會重複累加
    depth: usize,
    /// 每個累加點：(迴圈次數上界，None = 無界)、每次的增量範圍、行號
    deltas: Vec<(Option<i128>, i128, i128, u32)>,
    line: u32,
    /// "carry" 或 "variable"（`let mut x: T`）
    kind: &'static str,
}

struct Ck<'a> {
    prog: &'a Arc<Program>,
    it: Interp,
    func: String,
    scopes: Vec<HashMap<String, Var>>,
    /// 函式裡被賦值過的名稱：它們的值取宣告的型別（或未知），不用初值
    assigned: HashSet<String>,
    /// 外層迴圈的次數上界（None = 無界，例如 `for v in rx`）
    loops: Vec<Option<i128>>,
    carries: Vec<Carry>,
    ret: Option<RtType>,
    out: Vec<Finding>,
    /// 跨函式分析：呼叫的深度；> 0 時不回報（被呼叫的函式自己會被檢查）
    depth: usize,
    /// (函式, 引數的範圍) → 回傳值的範圍
    cache: Rc<RefCell<HashMap<String, AV>>>,
}

const BIG: i128 = 1i128 << 120;

fn checked(lo: Option<i128>, hi: Option<i128>) -> AV {
    match (lo, hi) {
        (Some(a), Some(b)) if a.abs() <= BIG && b.abs() <= BIG => AV::Int(a, b),
        _ => AV::Top,
    }
}

impl<'a> Ck<'a> {
    fn new(prog: &'a Arc<Program>, func: &str, cache: Rc<RefCell<HashMap<String, AV>>>) -> Ck<'a> {
        let sh = Shared::new(prog.clone(), false, false);
        Ck {
            prog,
            it: Interp::new(sh, 0, "check".to_string()),
            func: func.to_string(),
            scopes: vec![HashMap::new()],
            assigned: HashSet::new(),
            loops: vec![],
            carries: vec![],
            ret: None,
            out: vec![],
            depth: 0,
            cache,
        }
    }

    /// 以引數的範圍分析 `d` 的本體，得到回傳值的範圍（不回報；有快取）
    fn call_range(&mut self, d: &Arc<FnDecl>, args: &[AV]) -> Option<AV> {
        if self.depth >= CALL_DEPTH || !matches!(d.kind, FnKind::Fn | FnKind::Comb) || d.body.tail.is_none() { return None; }
        let key = format!("{}{:?}", d.name, args);
        if let Some(v) = self.cache.borrow().get(&key) { return Some(v.clone()); }
        let mut ck = Ck::new(self.prog, &d.name, self.cache.clone());
        ck.depth = self.depth + 1;
        collect_assigned(&d.body, &mut ck.assigned);
        for (p, a) in d.params.iter().zip(args) {
            let t = ck.ty(&p.ty);
            let val = match &t { Some(t) => meet(a, t), None => a.clone() };
            let val = if ck.assigned.contains(&p.name) { t.as_ref().map(AV::of_ty).unwrap_or(AV::Top) } else { val };
            ck.bind(&p.name, Var { decl: t, val, carry: None, nonzero: false });
        }
        let v = ck.block(&d.body);
        let v = match d.ret.as_ref().and_then(|t| ck.ty(t)) { Some(t) => meet(&v, &t), None => v };
        self.cache.borrow_mut().insert(key, v.clone());
        Some(v)
    }

    fn ty(&mut self, t: &TypeExpr) -> Option<RtType> {
        // 泛型參數、未知的具名型別：無法求值 → 不檢查
        block_on(self.it.eval_type(t)).ok()
    }

    fn report(&mut self, code: &'static str, line: u32, msg: String, data: Vec<(&str, String)>, fix: Option<String>, min: Option<String>) {
        if self.depth > 0 { return; }
        let func = self.func.clone();
        self.out.push(Finding { code, line, func, msg, data: data.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), fix, min_type: min });
    }

    // ------------------------------------------------------------------ scopes
    fn lookup(&self, n: &str) -> Option<&Var> { self.scopes.iter().rev().find_map(|s| s.get(n)) }
    fn lookup_mut(&mut self, n: &str) -> Option<&mut Var> { self.scopes.iter_mut().rev().find_map(|s| s.get_mut(n)) }
    fn bind(&mut self, n: &str, v: Var) { self.scopes.last_mut().unwrap().insert(n.to_string(), v); }
    fn push(&mut self) { self.scopes.push(HashMap::new()); }
    fn pop(&mut self) { self.scopes.pop(); }

    /// 變數現在可能的值：被賦值過就退回宣告的型別
    fn bind_let(&mut self, n: &str, decl: Option<RtType>, init: AV) {
        let val = if self.assigned.contains(n) { decl.as_ref().map(AV::of_ty).unwrap_or(AV::Top) } else { init };
        self.bind(n, Var { decl, val, carry: None, nonzero: false });
    }

    fn func_body(&mut self, d: &FnDecl) {
        collect_assigned(&d.body, &mut self.assigned);
        for p in &d.params {
            let t = self.ty(&p.ty);
            let val = t.as_ref().map(AV::of_ty).unwrap_or(AV::Top);
            self.bind(&p.name, Var { decl: t, val, carry: None, nonzero: false });
        }
        self.ret = d.ret.as_ref().and_then(|t| self.ty(t));
        let v = self.block(&d.body);
        if let (Some(t), Some(tail)) = (self.ret.clone(), &d.body.tail) {
            self.land(&v, &t, tail.line, &format!("the result of `{}`", d.name));
        }
        self.check_carries();
    }

    // ------------------------------------------------------------------ landing sites
    /// `v` 落在型別 `t`：範圍裝不下就是 E0301
    fn land(&mut self, v: &AV, t: &RtType, line: u32, what: &str) {
        let (Some((lo, hi)), Some((tlo, thi))) = (v.int(), int_range(t)) else { return };
        if lo >= tlo && hi <= thi { return; }
        let tn = type_name(tlo, thi);
        let need = min_type(lo.min(tlo), hi.max(thi));
        self.report("E0301", line,
            format!("{}: the value's range {} does not fit {} ({})", what, rng(lo, hi), tn, rng(tlo, thi)),
            vec![("range", rng(lo, hi)), ("type", tn.clone())],
            Some(format!("declare it as {} or convert explicitly: .sat::<{}>() / .wrap::<{}>()", need, tn, tn)),
            Some(need));
    }

    fn check_carries(&mut self) {
        let carries = std::mem::take(&mut self.carries);
        for c in carries {
            let Some((a, b)) = c.init else { continue };
            let (mut lo, mut hi) = (Some(a), Some(b));
            let mut unbounded = false;
            let mut parts = vec![];
            for (n, dlo, dhi, _) in &c.deltas {
                match n {
                    Some(n) => {
                        lo = lo.and_then(|x| n.checked_mul((*dlo).min(0)).and_then(|y| x.checked_add(y)));
                        hi = hi.and_then(|x| n.checked_mul((*dhi).max(0)).and_then(|y| x.checked_add(y)));
                        parts.push(format!("{} × {}", n, rng(*dlo, *dhi)));
                    }
                    None => {
                        if *dlo < 0 { lo = None; }
                        if *dhi > 0 { hi = None; }
                        if *dlo < 0 || *dhi > 0 { unbounded = true; }
                        parts.push(format!("unbounded × {}", rng(*dlo, *dhi)));
                    }
                }
            }
            let tn = type_name(c.ty.0, c.ty.1);
            if unbounded {
                self.report("E0301", c.line,
                    format!("{} `{}`: {} cannot be shown to hold the sum: a loop around the accumulation has no bound (e.g. `for v in rx`)", c.kind, c.name, tn),
                    vec![("type", tn.clone()), ("accumulation", parts.join(" + "))],
                    Some(format!("bound the loop, or accumulate explicitly: `{} = ({} + …).sat::<{}>()` / `+%`", c.name, c.name, tn)), None);
                continue;
            }
            let (Some(lo), Some(hi)) = (lo, hi) else { continue };
            if lo >= c.ty.0 && hi <= c.ty.1 { continue; }
            let need = min_type(lo, hi);
            self.report("E0301", c.line,
                format!("{} `{}`: {} is too small: init {} + {} reaches {}", c.kind, c.name, tn, rng(a, b), parts.join(" + "), rng(lo, hi)),
                vec![("type", tn.clone()), ("range", rng(lo, hi)), ("accumulation", parts.join(" + "))],
                Some(format!("declare it as {} (or saturate: .sat::<{}>())", need, tn)),
                Some(need));
        }
    }

    /// 外層迴圈從 `depth` 起的總次數上界
    fn trips_from(&self, depth: usize) -> Option<i128> {
        self.loops[depth.min(self.loops.len())..].iter().try_fold(1i128, |acc, n| n.and_then(|n| acc.checked_mul(n)))
    }

    // ------------------------------------------------------------------ statements
    fn block(&mut self, b: &Block) -> AV {
        self.push();
        for s in &b.stmts { self.stmt(s); }
        let v = b.tail.as_ref().map(|e| self.expr(e)).unwrap_or(AV::Top);
        self.pop();
        v
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { pat, ty, init, .. } => {
                let t = ty.as_ref().and_then(|t| self.ty(t));
                let v = init.as_ref().map(|e| self.expr(e)).unwrap_or(AV::Top);
                if let (Some(t), Pattern::Ident(n)) = (&t, pat) {
                    self.land(&v, t, s.line, &format!("`let {}`", n));
                }
                match pat {
                    Pattern::Ident(n) => {
                        let init = match &t { Some(t) => meet(&v, t), None => v };
                        // 有型別、初值已知、之後會被賦值：當成累加器（推廣 T-CARRY）
                        let acc = match (&t, init.int()) {
                            (Some(RtType::Int { lo, hi }), Some(iv)) if self.assigned.contains(n) => {
                                self.carries.push(Carry { name: n.clone(), ty: (*lo, *hi), init: Some(iv), depth: self.loops.len(), deltas: vec![], line: s.line, kind: "variable" });
                                Some(self.carries.len() - 1)
                            }
                            _ => None,
                        };
                        self.bind_let(n, t, init);
                        if let (Some(ci), Some(v)) = (acc, self.lookup_mut(n)) { v.carry = Some(ci); }
                    }
                    Pattern::Tuple(ps) => {
                        for (i, p) in ps.iter().enumerate() {
                            if let Pattern::Ident(n) = p {
                                let e = if let AV::Tup(vs) = &v { vs.get(i).cloned().unwrap_or(AV::Top) } else { AV::Top };
                                self.bind_let(n, None, e);
                            }
                        }
                    }
                    Pattern::Wild => {}
                }
            }
            StmtKind::Carry { name, ty, init } => {
                let t = self.ty(ty);
                let v = self.expr(init);
                if let Some(t) = &t { self.land(&v, t, s.line, &format!("the initial value of carry `{}`", name)); }
                let idx = match (&t, v.int()) {
                    (Some(RtType::Int { lo, hi }), Some(iv)) => {
                        self.carries.push(Carry { name: name.clone(), ty: (*lo, *hi), init: Some(iv), depth: self.loops.len(), deltas: vec![], line: s.line, kind: "carry" });
                        Some(self.carries.len() - 1)
                    }
                    _ => None,
                };
                let val = t.as_ref().map(AV::of_ty).unwrap_or(AV::Top);
                self.bind(name, Var { decl: t, val, carry: idx, nonzero: false });
            }
            StmtKind::Assign { target, op, value } => self.assign(target, *op, value, s.line),
            StmtKind::For { pat, iter, body, .. } => self.for_loop(pat, iter, body),
            StmtKind::Spawn(e) => { self.expr(e); }
            StmtKind::Parallel(ss) => { for s in ss { self.stmt(s); } }
            StmtKind::Assert { .. } => {}
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    let v = self.expr(e);
                    if let Some(t) = self.ret.clone() { self.land(&v, &t, s.line, "the returned value"); }
                }
            }
            StmtKind::Expr(e) => { self.expr(e); }
        }
    }

    fn assign(&mut self, target: &Expr, op: Option<BinOp>, value: &Expr, line: u32) {
        let rhs = self.expr(value);
        match &target.kind {
            ExprKind::Path(p) if p.len() == 1 => {
                let n = &p[0];
                let Some(var) = self.lookup(n).cloned() else { return };
                // carry 的累加：x += e、x -= e、x = x + e、x = e + x、x = x - e
                if let Some(ci) = var.carry {
                    let delta = match (op, &value.kind) {
                        (Some(BinOp::Add), _) => rhs.int(),
                        (Some(BinOp::Sub), _) => rhs.int().map(|(a, b)| (-b, -a)),
                        (None, ExprKind::Binary(BinOp::Add, l, r)) if is_var(l, n) => { let r = self.expr(r); r.int() }
                        (None, ExprKind::Binary(BinOp::Add, l, r)) if is_var(r, n) => { let l = self.expr(l); l.int() }
                        (None, ExprKind::Binary(BinOp::Sub, l, r)) if is_var(l, n) => { let r = self.expr(r); r.int().map(|(a, b)| (-b, -a)) }
                        _ => None,
                    };
                    if let Some((lo, hi)) = delta {
                        let depth = self.carries[ci].depth;
                        let trips = self.trips_from(depth);
                        self.carries[ci].deltas.push((trips, lo, hi, line));
                        return;
                    }
                    // 重設（x = e）或非加法的更新（§3.3：乘法、移位不做線性展開，一步就要裝得下）：
                    // 之後的累加可能從這個值開始，併入初值範圍
                    let t = var.decl.clone().unwrap();
                    let v = match op {
                        Some(BinOp::AddW | BinOp::SubW | BinOp::MulW) => AV::of_ty(&t),
                        Some(o) => self.binop(o, &AV::of_ty(&t), &rhs, line),
                        None => rhs,
                    };
                    if !matches!(op, Some(BinOp::AddW | BinOp::SubW | BinOp::MulW)) {
                        let what = match op { Some(o) => format!("`{} {}= …`", n, op_str(o)), None => format!("`{} = …`", n) };
                        self.land(&v, &t, line, &what);
                    }
                    let c = &mut self.carries[ci];
                    c.init = match (c.init, meet(&v, &t).int()) { (Some((a, b)), Some((x, y))) => Some((a.min(x), b.max(y))), _ => None };
                    return;
                }
                let Some(t) = var.decl.clone() else {
                    if let Some(v) = self.lookup_mut(n) { v.val = AV::Top; }
                    return;
                };
                let v = match op {
                    Some(BinOp::AddW | BinOp::SubW | BinOp::MulW) => return, // 顯式回繞
                    Some(o) => self.binop(o, &AV::of_ty(&t), &rhs, line),
                    None => rhs,
                };
                let what = match op { Some(o) => format!("`{} {}= …`", n, op_str(o)), None => format!("`{} = …`", n) };
                self.land(&v, &t, line, &what);
                if let Some(x) = self.lookup_mut(n) { x.val = AV::of_ty(&t); }
            }
            ExprKind::Index(a, i) => {
                let av = self.expr(a);
                let iv = self.expr(i);
                if let AV::Arr(_, n) = &av {
                    self.index_bounds(&iv, *n, line);
                    if op.is_none() {
                        if let Some(t) = elem_decl(&av) { self.land(&rhs, &t, line, "the array element"); }
                    }
                }
            }
            _ => { self.expr(target); }
        }
    }

    fn index_bounds(&mut self, iv: &AV, n: Option<usize>, line: u32) {
        if let (Some((lo, hi)), Some(n)) = (iv.int(), n) {
            if lo < 0 || hi >= n as i128 {
                self.report("E0703", line,
                    format!("index range {} is not within 0..={} (an array of {})", rng(lo, hi), n as i128 - 1, n),
                    vec![("range", rng(lo, hi)), ("length", n.to_string())],
                    Some("guard the index (`if i < N { … }`) or narrow its range".to_string()), None);
            }
        }
    }

    fn for_loop(&mut self, pat: &Pattern, iter: &Expr, body: &Block) {
        let (var, trips) = match &iter.kind {
            ExprKind::Range(a, b, incl) => {
                let (av, bv) = (self.expr(a), self.expr(b));
                match (av.int(), bv.int()) {
                    (Some((lo, _)), Some((_, bhi))) => {
                        // 變數範圍：[a 的下界, b 的上界（不含）]；次數上界：這個範圍的大小
                        let hi = if *incl { bhi } else { bhi - 1 };
                        (if hi >= lo { AV::Int(lo, hi) } else { AV::Top }, Some((hi - lo + 1).max(0)))
                    }
                    _ => (AV::Top, None),
                }
            }
            _ => {
                let v = self.expr(iter);
                match v {
                    AV::Chan(e) => (*e, None),
                    AV::Arr(e, Some(n)) => (*e, Some(n as i128)),
                    _ => (AV::Top, None),
                }
            }
        };
        self.loops.push(trips);
        self.push();
        if let Pattern::Ident(n) = pat { self.bind(n, Var { decl: None, val: var, carry: None, nonzero: false }); }
        self.block(body);
        self.pop();
        self.loops.pop();
    }

    // ------------------------------------------------------------------ expressions
    fn expr(&mut self, e: &Expr) -> AV {
        match &e.kind {
            ExprKind::Int(n) => AV::Int(*n, *n),
            ExprKind::Bool(_) => AV::Bool,
            ExprKind::Path(p) if p.len() == 1 => {
                if let Some(v) = self.lookup(&p[0]) { return v.val.clone(); }
                let c = self.prog.consts.read().ok().and_then(|m| m.get(&p[0]).cloned());
                c.map(|v| AV::of_value(&v)).unwrap_or(AV::Top)
            }
            ExprKind::Unary(op, x) => {
                let v = self.expr(x);
                match (op, v) {
                    ('-', AV::Int(a, b)) => checked(b.checked_neg(), a.checked_neg()),
                    ('!', AV::Bool) => AV::Bool,
                    _ => AV::Top,
                }
            }
            ExprKind::Binary(op, l, r) => {
                let (a, b) = (self.expr(l), self.expr(r));
                let has_zero = b.int().is_some_and(|(b0, b1)| b0 <= 0 && b1 >= 0);
                if matches!(op, BinOp::Div | BinOp::Rem) && has_zero && self.nonzero(r) {
                    // 除數已知不為 0：範圍有洞，用兩側的端點算
                    // 商：|q| ≤ |a|；餘數：|r| < |b|，正負號跟著 a
                    return match (a.int(), b.int()) {
                        (Some((a0, a1)), Some((b0, b1))) if *op == BinOp::Rem => {
                            let m = b0.abs().max(b1.abs()) - 1;
                            if a0 >= 0 { AV::Int(0, m.min(a1)) } else { AV::Int(-m, m) }
                        }
                        (Some((a0, a1)), Some(_)) => { let m = a0.abs().max(a1.abs()); AV::Int(-m, m) }
                        _ => AV::Top,
                    };
                }
                self.binop(*op, &a, &b, e.line)
            }
            ExprKind::Cast(x, t) => { self.expr(x); self.ty(t).map(|t| AV::of_ty(&t)).unwrap_or(AV::Top) }
            ExprKind::Method(x, name, targs, args) => {
                let v = self.expr(x);
                let avs: Vec<AV> = args.iter().map(|a| self.expr(&a.value)).collect();
                match name.as_str() {
                    "recv" => if let AV::Chan(t) = v { *t } else { AV::Top },
                    "send" => {
                        if let (AV::Chan(t), Some(a)) = (&v, avs.first()) {
                            if let Some(rt) = av_type(t) { self.land(a, &rt, e.line, "the value sent"); }
                        }
                        AV::Top
                    }
                    "sat" | "wrap" | "to" => match targs.first() {
                        Some(TArg::Ty(t)) => match self.ty(t) {
                            Some(t) => match (v.int(), int_range(&t)) {
                                // 裝得下：值不變
                                (Some((a, b)), Some((c, d))) if a >= c && b <= d => AV::Int(a, b),
                                // 飽和：夾在型別的範圍裡
                                (Some((a, b)), Some((c, d))) if name == "sat" && a.max(c) <= b.min(d) => AV::Int(a.max(c), b.min(d)),
                                _ => AV::of_ty(&t),
                            },
                            None => AV::Top,
                        },
                        _ => AV::Top,
                    },
                    _ => AV::Top,
                }
            }
            ExprKind::Call(f, args) => {
                let avs: Vec<AV> = args.iter().map(|a| self.expr(&a.value)).collect();
                let ExprKind::Path(p) = &f.kind else { return AV::Top };
                let Some(d) = self.prog.fns.get(p.last().unwrap()).cloned() else { return AV::Top };
                if !d.generics.is_empty() || d.kind == FnKind::Comptime { return AV::Top; }
                for (a, prm) in avs.iter().zip(&d.params) {
                    if let Some(t) = self.ty(&prm.ty) { self.land(a, &t, e.line, &format!("argument `{}` of `{}`", prm.name, d.name)); }
                }
                if let Some(v) = self.call_range(&d, &avs) { return v; }
                d.ret.as_ref().and_then(|t| self.ty(t)).map(|t| AV::of_ty(&t)).unwrap_or(AV::Top)
            }
            ExprKind::Index(a, i) => {
                let (av, iv) = (self.expr(a), self.expr(i));
                match av {
                    AV::Arr(el, n) => { self.index_bounds(&iv, n, e.line); *el }
                    // 位元索引
                    AV::Int(..) => AV::Int(0, 1),
                    _ => AV::Top,
                }
            }
            ExprKind::Slice(x, hi, lo) => {
                self.expr(x);
                match (self.expr(hi).int(), self.expr(lo).int()) {
                    (Some((h, h2)), Some((l, l2))) if h == h2 && l == l2 && h >= l && h - l < 120 => AV::Int(0, (1i128 << (h - l + 1)) - 1),
                    _ => AV::Top,
                }
            }
            ExprKind::Array(xs) => {
                let vs: Vec<AV> = xs.iter().map(|x| self.expr(x)).collect();
                let el = vs.iter().cloned().reduce(|a, b| a.join(&b)).unwrap_or(AV::Top);
                AV::Arr(Box::new(el), Some(xs.len()))
            }
            ExprKind::Repeat(x, n) => {
                let v = self.expr(x);
                let n = self.expr(n).int().filter(|(a, b)| a == b && *a >= 0).map(|(a, _)| a as usize);
                AV::Arr(Box::new(v), n)
            }
            ExprKind::Tuple(xs) => AV::Tup(xs.iter().map(|x| self.expr(x)).collect()),
            ExprKind::Field(x, f) => match (self.expr(x), f.parse::<usize>()) {
                (AV::Tup(vs), Ok(i)) => vs.get(i).cloned().unwrap_or(AV::Top),
                _ => AV::Top,
            },
            ExprKind::Block(b) => self.block(b),
            ExprKind::If(c, then, els) => {
                self.expr(c);
                let refine = conds(c);
                // 收窄成空集合的分支走不到：不分析，也不併入結果
                self.push();
                let a = if self.refine(&refine, false) { Some(self.block(then)) } else { None };
                self.pop();
                let b = match els {
                    Some(x) => {
                        self.push();
                        let ok = refine.len() != 1 || self.refine(&refine, true);
                        let b = if ok { Some(self.expr(x)) } else { None };
                        self.pop();
                        b
                    }
                    None => Some(AV::Top),
                };
                match (a, b) { (Some(a), Some(b)) => a.join(&b), (Some(v), None) | (None, Some(v)) => v, (None, None) => AV::Top }
            }
            ExprKind::Match(x, arms) => {
                self.expr(x);
                let mut v: Option<AV> = None;
                for arm in arms {
                    self.push();
                    bind_pat(self, &arm.pat);
                    if let Some(g) = &arm.guard { self.expr(g); }
                    let b = self.expr(&arm.body);
                    self.pop();
                    v = Some(match v { None => b, Some(a) => a.join(&b) });
                }
                v.unwrap_or(AV::Top)
            }
            ExprKind::Range(a, b, _) => { self.expr(a); self.expr(b); AV::Top }
            ExprKind::Lambda(..) | ExprKind::Comptime(_) => AV::Top,
            _ => AV::Top,
        }
    }

    fn binop(&mut self, op: BinOp, a: &AV, b: &AV, line: u32) -> AV {
        use BinOp::*;
        match op {
            Eq | Ne | Lt | Le | Gt | Ge | And | Or => return AV::Bool,
            _ => {}
        }
        let (Some((a0, a1)), Some((b0, b1))) = (a.int(), b.int()) else { return AV::Top };
        match op {
            Add => checked(a0.checked_add(b0), a1.checked_add(b1)),
            Sub => checked(a0.checked_sub(b1), a1.checked_sub(b0)),
            Mul => {
                let ps = [a0.checked_mul(b0), a0.checked_mul(b1), a1.checked_mul(b0), a1.checked_mul(b1)];
                if ps.iter().any(|p| p.is_none()) { return AV::Top; }
                let ps: Vec<i128> = ps.iter().map(|p| p.unwrap()).collect();
                checked(ps.iter().min().copied(), ps.iter().max().copied())
            }
            Div | Rem => {
                if b0 <= 0 && b1 >= 0 {
                    self.report("E0102", line,
                        format!("the divisor's range {} includes 0", rng(b0, b1)),
                        vec![("range", rng(b0, b1))],
                        Some("narrow the divisor's type so it excludes 0, or test it first (`if d != 0`)".to_string()), None);
                    return AV::Top;
                }
                if op == Rem {
                    let m = b0.abs().max(b1.abs()) - 1;
                    return if a0 >= 0 { AV::Int(0, m.min(a1)) } else { AV::Int(-m, m) };
                }
                // 除數不含 0：商的極值落在端點組合上
                let qs = [a0 / b0, a0 / b1, a1 / b0, a1 / b1];
                AV::Int(*qs.iter().min().unwrap(), *qs.iter().max().unwrap())
            }
            Shl | Shr => {
                if b0 != b1 || !(0..120).contains(&b0) { return AV::Top; }
                let k = b0 as u32;
                if op == Shl { checked(a0.checked_mul(1i128 << k), a1.checked_mul(1i128 << k)) } else { AV::Int(a0 >> k, a1 >> k) }
            }
            BitAnd if a0 >= 0 && b0 >= 0 => AV::Int(0, a1.min(b1)),
            BitOr | BitXor if a0 >= 0 && b0 >= 0 => {
                let w = int_bits(0, a1.max(b1));
                AV::Int(0, (1i128 << w) - 1)
            }
            _ => AV::Top,
        }
    }

    /// 已知不為 0 的除數：變數（`if x != 0` 收窄過），或它同寬以上的 `.wrap::<T>()`
    fn nonzero(&mut self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Path(p) if p.len() == 1 => self.lookup(&p[0]).is_some_and(|v| v.nonzero || v.val.int().is_some_and(|(a, b)| a > 0 || b < 0)),
            ExprKind::Method(x, m, targs, _) if m == "wrap" || m == "sat" => {
                let ExprKind::Path(p) = &x.kind else { return false };
                let Some(var) = self.lookup(&p[0]).cloned() else { return false };
                let src = var.decl.as_ref().and_then(int_range).map(|(a, b)| int_bits(a, b)).unwrap_or(u32::MAX);
                let dst = match targs.first() { Some(TArg::Ty(t)) => self.ty(t).as_ref().and_then(int_range).map(|(a, b)| int_bits(a, b)).unwrap_or(0), _ => 0 };
                (m == "sat" || dst >= src) && self.nonzero(x)
            }
            _ => false,
        }
    }

    /// 條件為真（`neg`：為假）時收窄變數的範圍；false = 收窄成空集合（這個分支走不到）
    fn refine(&mut self, cs: &[(String, BinOp, Expr)], neg: bool) -> bool {
        for (n, op, rhs) in cs {
            let Some(var) = self.lookup(n).cloned() else { continue };
            let (Some((lo, hi)), Some((c0, c1))) = (var.val.int(), self.expr(rhs).int()) else { continue };
            let op = if neg { negate(*op) } else { *op };
            let (nlo, nhi) = match op {
                BinOp::Lt => (lo, hi.min(c1 - 1)),
                BinOp::Le => (lo, hi.min(c1)),
                BinOp::Gt => (lo.max(c0 + 1), hi),
                BinOp::Ge => (lo.max(c0), hi),
                BinOp::Eq => (lo.max(c0), hi.min(c1)),
                // x != c（c 是常數）：只能削掉端點；c 為 0 時記下「不為 0」
                BinOp::Ne if c0 == c1 => (if lo == c0 { lo + 1 } else { lo }, if hi == c0 { hi - 1 } else { hi }),
                _ => continue,
            };
            if nlo > nhi { return false; }
            let nonzero = var.nonzero || (op == BinOp::Ne && c0 == 0 && c1 == 0);
            self.bind(n, Var { val: AV::Int(nlo, nhi), nonzero, ..var });
        }
        true
    }
}

fn negate(op: BinOp) -> BinOp {
    match op { BinOp::Lt => BinOp::Ge, BinOp::Le => BinOp::Gt, BinOp::Gt => BinOp::Le, BinOp::Ge => BinOp::Lt, BinOp::Eq => BinOp::Ne, BinOp::Ne => BinOp::Eq, o => o }
}

fn flip(op: BinOp) -> BinOp {
    match op { BinOp::Lt => BinOp::Gt, BinOp::Le => BinOp::Ge, BinOp::Gt => BinOp::Lt, BinOp::Ge => BinOp::Le, o => o }
}

/// `x < e && y >= f …` 的每個比較（變數在任一邊）
fn conds(c: &Expr) -> Vec<(String, BinOp, Expr)> {
    match &c.kind {
        ExprKind::Binary(BinOp::And, a, b) => { let mut v = conds(a); v.extend(conds(b)); v }
        ExprKind::Binary(op @ (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne), a, b) => {
            match (&a.kind, &b.kind) {
                (ExprKind::Path(p), _) if p.len() == 1 => vec![(p[0].clone(), *op, (**b).clone())],
                (_, ExprKind::Path(p)) if p.len() == 1 => vec![(p[0].clone(), flip(*op), (**a).clone())],
                _ => vec![],
            }
        }
        _ => vec![],
    }
}

fn bind_pat(ck: &mut Ck, p: &MatchPat) {
    match p {
        MatchPat::Bind(n) => ck.bind(n, Var { decl: None, val: AV::Top, carry: None, nonzero: false }),
        MatchPat::Tuple(ps) => for p in ps { bind_pat(ck, p) },
        _ => {}
    }
}

/// 型別範圍與值的交集（`let x: T = e` 之後 x 的範圍）
fn meet(v: &AV, t: &RtType) -> AV {
    match (v.int(), int_range(t)) {
        (Some((a, b)), Some((c, d))) if a.max(c) <= b.min(d) => AV::Int(a.max(c), b.min(d)),
        _ => AV::of_ty(t),
    }
}

/// 抽象值所代表的型別（通道元素的落點）
fn av_type(v: &AV) -> Option<RtType> {
    match v { AV::Int(lo, hi) => Some(RtType::Int { lo: *lo, hi: *hi }), _ => None }
}

fn elem_decl(v: &AV) -> Option<RtType> {
    if let AV::Arr(e, _) = v { av_type(e) } else { None }
}

fn is_var(e: &Expr, n: &str) -> bool { matches!(&e.kind, ExprKind::Path(p) if p.len() == 1 && p[0] == n) }


fn op_str(o: BinOp) -> &'static str {
    match o { BinOp::Add => "+", BinOp::Sub => "-", BinOp::Mul => "*", BinOp::Div => "/", BinOp::Rem => "%", BinOp::Shl => "<<", BinOp::Shr => ">>", BinOp::BitAnd => "&", BinOp::BitOr => "|", BinOp::BitXor => "^", _ => "?" }
}

/// 函式裡所有被賦值的變數名稱
fn collect_assigned(b: &Block, out: &mut HashSet<String>) {
    fn stmt(s: &Stmt, out: &mut HashSet<String>) {
        match &s.kind {
            StmtKind::Assign { target, .. } => {
                let mut t = target;
                while let ExprKind::Index(a, _) | ExprKind::Field(a, _) | ExprKind::Slice(a, _, _) = &t.kind { t = a; }
                if let ExprKind::Path(p) = &t.kind { if p.len() == 1 { out.insert(p[0].clone()); } }
            }
            StmtKind::For { body, .. } => collect_assigned(body, out),
            StmtKind::Parallel(ss) => for s in ss { stmt(s, out) },
            _ => {}
        }
        stmt_exprs(s, &mut |e| walk(e, &mut |x| match &x.kind {
            ExprKind::Block(b) | ExprKind::If(_, b, _) => collect_assigned(b, out),
            _ => {}
        }));
    }
    for s in &b.stmts { stmt(s, out); }
    if let Some(t) = &b.tail {
        walk(t, &mut |x| match &x.kind {
            ExprKind::Block(b) | ExprKind::If(_, b, _) => collect_assigned(b, out),
            _ => {}
        });
    }
}

fn stmt_exprs(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    match &s.kind {
        StmtKind::Let { init: Some(e), .. } | StmtKind::Carry { init: e, .. } | StmtKind::Spawn(e) | StmtKind::Expr(e) | StmtKind::Return(Some(e)) => f(e),
        StmtKind::Assign { target, value, .. } => { f(target); f(value) }
        StmtKind::For { iter, .. } => f(iter),
        _ => {}
    }
}

/// 運算式樹的每個節點（不進入區塊裡的陳述式：`collect_assigned` 另外處理）
fn walk(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Field(x, _) | ExprKind::Lambda(_, x) => walk(x, f),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b, _) | ExprKind::Repeat(a, b) => { walk(a, f); walk(b, f) }
        ExprKind::Slice(a, b, c) => { walk(a, f); walk(b, f); walk(c, f) }
        ExprKind::Call(x, args) => { walk(x, f); for a in args { walk(&a.value, f) } }
        ExprKind::Method(x, _, _, args) => { walk(x, f); for a in args { walk(&a.value, f) } }
        ExprKind::Array(xs) | ExprKind::Tuple(xs) => for x in xs { walk(x, f) },
        ExprKind::If(c, b, els) => {
            walk(c, f);
            if let Some(t) = &b.tail { walk(t, f) }
            if let Some(x) = els { walk(x, f) }
        }
        ExprKind::Match(x, arms) => { walk(x, f); for a in arms { walk(&a.body, f) } }
        ExprKind::Block(b) => if let Some(t) = &b.tail { walk(t, f) },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::check;

    fn run(src: &str) -> Vec<(String, String)> {
        let p = crate::load("t.fx", src).unwrap();
        check(&p).into_iter().map(|f| (f.code.to_string(), f.min_type.unwrap_or_default())).collect()
    }

    #[test]
    fn carry_sum_over_the_loop_bound() {
        // T-CARRY：65535 次 × i16² 需要 i47；8 次 × u8 裝得進 u11
        let f = run("task M(a: in chan<i16>, o: out chan<i24>, n: u16) { carry s: i24 = 0; for _ in 0..n { let x = a.recv(); s += x * x; } o.send(s); }");
        assert_eq!(f, vec![("E0301".into(), "i47".into())]);
        assert!(run("task M(a: in chan<u8>, o: out chan<u11>) { carry s: u11 = 0; for _ in 0..8 { s += a.recv(); } o.send(s); }").is_empty());
        // 無界的串流
        assert_eq!(run("task C(x: in chan<u8>, o: out chan<u8>) { carry c: u8 = 0; for _ in x { c += 1; } o.send(c); }").len(), 1);
    }

    #[test]
    fn landing_sites() {
        assert_eq!(run("test t { let a: u8 = 200; let b: u8 = 100; let c = a + b; let d: u8 = c; }"), vec![("E0301".into(), "u9".into())]);
        assert_eq!(run("task G(n: u32, o: out chan<u8>) { for i in 0..n { o.send(i); } }").len(), 1);
        assert!(run("task D(r: in chan<u8>, o: out chan<u9>) { for v in r { o.send(v * 2); } }").is_empty());
    }

    #[test]
    fn refinement_and_calls() {
        // 收窄：走不到的分支不算；函式以引數的範圍分析
        let src = "comb fn slot(a: u12) -> u6 { if a <= 3 { a.wrap::<u6>() } else if a == 0x180 { 4 } else { 5 } }
                   fn f(i: u12) -> u64 { let t: [u64; 6] = [0; 6]; t[slot(i)] }";
        assert!(run(src).is_empty(), "{:?}", run(src));
        assert_eq!(run("fn g(i: u4) -> u8 { let t: [u8; 6] = [0; 6]; t[i] }"), vec![("E0703".into(), String::new())]);
        assert!(run("fn h(i: u4) -> u8 { let t: [u8; 6] = [0; 6]; if i < 6 { t[i] } else { 0 } }").is_empty());
    }

    #[test]
    fn divisors() {
        assert_eq!(run("fn d(a: u8, b: u8) -> u8 { a / b }"), vec![("E0102".into(), String::new())]);
        assert!(run("fn d(a: u8, b: u8) -> u8 { if b == 0 { 255 } else { a / b } }").is_empty());
        assert!(run("fn d(a: u64, b: u64) -> u64 { if b != 0 { (a.wrap::<i64>() / b.wrap::<i64>()).wrap::<u64>() } else { 0 } }").is_empty());
    }
}
