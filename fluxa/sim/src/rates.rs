//! 通道與記憶體的靜態檢查（`reda check`，`check.rs` 呼叫）：
//!
//! - E0501：通道的 token 守恆。整段執行裡，一條通道送出的 token 數必須等於收到的數目，否則
//!   接收端永遠等不到（死結），或送出端在 FIFO 滿時卡住。每個端點的次數是線性式
//!   `c + Σ k·T(ch)`：常數迴圈乘上次數，`for v in rx` 的次數是 rx 上的 token 數 T(rx)。
//!   網路（`test`，或 spawn 其他任務的 `task`）的每條區域通道列一條方程，代入求解後比對。
//!   規格 §3.4 的 SDF 平衡在有限次執行上的形式。
//! - E0401：`pipeline(ii = n)` 迴圈每一輪對同一個資源的存取次數超過埠數 × n。資源是通道端點
//!   （每個時脈一個 token）與 `mem`（一塊 block RAM：一個讀埠、一個寫埠）；規格 §3.5 的模型，
//!   但只有一個 bank（語言還沒有分割的語法）。
//!
//! 次數無法確定（迴圈界限未知、條件分支的次數不同）就不檢查。

use crate::ast::*;
use crate::check::Finding;
use crate::value::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

// ====================================================================== 線性式

/// `c + Σ k·T(x)`：x 是通道的鍵
#[derive(Clone, Debug, PartialEq)]
struct Lin {
    c: i128,
    t: BTreeMap<String, i128>,
}

impl Lin {
    fn konst(c: i128) -> Lin { Lin { c, t: BTreeMap::new() } }
    fn var(x: &str) -> Lin { let mut t = BTreeMap::new(); t.insert(x.to_string(), 1); Lin { c: 0, t } }
    fn add(&self, o: &Lin) -> Lin {
        let mut r = self.clone();
        r.c += o.c;
        for (k, v) in &o.t { *r.t.entry(k.clone()).or_default() += v; }
        r.t.retain(|_, v| *v != 0);
        r
    }
    fn scale(&self, k: i128) -> Lin {
        let mut r = Lin { c: self.c * k, t: self.t.iter().map(|(x, v)| (x.clone(), v * k)).collect() };
        r.t.retain(|_, v| *v != 0);
        r
    }
    fn mentions(&self, x: &str) -> bool { self.t.contains_key(x) }
    /// 代入 x := v
    fn subst(&self, x: &str, v: &Lin) -> Lin {
        match self.t.get(x) {
            None => self.clone(),
            Some(k) => { let mut r = self.clone(); r.t.remove(x); r.add(&v.scale(*k)) }
        }
    }
    fn rename(&self, f: &dyn Fn(&str) -> String) -> Lin {
        let mut r = Lin::konst(self.c);
        for (k, v) in &self.t { *r.t.entry(f(k)).or_default() += v; }
        r
    }
    fn show(&self, names: &dyn Fn(&str) -> String) -> String {
        let mut parts: Vec<String> = vec![];
        for (x, k) in &self.t { parts.push(if *k == 1 { format!("T({})", names(x)) } else { format!("{}·T({})", k, names(x)) }); }
        if self.c != 0 || parts.is_empty() { parts.insert(0, self.c.to_string()); }
        parts.join(" + ")
    }
}

/// 一個端點的次數：線性式，或未知
type Cnt = Option<Lin>;

fn cadd(a: &Cnt, b: &Cnt) -> Cnt { match (a, b) { (Some(a), Some(b)) => Some(a.add(b)), _ => None } }

// ====================================================================== 一段程式的次數

/// 一段程式（任務或測試本體）的端點次數
#[derive(Clone, Default)]
struct Summary {
    /// 端點名稱 → (送出次數, 接收次數)
    ops: HashMap<String, (Cnt, Cnt)>,
}

impl Summary {
    fn bump(&mut self, ep: &str, send: bool, n: Cnt) {
        let e = self.ops.entry(ep.to_string()).or_insert((Some(Lin::konst(0)), Some(Lin::konst(0))));
        let slot = if send { &mut e.0 } else { &mut e.1 };
        *slot = cadd(slot, &n);
    }
    fn merge(&mut self, o: &Summary) {
        for (k, (s, r)) in &o.ops { self.bump(k, true, s.clone()); self.bump(k, false, r.clone()); }
    }
}

/// 外層迴圈的乘數：k 次，或 k·T(串流) 次；None = 未知
#[derive(Clone)]
struct Mult(Option<(i128, Option<String>)>);

impl Mult {
    fn lin(&self) -> Cnt {
        self.0.as_ref().map(|(k, s)| match s { None => Lin::konst(*k), Some(x) => Lin::var(x).scale(*k) })
    }
}

/// 通道的一端：區域通道（鍵、是否為送出端），或參數
#[derive(Clone, Debug)]
enum Ep {
    Local { key: String, tx: bool },
    Param(String),
}

