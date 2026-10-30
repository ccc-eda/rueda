//! 遞迴下降 + 優先序爬升語法分析器（EBNF 見規格 §2）。
//!
//! 行為模擬器支援的子集：`const`、`fn`/`comb fn`/`comptime fn`/`task`、`test`，
//! 以及完整的陳述式與運算式。`iface` / `schedule` / `type` / `use` 會被解析並略過；
//! `stage` 層（週期精確）不在行為模擬範圍內，遇到時報錯。

use crate::ast::*;
use crate::lexer::{lex_from, FxError, Res, Tok, Token};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    /// >0 時 `>` 與 `>>` 不當作運算子（處理泛型括號）
    no_gt: u32,
}

pub fn parse_program(file: &str, src: &str) -> Res<Program> {
    parse_program_at(file, src, 0)
}

/// `file_idx`：此檔案在載入清單中的編號（用於還原錯誤位置）
pub fn parse_program_at(file: &str, src: &str, file_idx: u32) -> Res<Program> {
    let toks = lex_from(src, file_idx * crate::ast::FILE_STRIDE)?;
    let mut p = Parser { src, toks, pos: 0, no_gt: 0 };
    let mut consts = Vec::new();
    let mut fns: HashMap<String, Arc<FnDecl>> = HashMap::new();
    let mut tests = Vec::new();

    loop {
        if p.is_eof() {
            break;
        }
        p.eat_kw("pub");
        let line = p.line();
        let kw = match p.peek().clone() {
            Tok::Ident(s) => s,
            other => return p.err(format!("unexpected token {:?} at top level", other)),
        };
        match kw.as_str() {
            "use" | "type" => p.skip_until_semi()?,
            "const" => consts.push(p.parse_const()?),
            "fn" => {
                p.next();
                let d = p.parse_fn(FnKind::Fn, line)?;
                fns.insert(d.name.clone(), Arc::new(d));
            }
            "comb" => {
                p.next();
                p.expect_kw("fn")?;
                let d = p.parse_fn(FnKind::Comb, line)?;
                fns.insert(d.name.clone(), Arc::new(d));
            }
            "comptime" => {
                p.next();
                p.expect_kw("fn")?;
                let d = p.parse_fn(FnKind::Comptime, line)?;
                fns.insert(d.name.clone(), Arc::new(d));
            }
            "task" => {
                p.next();
                let d = p.parse_fn(FnKind::Task, line)?;
                fns.insert(d.name.clone(), Arc::new(d));
            }
            "stage" => {
                return p.err(
                    "`stage` (cycle-accurate layer) is not supported by the behavioral simulator"
                        .to_string(),
                )
            }
            "iface" | "schedule" => p.skip_item_braces()?,
            "test" => {
                p.next();
                let name = p.expect_ident()?;
                let body = p.parse_block()?;
                tests.push(TestDecl { name, body, line });
            }
            other => return p.err(format!("unexpected `{}` at top level", other)),
        }
    }

    Ok(Program {
        files: vec![file.to_string()],
        consts_decl: consts,
        fns,
        tests,
        consts: RwLock::new(HashMap::new()),
    })
}