/// 一條區域通道
struct Chan {
    line: u32,
    depth: Option<i128>,
    label: String,
    /// broadcast 的輸出：T = T(來源)
    from: Option<String>,
}

struct Rates<'a> {
    prog: &'a Arc<Program>,
    out: Vec<Finding>,
    /// (任務, 常數引數) → 任務參數上的次數
    cache: HashMap<String, Option<Summary>>,
    depth: usize,
}

/// 一段程式走訪時的狀態
struct Body {
    func: String,
    /// 已知的整數值（常數引數、`let n = 8`）與陣列長度
    ints: HashMap<String, i128>,
    lens: HashMap<String, usize>,
    /// 端點名稱 → 端點
    eps: HashMap<String, Ep>,
    /// broadcast 的結果：名稱 → 各個輸出的通道鍵
    arrays: HashMap<String, Vec<String>>,
    chans: BTreeMap<String, Chan>,
    sum: Summary,
    /// 任務呼叫無法分析：網路不完整，不檢查守恆
    incomplete: bool,
    /// 交給 spawn 的端點 → spawn 的行號（之後再用是線性違規，E0702）
    moved: HashMap<String, u32>,
}

impl<'a> Rates<'a> {
    fn const_of(&self, b: &Body, e: &Expr) -> Option<i128> {
        match &e.kind {
            ExprKind::Int(n) => Some(*n),
            ExprKind::Path(p) if p.len() == 1 => b.ints.get(&p[0]).copied().or_else(|| match self.prog.consts.read().ok()?.get(&p[0]) { Some(Value::Int(n)) => Some(*n), _ => None }),
            ExprKind::Unary('-', x) => self.const_of(b, x).and_then(|v| v.checked_neg()),
            ExprKind::Binary(op, l, r) => {
                let (a, c) = (self.const_of(b, l)?, self.const_of(b, r)?);
                match op {
                    BinOp::Add => a.checked_add(c), BinOp::Sub => a.checked_sub(c), BinOp::Mul => a.checked_mul(c),
                    BinOp::Div if c != 0 => Some(a / c), BinOp::Rem if c != 0 => Some(a % c),
                    BinOp::Shl if (0..120).contains(&c) => a.checked_mul(1i128 << c), BinOp::Shr if (0..127).contains(&c) => Some(a >> c),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn len_of(&self, b: &Body, e: &Expr) -> Option<usize> {
        match &e.kind {
            ExprKind::Array(xs) => Some(xs.len()),
            ExprKind::Repeat(_, n) => self.const_of(b, n).map(|n| n as usize),
            ExprKind::Path(p) if p.len() == 1 => b.lens.get(&p[0]).copied().or_else(|| match self.prog.consts.read().ok()?.get(&p[0]) { Some(Value::Array(xs)) => Some(xs.len()), _ => None }),
            _ => None,
        }
    }

    /// 運算式指到的端點名稱（`tx`、`outs[1]`）
    fn ep_name(&self, b: &Body, e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Path(p) if p.len() == 1 && b.eps.contains_key(&p[0]) => Some(p[0].clone()),
            ExprKind::Index(a, i) => {
                let ExprKind::Path(p) = &a.kind else { return None };
                let i = self.const_of(b, i)?;
                let keys = b.arrays.get(&p[0])?;
                keys.get(i as usize).map(|_| format!("{}[{}]", p[0], i))
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------------ walk
    fn block(&mut self, b: &mut Body, blk: &Block, m: &Mult) {
        for s in &blk.stmts { self.stmt(b, s, m); }
        if let Some(t) = &blk.tail { self.expr(b, t, m); }
    }

    fn stmt(&mut self, b: &mut Body, s: &Stmt, m: &Mult) {
        match &s.kind {
            StmtKind::Let { pat, init: Some(e), .. } => {
                // let (tx, rx) = chan::<T>(depth = d)
                if let (Pattern::Tuple(ps), Some(depth)) = (pat, chan_call(e)) {
                    if let [Pattern::Ident(t), Pattern::Ident(r)] = &ps[..] {
                        let key = format!("{}->{}@{}", t, r, s.line);
                        let depth = depth.and_then(|d| self.const_of(b, d));
                        b.chans.insert(key.clone(), Chan { line: s.line, depth, label: format!("{}->{}", t, r), from: None });
                        b.eps.insert(t.clone(), Ep::Local { key: key.clone(), tx: true });
                        b.eps.insert(r.clone(), Ep::Local { key, tx: false });
                        return;
                    }
                }
                // let outs = broadcast(rx, n)
                if let (Pattern::Ident(n), ExprKind::Call(f, args)) = (pat, &e.kind) {
                    if matches!(&f.kind, ExprKind::Path(p) if p.len() == 1 && p[0] == "broadcast") && args.len() == 2 {
                        if let (Some(src), Some(k)) = (self.ep_name(b, &args[0].value), self.const_of(b, &args[1].value)) {
                            // broadcast 收完來源的所有 token（T(來源)），每個輸出各送 T(來源)
                            let skey = self.key_of(b, &src);
                            b.sum.bump(&src, false, skey.as_ref().map(|k| Lin::var(k)));
                            let mut keys = vec![];
                            for i in 0..k.max(0) {
                                let key = format!("{}[{}]@{}", n, i, s.line);
                                b.chans.insert(key.clone(), Chan { line: s.line, depth: None, label: format!("{}[{}]", n, i), from: skey.clone() });
                                b.eps.insert(format!("{}[{}]", n, i), Ep::Local { key: key.clone(), tx: false });
                                keys.push(key);
                            }
                            b.arrays.insert(n.clone(), keys);
                            return;
                        }
                    }
                }
                self.expr(b, e, m);
                if let Pattern::Ident(n) = pat {
                    if let Some(v) = self.const_of(b, e) { b.ints.insert(n.clone(), v); }
                    if let Some(l) = self.len_of(b, e) { b.lens.insert(n.clone(), l); }
                }
            }
            StmtKind::Let { .. } => {}
            StmtKind::Carry { init, .. } => self.expr(b, init, m),
            StmtKind::Assign { target, value, .. } => { self.expr(b, value, m); self.expr(b, target, m); }
            StmtKind::For { pat, iter, body, .. } => self.for_loop(b, pat, iter, body, m),
            StmtKind::Spawn(e) => self.spawn(b, e, m),
            StmtKind::Parallel(ss) => for s in ss { self.stmt(b, s, m) },
            StmtKind::Assert { cond, msg, .. } => { self.expr(b, cond, m); if let Some(x) = msg { self.expr(b, x, m); } }
            StmtKind::Return(Some(e)) | StmtKind::Expr(e) => self.expr(b, e, m),
            StmtKind::Return(None) => {}
        }
    }

    fn for_loop(&mut self, b: &mut Body, pat: &Pattern, iter: &Expr, body: &Block, m: &Mult) {
        // for v in rx：收完所有 token；本體跑 T(rx) 次
        if let Some(ep) = self.ep_name(b, iter) {
            let key = self.key_of(b, &ep);
            let inner = match (&m.0, &key) {
                (Some((1, None)), Some(k)) => { b.sum.bump(&ep, false, Some(Lin::var(k))); Mult(Some((1, Some(k.clone())))) }
                _ => { b.sum.bump(&ep, false, None); Mult(None) }
            };
            self.block(b, body, &inner);
            return;
        }
        let n = match &iter.kind {
            ExprKind::Range(a, z, incl) => match (self.const_of(b, a), self.const_of(b, z)) {
                (Some(a), Some(z)) => Some((z - a + if *incl { 1 } else { 0 }).max(0)),
                _ => None,
            },
            _ => { self.expr(b, iter, m); self.len_of(b, iter).map(|n| n as i128) }
        };
        let inner = match (&m.0, n) {
            (Some((k, s)), Some(n)) => Mult(Some((k * n, s.clone()))),
            _ => Mult(None),
        };
        let _ = pat;
        self.block(b, body, &inner);
    }

    fn expr(&mut self, b: &mut Body, e: &Expr, m: &Mult) {
        match &e.kind {
            ExprKind::Method(x, name, _, args) => {
                for a in args { self.expr(b, &a.value, m); }
                if let Some(ep) = self.ep_name(b, x) {
                    if let Some(at) = b.moved.get(&ep).copied() {
                        if name == "recv" || name == "send" || name == "close" {
                            self.out.push(Finding {
                                code: "E0702", line: e.line, func: b.func.clone(),
                                msg: format!("`{}.{}()`: the endpoint `{}` was given to the task spawned on line {}", ep, name, ep, at % FILE_STRIDE),
                                data: vec![("endpoint".into(), ep.clone()), ("spawned".into(), (at % FILE_STRIDE).to_string())],
                                fix: Some("an endpoint has one owner: use another channel, or keep the endpoint here".into()),
                                min_type: None,
                            });
                        }
                    }
                    match name.as_str() {
                        "recv" => b.sum.bump(&ep, false, m.lin()),
                        "send" => b.sum.bump(&ep, true, m.lin()),
                        _ => {}
                    }
                } else {
                    self.expr(b, x, m);
                }
            }
            // 分支：每個端點在各分支的次數相同才算得出來
            ExprKind::If(c, then, els) => {
                self.expr(b, c, m);
                let mut arms: Vec<Summary> = vec![];
                let mut t = self.sub(b, |me, bb| me.block(bb, then, m));
                arms.push(std::mem::take(&mut t));
                arms.push(match els { Some(x) => self.sub(b, |me, bb| me.expr(bb, x, m)), None => Summary::default() });
                self.join(b, arms);
            }
            ExprKind::Match(x, ms) => {
                self.expr(b, x, m);
                let arms: Vec<Summary> = ms.iter().map(|a| self.sub(b, |me, bb| me.expr(bb, &a.body, m))).collect();
                self.join(b, arms);
            }
            ExprKind::Block(blk) => self.block(b, blk, m),
            ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Field(x, _) => self.expr(b, x, m),
            ExprKind::Binary(_, x, y) | ExprKind::Index(x, y) => { self.expr(b, x, m); self.expr(b, y, m); }
            ExprKind::Call(f, args) => {
                self.expr(b, f, m);
                for a in args { self.expr(b, &a.value, m); }
                // 把端點交給函式：次數看不到
                for a in args { if let Some(ep) = self.ep_name(b, &a.value) { b.sum.bump(&ep, true, None); b.sum.bump(&ep, false, None); } }
            }
            ExprKind::Array(xs) | ExprKind::Tuple(xs) => for x in xs { self.expr(b, x, m) },
            _ => {}
        }
    }

    /// 在另一份次數表上走訪一個分支
    fn sub(&mut self, b: &mut Body, f: impl FnOnce(&mut Self, &mut Body)) -> Summary {
        let saved = std::mem::take(&mut b.sum);
        f(self, b);
        std::mem::replace(&mut b.sum, saved)
    }

    fn join(&mut self, b: &mut Body, arms: Vec<Summary>) {
        let mut keys: Vec<String> = arms.iter().flat_map(|a| a.ops.keys().cloned()).collect();
        keys.sort();
        keys.dedup();
        let zero = (Some(Lin::konst(0)), Some(Lin::konst(0)));
        for k in keys {
            let vs: Vec<(Cnt, Cnt)> = arms.iter().map(|a| a.ops.get(&k).cloned().unwrap_or(zero.clone())).collect();
            let same = |i: usize| -> Cnt {
                let first = if i == 0 { vs[0].0.clone() } else { vs[0].1.clone() };
                if vs.iter().all(|v| (if i == 0 { &v.0 } else { &v.1 }) == &first) { first } else { None }
            };
            b.sum.bump(&k, true, same(0));
            b.sum.bump(&k, false, same(1));
        }
    }

    /// 端點所屬通道的鍵
    fn key_of(&self, b: &Body, ep: &str) -> Option<String> {
        match b.eps.get(ep)? { Ep::Local { key, .. } => Some(key.clone()), Ep::Param(p) => Some(p.clone()) }
    }

    fn spawn(&mut self, b: &mut Body, e: &Expr, _m: &Mult) {
        let (callee, generics, args) = match &e.kind {
            ExprKind::Call(f, args) => match &f.kind {
                ExprKind::Path(p) => (p.last().cloned(), vec![], args),
                ExprKind::PathT(p, g) => (p.last().cloned(), g.clone(), args),
                _ => (None, vec![], args),
            },
            _ => (None, vec![], &vec![]),
        };
        let Some(d) = callee.and_then(|n| self.prog.fns.get(&n).cloned()) else { b.incomplete = true; return };
        // 參數：端點 → 本體的端點名稱；整數 → 常數
        let mut ints = HashMap::new();
        for (g, a) in d.generics.iter().zip(&generics) {
            if let TArg::Ex(x) = a { if let Some(v) = self.const_of(b, x) { ints.insert(g.name.clone(), v); } }
        }
        let mut map: HashMap<String, String> = HashMap::new();
        for (p, a) in d.params.iter().zip(args.iter()) {
            if let Some(ep) = self.ep_name(b, &a.value) { b.moved.entry(ep.clone()).or_insert(e.line); map.insert(p.name.clone(), ep); }
            else if let Some(v) = self.const_of(b, &a.value) { ints.insert(p.name.clone(), v); }
        }
        let Some(kid) = self.summarize(&d, ints) else {
            for ep in map.values() { b.sum.bump(ep, true, None); b.sum.bump(ep, false, None); }
            return;
        };
        // 任務參數上的次數 → 本體的端點；T(參數) → T(端點的通道)
        let keys: HashMap<String, Option<String>> = map.iter().map(|(p, ep)| (p.clone(), self.key_of(b, ep))).collect();
        let mut mapped = Summary::default();
        for (p, (s, r)) in &kid.ops {
            let Some(ep) = map.get(p) else { continue };
            let ren = |c: &Cnt| -> Cnt {
                let c = c.as_ref()?;
                if c.t.keys().any(|x| keys.get(x).is_none_or(|k| k.is_none())) { return None; }
                Some(c.rename(&|x| keys[x].clone().unwrap()))
            };
            mapped.bump(ep, true, ren(s));
            mapped.bump(ep, false, ren(r));
        }
        b.sum.merge(&mapped);
    }

    /// 任務 `d` 在這些常數下，參數端點上的次數（也檢查它內部的網路）
    fn summarize(&mut self, d: &Arc<FnDecl>, ints: HashMap<String, i128>) -> Option<Summary> {
        if d.kind != FnKind::Task || self.depth > 8 { return None; }
        let mut kv: Vec<_> = ints.iter().collect();
        kv.sort();
        let key = format!("{}{:?}", d.name, kv);
        if let Some(s) = self.cache.get(&key) { return s.clone(); }
        self.cache.insert(key.clone(), None);
        let mut b = Body::new(&d.name, ints);
        for p in &d.params {
            if matches!(p.ty, TypeExpr::Chan(..)) { b.eps.insert(p.name.clone(), Ep::Param(p.name.clone())); }
        }
        self.depth += 1;
        self.block(&mut b, &d.body, &Mult(Some((1, None))));
        self.depth -= 1;
        let s = self.solve(&mut b, d.line);
        self.cache.insert(key, s.clone());
        s
    }

    /// 區域通道的守恆方程：代入求解、比對；回傳參數端點上的次數
    fn solve(&mut self, b: &mut Body, _line: u32) -> Option<Summary> {
        // 每條區域通道：送出端與接收端的總次數
        let mut prod: BTreeMap<String, Cnt> = BTreeMap::new();
        let mut cons: BTreeMap<String, Cnt> = BTreeMap::new();
        for (k, c) in &b.chans {
            prod.insert(k.clone(), c.from.as_ref().map(|f| Lin::var(f)).or(Some(Lin::konst(0))));
            cons.insert(k.clone(), Some(Lin::konst(0)));
        }
        let mut params: HashMap<String, (Cnt, Cnt)> = HashMap::new();
        for (ep, (s, r)) in &b.sum.ops {
            match b.eps.get(ep) {
                Some(Ep::Local { key, tx }) => {
                    // 送出端只送、接收端只收；反過來用就是錯（線性檢查負責），這裡照實累加
                    let _ = tx;
                    let p = prod.get_mut(key).unwrap();
                    *p = cadd(p, s);
                    let c = cons.get_mut(key).unwrap();
                    *c = cadd(c, r);
                }
                Some(Ep::Param(p)) => { params.insert(p.clone(), (s.clone(), r.clone())); }
                None => {}
            }
        }
        // 代入求解：一邊不含 T(k) 時，T(k) 就是那一邊
        let keys: Vec<String> = b.chans.keys().cloned().collect();
        let mut val: HashMap<String, Lin> = HashMap::new();
        for _ in 0..keys.len() + 1 {
            let mut progress = false;
            for k in &keys {
                if val.contains_key(k) { continue; }
                let sub = |c: &Cnt| -> Cnt { let mut c = c.clone()?; for (x, v) in &val { c = c.subst(x, v); } Some(c) };
                let (p, c) = (sub(&prod[k]), sub(&cons[k]));
                let v = match (&p, &c) {
                    (Some(p), _) if !p.mentions(k) && p.t.keys().all(|x| !keys.contains(x)) => Some(p.clone()),
                    (_, Some(c)) if !c.mentions(k) && c.t.keys().all(|x| !keys.contains(x)) => Some(c.clone()),
                    _ => None,
                };
                if let Some(v) = v { val.insert(k.clone(), v); progress = true; }
            }
            if !progress { break; }
        }
        if !b.incomplete {
            for k in &keys {
                let Some(v) = val.get(k) else { continue };
                let sub = |c: &Cnt| -> Cnt { let mut c = c.clone()?; for (x, w) in &val { c = c.subst(x, w); } Some(c) };
                let (Some(p), Some(c)) = (sub(&prod[k]), sub(&cons[k])) else { continue };
                let _ = v;
                if p == c { continue; }
                let ch = &b.chans[k];
                let names = |x: &str| b.chans.get(x).map(|c| c.label.clone()).unwrap_or_else(|| x.to_string());
                let (ps, cs) = (p.show(&names), c.show(&names));
                // 收的比送的多、或剩下的超過 FIFO：會卡住（E0501）；剩下的放得進 FIFO：W0503
                let (code, consequence) = match (p.t.is_empty() && c.t.is_empty(), p.c.cmp(&c.c)) {
                    (true, std::cmp::Ordering::Less) => ("E0501", "the receiver waits for tokens nobody sends: a deadlock".to_string()),
                    (true, std::cmp::Ordering::Greater) => match ch.depth {
                        Some(d) if p.c - c.c > d => ("E0501", format!("{} token(s) are never received and the FIFO holds {}: the sender blocks (a deadlock)", p.c - c.c, d)),
                        _ => ("W0503", format!("{} token(s) are left in the channel, never received", p.c - c.c)),
                    },
                    _ => ("E0501", "the counts differ for some inputs".to_string()),
                };
                self.out.push(Finding {
                    code,
                    line: ch.line,
                    func: b.func.clone(),
                    msg: format!("channel `{}`: {} token(s) sent, {} received: {}", ch.label, ps, cs, consequence),
                    data: vec![("sent".into(), ps), ("received".into(), cs)],
                    fix: Some("make the counts agree, or receive with `for v in rx` (until the sender closes)".into()),
                    min_type: None,
                });
            }
        }
        // 參數上的次數：代入區域通道的值；還含區域通道就是未知
        let mut s = Summary::default();
        for (p, (snd, rcv)) in params {
            let sub = |c: &Cnt| -> Cnt {
                let mut c = c.clone()?;
                for (x, w) in &val { c = c.subst(x, w); }
                if c.t.keys().any(|x| keys.contains(x)) { None } else { Some(c) }
            };
            s.bump(&p, true, sub(&snd));
            s.bump(&p, false, sub(&rcv));
        }
        if b.incomplete { None } else { Some(s) }
    }
}

impl Body {
    fn new(func: &str, ints: HashMap<String, i128>) -> Body {
        Body { func: func.to_string(), ints, lens: HashMap::new(), eps: HashMap::new(), arrays: HashMap::new(), chans: BTreeMap::new(), sum: Summary::default(), incomplete: false, moved: HashMap::new() }
    }
}

/// `chan::<T>(depth = d)` → Some(深度的運算式)
fn chan_call(e: &Expr) -> Option<Option<&Expr>> {
    let ExprKind::Call(f, args) = &e.kind else { return None };
    match &f.kind {
        ExprKind::PathT(p, _) | ExprKind::Path(p) if p.last().is_some_and(|n| n == "chan") => {
            Some(args.iter().find(|a| a.name.as_deref() == Some("depth")).or(args.first()).map(|a| &a.value))
        }
        _ => None,
    }
}

/// E0501：每個 test 與每個 spawn 其他任務的 task
pub fn rates(prog: &Arc<Program>) -> Vec<Finding> {
    let mut r = Rates { prog, out: vec![], cache: HashMap::new(), depth: 0 };
    let mut fns: Vec<&Arc<FnDecl>> = prog.fns.values().filter(|d| d.kind == FnKind::Task).collect();
    fns.sort_by_key(|d| d.line);
    for d in fns {
        // 參數未知的任務：只有它自己建的網路有意義
        let _ = r.summarize(d, HashMap::new());
    }
    for t in &prog.tests {
        let mut b = Body::new(&format!("test {}", t.name), HashMap::new());
        r.block(&mut b, &t.body, &Mult(Some((1, None))));
        let _ = r.solve(&mut b, t.line);
    }
    r.out
}

// ====================================================================== E0401

/// E0401：`pipeline(ii = n)` 迴圈每一輪的資源使用
pub fn ports(prog: &Arc<Program>) -> Vec<Finding> {
    let mut out = vec![];
    let mut fns: Vec<&Arc<FnDecl>> = prog.fns.values().collect();
    fns.sort_by_key(|d| d.line);
    for d in fns {
        let mut mems: Vec<String> = d.params.iter().filter(|p| matches!(p.ty, TypeExpr::Mem(..))).map(|p| p.name.clone()).collect();
        let chans: Vec<String> = d.params.iter().filter(|p| matches!(p.ty, TypeExpr::Chan(..))).map(|p| p.name.clone()).collect();
        mem_decls(&d.body, &mut mems);
        find_pipelines(prog, &d.name, &d.body, &mems, &chans, &mut out);
    }
    out
}

fn mem_decls(b: &Block, out: &mut Vec<String>) {
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Let { pat: Pattern::Ident(n), ty: Some(TypeExpr::Mem(..)), .. } => out.push(n.clone()),
            StmtKind::For { body, .. } => mem_decls(body, out),
            _ => {}
        }
    }
}

fn find_pipelines(prog: &Arc<Program>, func: &str, b: &Block, mems: &[String], chans: &[String], out: &mut Vec<Finding>) {
    for s in &b.stmts {
        if let StmtKind::For { attr, body, .. } = &s.kind {
            if let LoopAttr::Pipeline(kv) = attr {
                let ii = kv.iter().find(|(k, _)| k == "ii").and_then(|(_, e)| if let ExprKind::Int(n) = e.kind { Some(n) } else { None }).unwrap_or(1);
                let mut uses: BTreeMap<(String, &'static str), (i128, Vec<u32>)> = BTreeMap::new();
                count_uses(prog, body, 1, mems, chans, &mut uses);
                for ((res, class), (n, lines)) in uses {
                    if n <= ii { continue; }
                    let what = if class == "read" || class == "write" { format!("memory `{}` ({} port: 1 per clock)", res, class) } else { format!("channel `{}` (1 token per clock)", res) };
                    let mut ls: Vec<String> = lines.iter().map(|l| (l % FILE_STRIDE).to_string()).collect();
                    ls.dedup();
                    out.push(Finding {
                        code: "E0401",
                        line: s.line,
                        func: func.to_string(),
                        msg: format!("pipeline(ii = {}): {} {}s an iteration on {}, more than {} (lines {})", ii, n, class, what, ii, ls.join(", ")),
                        data: vec![("resource".into(), res.clone()), ("uses".into(), n.to_string()), ("capacity".into(), ii.to_string())],
                        fix: Some(format!("use pipeline(ii = {}), or spread the accesses over iterations", n)),
                        min_type: None,
                    });
                }
            } else {
                find_pipelines(prog, func, body, mems, chans, out);
            }
        }
    }
}

/// 一輪裡每個資源的使用次數（內層的常數迴圈會被展開：乘上次數；分支都算，因為硬體兩邊都排）
fn count_uses<'b>(prog: &Arc<Program>, b: &Block, k: i128, mems: &[String], chans: &[String], uses: &mut BTreeMap<(String, &'static str), (i128, Vec<u32>)>) {
    let add = |res: &str, class: &'static str, line: u32, uses: &mut BTreeMap<(String, &'static str), (i128, Vec<u32>)>| {
        let e = uses.entry((res.to_string(), class)).or_insert((0, vec![]));
        e.0 += k;
        e.1.push(line);
    };
    let visit = |e: &Expr, uses: &mut BTreeMap<(String, &'static str), (i128, Vec<u32>)>| {
        walk(e, &mut |x| match &x.kind {
            ExprKind::Method(c, n, _, _) if n == "recv" || n == "send" => {
                if let ExprKind::Path(p) = &c.kind { if chans.contains(&p[0]) { add(&p[0], if n == "recv" { "recv" } else { "send" }, x.line, uses); } }
            }
            ExprKind::Index(a, _) => {
                if let ExprKind::Path(p) = &a.kind { if mems.contains(&p[0]) { add(&p[0], "read", x.line, uses); } }
            }
            _ => {}
        });
    };
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Assign { target, value, .. } => {
                visit(value, uses);
                if let ExprKind::Index(a, i) = &target.kind {
                    if let ExprKind::Path(p) = &a.kind {
                        if mems.contains(&p[0]) {
                            let e = uses.entry((p[0].clone(), "write")).or_insert((0, vec![]));
                            e.0 += k;
                            e.1.push(s.line);
                            visit(i, uses);
                            continue;
                        }
                    }
                }
                visit(target, uses);
            }
            StmtKind::For { iter, body, .. } => {
                // 內層迴圈：常數次數就展開
                let n = match &iter.kind {
                    ExprKind::Range(a, z, incl) => match (&a.kind, &z.kind) {
                        (ExprKind::Int(a), ExprKind::Int(z)) => Some(z - a + if *incl { 1 } else { 0 }),
                        _ => const_int(prog, a).zip(const_int(prog, z)).map(|(a, z)| z - a + if *incl { 1 } else { 0 }),
                    },
                    _ => None,
                };
                if let Some(n) = n { count_uses(prog, body, k * n.max(0), mems, chans, uses); }
            }
            StmtKind::Let { init: Some(e), .. } | StmtKind::Carry { init: e, .. } | StmtKind::Expr(e) | StmtKind::Return(Some(e)) => visit(e, uses),
            StmtKind::Assert { cond, .. } => visit(cond, uses),
            _ => {}
        }
    }
    if let Some(t) = &b.tail { visit(t, uses); }
}