impl<'a> Parser<'a> {
    // ------------------------------------------------------------ 基礎
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn peek_at(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].tok
    }
    fn line(&self) -> u32 {
        self.toks[self.pos].line
    }
    fn is_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }
    fn next(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn err<T>(&self, msg: String) -> Res<T> {
        let t = &self.toks[self.pos];
        Err(FxError { line: t.line, col: t.col, msg })
    }
    fn is_p(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::P(p) if *p == s)
    }
    fn is_p_at(&self, n: usize, s: &str) -> bool {
        matches!(self.peek_at(n), Tok::P(p) if *p == s)
    }
    fn eat_p(&mut self, s: &str) -> bool {
        if self.is_p(s) {
            self.next();
            true
        } else {
            false
        }
    }
    fn expect_p(&mut self, s: &str) -> Res<()> {
        if self.eat_p(s) {
            Ok(())
        } else {
            self.err(format!("expected `{}`, found {}", s, self.describe()))
        }
    }
    fn describe(&self) -> String {
        match self.peek() {
            Tok::Ident(s) => format!("`{}`", s),
            Tok::Int(n) => format!("`{}`", n),
            Tok::Sized(w, v) => format!("`{}'d{}`", w, v),
            Tok::Float(x) => format!("`{}`", x),
            Tok::Str(s) => format!("\"{}\"", s),
            Tok::P(p) => format!("`{}`", p),
            Tok::Eof => "end of file".to_string(),
        }
    }
    fn is_kw(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Ident(i) if i == s)
    }
    fn is_kw_at(&self, n: usize, s: &str) -> bool {
        matches!(self.peek_at(n), Tok::Ident(i) if i == s)
    }
    fn eat_kw(&mut self, s: &str) -> bool {
        if self.is_kw(s) {
            self.next();
            true
        } else {
            false
        }
    }
    fn expect_kw(&mut self, s: &str) -> Res<()> {
        if self.eat_kw(s) {
            Ok(())
        } else {
            self.err(format!("expected `{}`, found {}", s, self.describe()))
        }
    }
    fn expect_ident(&mut self) -> Res<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.next();
                Ok(s)
            }
            _ => self.err(format!("expected identifier, found {}", self.describe())),
        }
    }
    /// 期望 `>`；若遇到 `>>` 就拆成兩個 `>`
    fn expect_gt(&mut self) -> Res<()> {
        if self.is_p(">") {
            self.next();
            return Ok(());
        }
        if self.is_p(">>") {
            self.toks[self.pos].tok = Tok::P(">");
            return Ok(());
        }
        self.err(format!("expected `>`, found {}", self.describe()))
    }

    fn skip_until_semi(&mut self) -> Res<()> {
        while !self.is_p(";") {
            if self.is_eof() {
                return self.err("unexpected end of file".to_string());
            }
            self.next();
        }
        self.next();
        Ok(())
    }
    fn skip_balanced(&mut self, open: &str, close: &str) -> Res<()> {
        self.expect_p(open)?;
        let mut depth = 1;
        while depth > 0 {
            if self.is_eof() {
                return self.err(format!("unbalanced `{}`", open));
            }
            if self.is_p(open) {
                depth += 1;
            } else if self.is_p(close) {
                depth -= 1;
            }
            self.next();
        }
        Ok(())
    }
    fn skip_item_braces(&mut self) -> Res<()> {
        while !self.is_p("{") {
            if self.is_eof() {
                return self.err("unexpected end of file".to_string());
            }
            self.next();
        }
        self.skip_balanced("{", "}")
    }

    // ------------------------------------------------------------ 宣告
    fn parse_const(&mut self) -> Res<ConstDecl> {
        let line = self.line();
        self.expect_kw("const")?;
        let name = self.expect_ident()?;
        let ty = if self.eat_p(":") { Some(self.parse_type()?) } else { None };
        self.expect_p("=")?;
        let value = self.parse_expr()?;
        self.expect_p(";")?;
        Ok(ConstDecl { name, ty, value, line })
    }

    fn parse_fn(&mut self, kind: FnKind, line: u32) -> Res<FnDecl> {
        let name = self.expect_ident()?;
        let mut generics = Vec::new();
        if self.eat_p("<") {
            self.no_gt += 1;
            loop {
                if self.is_p(">") || self.is_p(">>") {
                    break;
                }
                let is_const = self.eat_kw("const");
                let gname = self.expect_ident()?;
                if self.eat_p(":") {
                    self.parse_type()?;
                }
                generics.push(Generic { name: gname, is_const });
                if !self.eat_p(",") {
                    break;
                }
            }
            self.no_gt -= 1;
            self.expect_gt()?;
        }
        self.expect_p("(")?;
        let mut params = Vec::new();
        while !self.is_p(")") {
            let pname = self.expect_ident()?;
            self.expect_p(":")?;
            // `in` / `out` 方向標記僅供文件用途；行為由通道端點型別決定
            if self.is_kw("in") || self.is_kw("out") {
                self.next();
            }
            let ty = self.parse_type()?;
            params.push(Param { name: pname, ty });
            if !self.eat_p(",") {
                break;
            }
        }
        self.expect_p(")")?;
        let ret = if self.eat_p("->") { Some(self.parse_type()?) } else { None };
        let body = self.parse_block()?;
        Ok(FnDecl { name, kind, generics, params, ret, body, line })
    }

    // ------------------------------------------------------------ 型別
    fn parse_ng(&mut self) -> Res<Expr> {
        self.no_gt += 1;
        let r = self.parse_or();
        self.no_gt -= 1;
        r
    }

    fn is_type_start(&self) -> bool {
        match self.peek() {
            Tok::Ident(s) => {
                matches!(s.as_str(), "fixed" | "int" | "bits" | "bit" | "bool" | "chan" | "mem")
                    || crate::value::parse_int_type(s).is_some()
            }
            Tok::P("[") => true,
            _ => false,
        }
    }

    pub fn parse_type(&mut self) -> Res<TypeExpr> {
        if self.eat_p("[") {
            let t = self.parse_type()?;
            self.expect_p(";")?;
            let saved = self.no_gt;
            self.no_gt = 0;
            let n = self.parse_expr()?;
            self.no_gt = saved;
            self.expect_p("]")?;
            return Ok(TypeExpr::Array(Box::new(t), Box::new(n)));
        }
        if self.eat_p("(") {
            let mut ts = Vec::new();
            while !self.is_p(")") {
                ts.push(self.parse_type()?);
                if !self.eat_p(",") {
                    break;
                }
            }
            self.expect_p(")")?;
            return Ok(TypeExpr::Tuple(ts));
        }
        let name = self.expect_ident()?;
        match name.as_str() {
            "fixed" => {
                self.expect_p("<")?;
                let i = self.parse_ng()?;
                self.expect_p(",")?;
                let f = self.parse_ng()?;
                let mut opts = Vec::new();
                while self.eat_p(",") {
                    let k = self.expect_ident()?;
                    self.expect_p("=")?;
                    let v = self.expect_ident()?;
                    opts.push((k, v));
                }
                self.expect_gt()?;
                Ok(TypeExpr::Fixed(Box::new(i), Box::new(f), opts))
            }
            "int" => {
                self.expect_p("<")?;
                let lo = self.parse_ng()?;
                self.expect_p("..=")?;
                let hi = self.parse_ng()?;
                self.expect_gt()?;
                Ok(TypeExpr::Range(Box::new(lo), Box::new(hi)))
            }
            "bits" => {
                self.expect_p("<")?;
                let n = self.parse_ng()?;
                self.expect_gt()?;
                Ok(TypeExpr::Bits(Box::new(n)))
            }
            "chan" => {
                self.expect_p("<")?;
                let t = self.parse_type()?;
                let mut depth = None;
                if self.eat_p(",") {
                    let k = self.expect_ident()?;
                    if k != "depth" {
                        return self.err(format!("unknown chan option `{}`", k));
                    }
                    self.expect_p("=")?;
                    depth = Some(Box::new(self.parse_ng()?));
                }
                self.expect_gt()?;
                Ok(TypeExpr::Chan(Box::new(t), depth))
            }
            "mem" => {
                self.expect_p("<")?;
                let t = self.parse_type()?;
                self.expect_p(",")?;
                let n = self.parse_ng()?;
                self.expect_gt()?;
                Ok(TypeExpr::Mem(Box::new(t), Box::new(n)))
            }
            _ => {
                let primitive = matches!(name.as_str(), "bit" | "bool")
                    || crate::value::parse_int_type(&name).is_some();
                let mut path = vec![name];
                let mut targs = Vec::new();
                if !primitive {
                    while self.is_p("::") && !self.is_p_at(1, "<") {
                        self.next();
                        path.push(self.expect_ident()?);
                    }
                    if self.is_p("<") {
                        self.next();
                        targs = self.parse_targ_list()?;
                    }
                }
                Ok(TypeExpr::Name(path, targs))
            }
        }
    }

    /// 已消耗開頭的 `<`
    fn parse_targ_list(&mut self) -> Res<Vec<TArg>> {
        self.no_gt += 1;
        let mut out = Vec::new();
        loop {
            if self.is_p(">") || self.is_p(">>") {
                break;
            }
            if self.is_type_start() {
                out.push(TArg::Ty(self.parse_type()?));
            } else {
                out.push(TArg::Ex(self.parse_or()?));
            }
            if !self.eat_p(",") {
                break;
            }
        }
        self.no_gt -= 1;
        self.expect_gt()?;
        Ok(out)
    }

    // ------------------------------------------------------------ 陳述式
    pub fn parse_block(&mut self) -> Res<Block> {
        self.expect_p("{")?;
        let saved = self.no_gt;
        self.no_gt = 0;
        let mut stmts = Vec::new();
        let mut tail = None;
        loop {
            if self.eat_p("}") {
                break;
            }
            if self.eat_p(";") {
                continue;
            }
            if self.is_eof() {
                return self.err("unexpected end of file, expected `}`".to_string());
            }
            match self.parse_stmt()? {
                Parsed::Stmt(s) => stmts.push(s),
                Parsed::Tail(e) => {
                    tail = Some(Box::new(e));
                    self.expect_p("}")?;
                    break;
                }
            }
        }
        self.no_gt = saved;
        Ok(Block { stmts, tail })
    }

    fn parse_pattern(&mut self) -> Res<Pattern> {
        if self.eat_p("(") {
            let mut ps = Vec::new();
            while !self.is_p(")") {
                ps.push(self.parse_pattern()?);
                if !self.eat_p(",") {
                    break;
                }
            }
            self.expect_p(")")?;
            return Ok(Pattern::Tuple(ps));
        }
        let n = self.expect_ident()?;
        Ok(if n == "_" { Pattern::Wild } else { Pattern::Ident(n) })
    }

    fn parse_kv_list(&mut self) -> Res<Vec<(String, Expr)>> {
        self.expect_p("(")?;
        let mut out = Vec::new();
        while !self.is_p(")") {
            let k = self.expect_ident()?;
            self.expect_p("=")?;
            let v = self.parse_expr()?;
            out.push((k, v));
            if !self.eat_p(",") {
                break;
            }
        }
        self.expect_p(")")?;
        Ok(out)
    }

    fn parse_stmt(&mut self) -> Res<Parsed> {
        let line = self.line();
        let mk = |kind| Ok(Parsed::Stmt(Stmt { kind, line }));

        if self.is_p(";;") {
            return self.err("`;;` (stage separator) is not supported by the behavioral simulator".to_string());
        }

        if let Tok::Ident(kw) = self.peek().clone() {
            match kw.as_str() {
                "let" => {
                    self.next();
                    self.eat_kw("mut");
                    let pat = self.parse_pattern()?;
                    let mut ty = None;
                    let mut placement = None;
                    if self.eat_p(":") {
                        ty = Some(self.parse_type()?);
                        if self.eat_p("@") {
                            placement = Some(self.expect_ident()?);
                            if self.is_p("(") {
                                self.skip_balanced("(", ")")?;
                            }
                        }
                    }
                    let init = if self.eat_p("=") { Some(self.parse_expr()?) } else { None };
                    self.expect_p(";")?;
                    return mk(StmtKind::Let { pat, ty, placement, init });
                }
                "carry" => {
                    self.next();
                    let name = self.expect_ident()?;
                    self.expect_p(":")?;
                    let ty = self.parse_type()?;
                    self.expect_p("=")?;
                    let init = self.parse_expr()?;
                    self.expect_p(";")?;
                    return mk(StmtKind::Carry { name, ty, init });
                }
                "spawn" => {
                    self.next();
                    let e = self.parse_expr()?;
                    self.expect_p(";")?;
                    return mk(StmtKind::Spawn(e));
                }
                "parallel" if self.is_p_at(1, "{") => {
                    self.next();
                    let b = self.parse_block()?;
                    return mk(StmtKind::Parallel(b.stmts));
                }
                "return" => {
                    self.next();
                    let e = if self.is_p(";") || self.is_p("}") { None } else { Some(self.parse_expr()?) };
                    self.eat_p(";");
                    return mk(StmtKind::Return(e));
                }
                "assert" => {
                    self.next();
                    if self.is_kw("always") || self.is_kw("eventually") {
                        self.next();
                    }
                    let s0 = self.toks[self.pos].start;
                    let cond = self.parse_expr()?;
                    let e0 = self.toks[self.pos - 1].end;
                    let text = self.src[s0..e0].trim().to_string();
                    let msg = if self.eat_p(",") { Some(self.parse_expr()?) } else { None };
                    self.expect_p(";")?;
                    return mk(StmtKind::Assert { cond, text, msg });
                }
                "pipeline" | "unroll" | "serial" | "for" => return self.parse_loop(line),
                _ => {}
            }
            // 標籤迴圈 `name: pipeline(..) for ...`
            if self.is_p_at(1, ":")
                && (self.is_kw_at(2, "pipeline")
                    || self.is_kw_at(2, "unroll")
                    || self.is_kw_at(2, "serial")
                    || self.is_kw_at(2, "for"))
            {
                self.next();
                self.next();
                return self.parse_loop(line);
            }
        }

        // 區塊式運算式陳述式
        if self.is_kw("if") || self.is_kw("match") || self.is_p("{") || self.is_kw("comptime") {
            let e = self.parse_primary()?;
            if self.is_p("}") {
                return Ok(Parsed::Tail(e));
            }
            self.eat_p(";");
            return mk(StmtKind::Expr(e));
        }

        let e = self.parse_expr()?;
        let assign_op = match self.peek() {
            Tok::P("=") => Some(None),
            Tok::P("+=") => Some(Some(BinOp::Add)),
            Tok::P("-=") => Some(Some(BinOp::Sub)),
            Tok::P("*=") => Some(Some(BinOp::Mul)),
            Tok::P("/=") => Some(Some(BinOp::Div)),
            Tok::P("&=") => Some(Some(BinOp::BitAnd)),
            Tok::P("|=") => Some(Some(BinOp::BitOr)),
            Tok::P("^=") => Some(Some(BinOp::BitXor)),
            Tok::P("<<=") => Some(Some(BinOp::Shl)),
            Tok::P(">>=") => Some(Some(BinOp::Shr)),
            _ => None,
        };
        if let Some(op) = assign_op {
            self.next();
            let value = self.parse_expr()?;
            self.expect_p(";")?;
            return mk(StmtKind::Assign { target: e, op, value });
        }
        if self.eat_p(";") {
            return mk(StmtKind::Expr(e));
        }
        if self.is_p("}") {
            return Ok(Parsed::Tail(e));
        }
        self.err(format!("expected `;`, found {}", self.describe()))
    }

    fn parse_loop(&mut self, line: u32) -> Res<Parsed> {
        let attr = if self.eat_kw("pipeline") {
            LoopAttr::Pipeline(if self.is_p("(") { self.parse_kv_list()? } else { vec![] })
        } else if self.eat_kw("unroll") {
            if self.eat_p("(") {
                let e = self.parse_expr()?;
                self.expect_p(")")?;
                LoopAttr::Unroll(Some(e))
            } else {
                LoopAttr::Unroll(None)
            }
        } else if self.eat_kw("serial") {
            LoopAttr::Serial
        } else {
            LoopAttr::None
        };
        self.expect_kw("for")?;
        let pat = self.parse_pattern()?;
        self.expect_kw("in")?;
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Parsed::Stmt(Stmt { kind: StmtKind::For { attr, pat, iter, body }, line }))
    }

    // ------------------------------------------------------------ 運算式
    pub fn parse_expr(&mut self) -> Res<Expr> {
        self.parse_range()
    }

    fn mk(&self, kind: ExprKind, line: u32) -> Expr {
        Expr { kind, line }
    }

    fn parse_range(&mut self) -> Res<Expr> {
        let lo = self.parse_or()?;
        if self.is_p("..") || self.is_p("..=") {
            let line = lo.line;
            let incl = self.is_p("..=");
            self.next();
            let hi = self.parse_or()?;
            return Ok(self.mk(ExprKind::Range(Box::new(lo), Box::new(hi), incl), line));
        }
        Ok(lo)
    }

    fn parse_or(&mut self) -> Res<Expr> {
        let mut l = self.parse_and()?;
        while self.is_p("||") {
            self.next();
            let r = self.parse_and()?;
            let line = l.line;
            l = self.mk(ExprKind::Binary(BinOp::Or, Box::new(l), Box::new(r)), line);
        }
        Ok(l)
    }
    fn parse_and(&mut self) -> Res<Expr> {
        let mut l = self.parse_cmp()?;
        while self.is_p("&&") {
            self.next();
            let r = self.parse_cmp()?;
            let line = l.line;
            l = self.mk(ExprKind::Binary(BinOp::And, Box::new(l), Box::new(r)), line);
        }
        Ok(l)
    }
    fn parse_cmp(&mut self) -> Res<Expr> {
        let l = self.parse_bor()?;
        let op = match self.peek() {
            Tok::P("==") => Some(BinOp::Eq),
            Tok::P("!=") => Some(BinOp::Ne),
            Tok::P("<") => Some(BinOp::Lt),
            Tok::P("<=") => Some(BinOp::Le),
            Tok::P(">") if self.no_gt == 0 => Some(BinOp::Gt),
            Tok::P(">=") if self.no_gt == 0 => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.next();
            let r = self.parse_bor()?;
            let line = l.line;
            return Ok(self.mk(ExprKind::Binary(op, Box::new(l), Box::new(r)), line));
        }
        Ok(l)
    }
    fn parse_bor(&mut self) -> Res<Expr> {
        let mut l = self.parse_bxor()?;
        while self.is_p("|") {
            self.next();
            let r = self.parse_bxor()?;
            let line = l.line;
            l = self.mk(ExprKind::Binary(BinOp::BitOr, Box::new(l), Box::new(r)), line);
        }
        Ok(l)
    }
    fn parse_bxor(&mut self) -> Res<Expr> {
        let mut l = self.parse_band()?;
        while self.is_p("^") {
            self.next();
            let r = self.parse_band()?;
            let line = l.line;
            l = self.mk(ExprKind::Binary(BinOp::BitXor, Box::new(l), Box::new(r)), line);
        }
        Ok(l)
    }
    fn parse_band(&mut self) -> Res<Expr> {
        let mut l = self.parse_shift()?;
        while self.is_p("&") {
            self.next();
            let r = self.parse_shift()?;
            let line = l.line;
            l = self.mk(ExprKind::Binary(BinOp::BitAnd, Box::new(l), Box::new(r)), line);
        }
        Ok(l)
    }
    fn parse_shift(&mut self) -> Res<Expr> {
        let mut l = self.parse_add()?;
        loop {
            let op = match self.peek() {
                Tok::P("<<") => BinOp::Shl,
                Tok::P(">>") if self.no_gt == 0 => BinOp::Shr,
                _ => break,
            };
            self.next();
            let r = self.parse_add()?;
            let line = l.line;
            l = self.mk(ExprKind::Binary(op, Box::new(l), Box::new(r)), line);
        }
        Ok(l)
    }
    fn parse_add(&mut self) -> Res<Expr> {
        let mut l = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Tok::P("+") => BinOp::Add,
                Tok::P("-") => BinOp::Sub,
                Tok::P("+%") => BinOp::AddW,
                Tok::P("-%") => BinOp::SubW,
                _ => break,
            };
            self.next();
            let r = self.parse_mul()?;
            let line = l.line;
            l = self.mk(ExprKind::Binary(op, Box::new(l), Box::new(r)), line);
        }
        Ok(l)
    }
    fn parse_mul(&mut self) -> Res<Expr> {
        let mut l = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Tok::P("*") => BinOp::Mul,
                Tok::P("/") => BinOp::Div,
                Tok::P("%") => BinOp::Rem,
                Tok::P("*%") => BinOp::MulW,
                _ => break,
            };
            self.next();
            let r = self.parse_unary()?;
            let line = l.line;
            l = self.mk(ExprKind::Binary(op, Box::new(l), Box::new(r)), line);
        }
        Ok(l)
    }
    fn parse_unary(&mut self) -> Res<Expr> {
        let line = self.line();
        let op = match self.peek() {
            Tok::P("-") => Some('-'),
            Tok::P("!") => Some('!'),
            Tok::P("~") => Some('~'),
            _ => None,
        };
        if let Some(c) = op {
            self.next();
            let e = self.parse_unary()?;
            return Ok(self.mk(ExprKind::Unary(c, Box::new(e)), line));
        }
        self.parse_cast()
    }
    fn parse_cast(&mut self) -> Res<Expr> {
        let mut e = self.parse_postfix()?;
        while self.is_kw("as") {
            self.next();
            let t = self.parse_type()?;
            let line = e.line;
            e = self.mk(ExprKind::Cast(Box::new(e), t), line);
        }
        Ok(e)
    }

    fn parse_args(&mut self) -> Res<Vec<Arg>> {
        self.expect_p("(")?;
        let saved = self.no_gt;
        self.no_gt = 0;
        let mut args = Vec::new();
        while !self.is_p(")") {
            let named = matches!(self.peek(), Tok::Ident(_)) && self.is_p_at(1, "=");
            let name = if named {
                let n = self.expect_ident()?;
                self.next(); // `=`
                Some(n)
            } else {
                None
            };
            let value = self.parse_expr()?;
            args.push(Arg { name, value });
            if !self.eat_p(",") {
                break;
            }
        }
        self.expect_p(")")?;
        self.no_gt = saved;
        Ok(args)
    }

    fn parse_postfix(&mut self) -> Res<Expr> {
        let mut e = self.parse_primary()?;
        loop {
            let line = e.line;
            if self.is_p("(") {
                let args = self.parse_args()?;
                e = self.mk(ExprKind::Call(Box::new(e), args), line);
            } else if self.is_p("[") {
                self.next();
                let saved = self.no_gt;
                self.no_gt = 0;
                let i = self.parse_expr()?;
                if self.eat_p(":") {
                    let lo = self.parse_expr()?;
                    self.no_gt = saved;
                    self.expect_p("]")?;
                    e = self.mk(ExprKind::Slice(Box::new(e), Box::new(i), Box::new(lo)), line);
                } else {
                    self.no_gt = saved;
                    self.expect_p("]")?;
                    e = self.mk(ExprKind::Index(Box::new(e), Box::new(i)), line);
                }
            } else if self.is_p(".") {
                self.next();
                match self.peek().clone() {
                    Tok::Ident(name) => {
                        self.next();
                        let mut targs = Vec::new();
                        if self.is_p("::") && self.is_p_at(1, "<") {
                            self.next();
                            self.next();
                            targs = self.parse_targ_list()?;
                        }
                        if self.is_p("(") {
                            let args = self.parse_args()?;
                            e = self.mk(ExprKind::Method(Box::new(e), name, targs, args), line);
                        } else {
                            e = self.mk(ExprKind::Field(Box::new(e), name), line);
                        }
                    }
                    Tok::Int(n) => {
                        self.next();
                        e = self.mk(ExprKind::Field(Box::new(e), n.to_string()), line);
                    }
                    _ => return self.err(format!("expected field or method name, found {}", self.describe())),
                }
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_if(&mut self) -> Res<Expr> {
        let line = self.line();
        self.expect_kw("if")?;
        let cond = self.parse_expr()?;
        let then = self.parse_block()?;
        let els = if self.eat_kw("else") {
            if self.is_kw("if") {
                Some(Box::new(self.parse_if()?))
            } else {
                let l = self.line();
                let b = self.parse_block()?;
                Some(Box::new(self.mk(ExprKind::Block(b), l)))
            }
        } else {
            None
        };
        Ok(self.mk(ExprKind::If(Box::new(cond), then, els), line))
    }

    fn parse_match_pat(&mut self) -> Res<MatchPat> {
        if self.eat_p("(") {
            let mut ps = Vec::new();
            while !self.is_p(")") {
                ps.push(self.parse_match_pat()?);
                if !self.eat_p(",") {
                    break;
                }
            }
            self.expect_p(")")?;
            return Ok(MatchPat::Tuple(ps));
        }
        if let Tok::Ident(n) = self.peek().clone() {
            if n == "_" {
                self.next();
                return Ok(MatchPat::Wild);
            }
            if n != "true" && n != "false" && !self.is_p_at(1, "..") && !self.is_p_at(1, "..=") {
                self.next();
                return Ok(MatchPat::Bind(n));
            }
        }
        let lo = self.parse_or()?;
        if self.is_p("..") || self.is_p("..=") {
            let incl = self.is_p("..=");
            self.next();
            let hi = self.parse_or()?;
            return Ok(MatchPat::Range(lo, hi, incl));
        }
        Ok(MatchPat::Lit(lo))
    }

    fn parse_match(&mut self) -> Res<Expr> {
        let line = self.line();
        self.expect_kw("match")?;
        let scrut = self.parse_expr()?;
        self.expect_p("{")?;
        let mut arms = Vec::new();
        while !self.is_p("}") {
            let pat = self.parse_match_pat()?;
            let guard = if self.eat_kw("if") { Some(self.parse_expr()?) } else { None };
            self.expect_p("=>")?;
            let body = if self.is_p("{") {
                let l = self.line();
                let b = self.parse_block()?;
                self.mk(ExprKind::Block(b), l)
            } else {
                self.parse_expr()?
            };
            arms.push(Arm { pat, guard, body });
            self.eat_p(",");
        }
        self.expect_p("}")?;
        Ok(self.mk(ExprKind::Match(Box::new(scrut), arms), line))
    }

    fn parse_primary(&mut self) -> Res<Expr> {
        let line = self.line();
        let tok = self.peek().clone();
        match tok {
            Tok::Int(n) => {
                self.next();
                Ok(self.mk(ExprKind::Int(n), line))
            }
            Tok::Sized(_, n) => {
                self.next();
                Ok(self.mk(ExprKind::Int(n), line))
            }
            Tok::Float(x) => {
                self.next();
                Ok(self.mk(ExprKind::Float(x), line))
            }
            Tok::Str(s) => {
                self.next();
                Ok(self.mk(ExprKind::Str(s), line))
            }
            Tok::P("(") => {
                self.next();
                let saved = self.no_gt;
                self.no_gt = 0;
                let mut items = Vec::new();
                let mut trailing = false;
                while !self.is_p(")") {
                    items.push(self.parse_expr()?);
                    trailing = false;
                    if self.eat_p(",") {
                        trailing = true;
                    } else {
                        break;
                    }
                }
                self.expect_p(")")?;
                self.no_gt = saved;
                if items.len() == 1 && !trailing {
                    Ok(items.pop().unwrap())
                } else {
                    Ok(self.mk(ExprKind::Tuple(items), line))
                }
            }
            Tok::P("[") => {
                self.next();
                let saved = self.no_gt;
                self.no_gt = 0;
                if self.eat_p("]") {
                    self.no_gt = saved;
                    return Ok(self.mk(ExprKind::Array(vec![]), line));
                }
                let first = self.parse_expr()?;
                if self.eat_p(";") {
                    let n = self.parse_expr()?;
                    self.expect_p("]")?;
                    self.no_gt = saved;
                    return Ok(self.mk(ExprKind::Repeat(Box::new(first), Box::new(n)), line));
                }
                let mut items = vec![first];
                while self.eat_p(",") {
                    if self.is_p("]") {
                        break;
                    }
                    items.push(self.parse_expr()?);
                }
                self.expect_p("]")?;
                self.no_gt = saved;
                Ok(self.mk(ExprKind::Array(items), line))
            }
            Tok::P("{") => {
                let b = self.parse_block()?;
                Ok(self.mk(ExprKind::Block(b), line))
            }
            Tok::P("||") => {
                self.next();
                let body = self.parse_expr()?;
                Ok(self.mk(ExprKind::Lambda(vec![], Box::new(body)), line))
            }
            Tok::P("|") => {
                self.next();
                let mut params = Vec::new();
                while !self.is_p("|") {
                    params.push(self.expect_ident()?);
                    if !self.eat_p(",") {
                        break;
                    }
                }
                self.expect_p("|")?;
                let body = self.parse_expr()?;
                Ok(self.mk(ExprKind::Lambda(params, Box::new(body)), line))
            }
            Tok::Ident(name) => match name.as_str() {
                "true" => {
                    self.next();
                    Ok(self.mk(ExprKind::Bool(true), line))
                }
                "false" => {
                    self.next();
                    Ok(self.mk(ExprKind::Bool(false), line))
                }
                "if" => self.parse_if(),
                "match" => self.parse_match(),
                "comptime" if self.is_p_at(1, "{") => {
                    self.next();
                    let b = self.parse_block()?;
                    Ok(self.mk(ExprKind::Comptime(b), line))
                }
                _ => {
                    self.next();
                    let mut segs = vec![name];
                    let mut targs: Option<Vec<TArg>> = None;
                    while self.is_p("::") {
                        self.next();
                        if self.eat_p("<") {
                            targs = Some(self.parse_targ_list()?);
                            break;
                        }
                        segs.push(self.expect_ident()?);
                    }
                    Ok(match targs {
                        Some(t) => self.mk(ExprKind::PathT(segs, t), line),
                        None => self.mk(ExprKind::Path(segs), line),
                    })
                }
            },
            _ => self.err(format!("expected expression, found {}", self.describe())),
        }
    }
}

enum Parsed {
    Stmt(Stmt),
    Tail(Expr),
}