fn const_int(prog: &Arc<Program>, e: &Expr) -> Option<i128> {
    match &e.kind {
        ExprKind::Int(n) => Some(*n),
        ExprKind::Path(p) if p.len() == 1 => match prog.consts.read().ok()?.get(&p[0]) { Some(Value::Int(n)) => Some(*n), _ => None },
        _ => None,
    }
}

/// 運算式樹的每個節點，包括區塊裡的陳述式
fn walk(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    let blk = |b: &Block, f: &mut dyn FnMut(&Expr)| {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let { init: Some(x), .. } | StmtKind::Carry { init: x, .. } | StmtKind::Expr(x) | StmtKind::Return(Some(x)) | StmtKind::Spawn(x) => walk(x, f),
                StmtKind::Assign { target, value, .. } => { walk(target, f); walk(value, f) }
                StmtKind::Assert { cond, .. } => walk(cond, f),
                _ => {}
            }
        }
        if let Some(t) = &b.tail { walk(t, f) }
    };
    match &e.kind {
        ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Field(x, _) => walk(x, f),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b, _) | ExprKind::Repeat(a, b) => { walk(a, f); walk(b, f) }
        ExprKind::Slice(a, b, c) => { walk(a, f); walk(b, f); walk(c, f) }
        ExprKind::Call(x, args) | ExprKind::Method(x, _, _, args) => { walk(x, f); for a in args { walk(&a.value, f) } }
        ExprKind::Array(xs) | ExprKind::Tuple(xs) => for x in xs { walk(x, f) },
        ExprKind::If(c, b, els) => { walk(c, f); blk(b, f); if let Some(x) = els { walk(x, f) } }
        ExprKind::Match(x, arms) => { walk(x, f); for a in arms { walk(&a.body, f) } }
        ExprKind::Block(b) => blk(b, f),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    fn codes(src: &str) -> Vec<String> {
        let p = crate::load("t.fx", src).unwrap();
        let mut v: Vec<String> = super::rates(&p).into_iter().chain(super::ports(&p)).map(|f| f.code.to_string()).collect();
        v.sort();
        v
    }

    const TASKS: &str = "
        task Gen(n: u32, o: out chan<u8>) { for i in 0..n { o.send(1); } }
        task Pass(x: in chan<u8>, y: out chan<u8>) { for v in x { y.send(v); } }
        task Pair(x: in chan<u8>, y: out chan<u8>) { for v in x { let w = x.recv(); y.send(v + w); } }
        task Total(x: in chan<u8>, y: out chan<u16>) { carry s: u16 = 0; for v in x { s += v; } y.send(s); }";

    #[test]
    fn balanced_networks() {
        // 生產 10、經過 Pass、broadcast 兩路、各自收完
        let t = format!("{} test t {{ let (a, ra) = chan::<u8>(depth = 2); let (b, rb) = chan::<u8>(depth = 2);
            let (c, rc) = chan::<u16>(depth = 1); spawn Gen(10, a); spawn Pass(ra, b); let o = broadcast(rb, 2);
            spawn Total(o[0], c); let (d, rd) = chan::<u16>(depth = 1); spawn Total(o[1], d);
            let s = rc.recv(); let u = rd.recv(); }}", TASKS);
        assert!(codes(&t).is_empty(), "{:?}", codes(&t));
    }

    #[test]
    fn mismatches() {
        // 收 11 個、只送 10 個 → 死結
        let t = format!("{} test t {{ let (a, ra) = chan::<u8>(depth = 2); spawn Gen(10, a); for _ in 0..11 {{ let v = ra.recv(); }} }}", TASKS);
        assert_eq!(codes(&t), vec!["E0501"]);
        // Pair 一輪收 2 個、送 1 個：送 5 個進去，收 5 個出來 → 收不到
        let t = format!("{} test t {{ let (a, ra) = chan::<u8>(depth = 8); let (b, rb) = chan::<u8>(depth = 8);
            spawn Pair(ra, b); for _ in 0..4 {{ a.send(1); }} a.close(); for _ in 0..4 {{ let v = rb.recv(); }} }}", TASKS);
        assert_eq!(codes(&t), vec!["E0501"], "a 2:1 task");
        // 剩 1 個在 FIFO 裡：W0503；剩 5 個、FIFO 只有 2：E0501
        let t = format!("{} test t {{ let (a, ra) = chan::<u8>(depth = 2); spawn Gen(10, a); for _ in 0..9 {{ let v = ra.recv(); }} }}", TASKS);
        assert_eq!(codes(&t), vec!["W0503"]);
        let t = format!("{} test t {{ let (a, ra) = chan::<u8>(depth = 2); spawn Gen(10, a); for _ in 0..5 {{ let v = ra.recv(); }} }}", TASKS);
        assert_eq!(codes(&t), vec!["E0501"]);
        // spawn 之後再用端點
        let t = format!("{} test t {{ let (a, ra) = chan::<u8>(depth = 2); let (b, rb) = chan::<u8>(depth = 2); spawn Pass(ra, b); a.send(1); a.close(); let v = ra.recv(); let w = rb.recv(); }}", TASKS);
        assert!(codes(&t).contains(&"E0702".to_string()), "{:?}", codes(&t));
    }

    #[test]
    fn data_dependent_counts_are_not_checked() {
        let t = "task F(x: in chan<u8>, y: out chan<u8>) { for v in x { if v > 3 { y.send(v); } } }
                 test t { let (a, ra) = chan::<u8>(depth = 4); let (b, rb) = chan::<u8>(depth = 4);
                 spawn F(ra, b); a.send(5); a.close(); let v = rb.recv(); }";
        assert!(codes(t).is_empty(), "{:?}", codes(t));
    }

    #[test]
    fn pipeline_ports() {
        // 同一個通道一輪收兩次：ii = 1 不夠，ii = 2 可以
        assert_eq!(codes("task P(x: in chan<u8>, y: out chan<u9>, n: u16) { pipeline(ii = 1) for _ in 0..n { let a = x.recv(); let b = x.recv(); y.send(a + b); } }"), vec!["E0401"]);
        assert!(codes("task P(x: in chan<u8>, y: out chan<u9>, n: u16) { pipeline(ii = 2) for _ in 0..n { let a = x.recv(); let b = x.recv(); y.send(a + b); } }").is_empty());
        // 一塊 block RAM 一個讀埠：展開 4 次的讀
        assert_eq!(codes("task M(y: out chan<u10>, n: u16) { let mut m: mem<u8, 16> = [0; 16];
            pipeline(ii = 1) for i in 0..n { carry s: u10 = 0; for k in 0..4 { s += m[k]; } y.send(s); } }"), vec!["E0401"]);
    }
}
