//! 抽象語法樹。所有節點皆為擁有型別（Send + Sync），可在任務執行緒間共享。

use crate::value::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// `u8`、`i12`、`bit`、`bool` 或具名型別（含泛型引數）
    Name(Vec<String>, Vec<TArg>),
    /// `fixed<I, F, round = r, ovf = o>`
    Fixed(Box<Expr>, Box<Expr>, Vec<(String, String)>),
    /// `int<lo..=hi>`
    Range(Box<Expr>, Box<Expr>),
    /// `bits<N>`
    Bits(Box<Expr>),
    Tuple(Vec<TypeExpr>),
    /// `[T; N]`
    Array(Box<TypeExpr>, Box<Expr>),
    /// `chan<T, depth = d>`
    Chan(Box<TypeExpr>, Option<Box<Expr>>),
    /// `mem<T, N>`
    Mem(Box<TypeExpr>, Box<Expr>),
}

#[derive(Debug, Clone)]
pub enum TArg {
    Ty(TypeExpr),
    Ex(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    AddW,
    SubW,
    MulW,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i128),
    Float(f64),
    Bool(bool),
    Str(String),
    Path(Vec<String>),
    /// 帶 turbofish 的路徑：`chan::<u32>`、`Fir::<4>`
    PathT(Vec<String>, Vec<TArg>),
    Unary(char, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Cast(Box<Expr>, TypeExpr),
    Call(Box<Expr>, Vec<Arg>),
    Method(Box<Expr>, String, Vec<TArg>, Vec<Arg>),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// 位元切片 `x[hi:lo]`
    Slice(Box<Expr>, Box<Expr>, Box<Expr>),
    Range(Box<Expr>, Box<Expr>, bool),
    Array(Vec<Expr>),
    Repeat(Box<Expr>, Box<Expr>),
    Tuple(Vec<Expr>),
    Lambda(Vec<String>, Box<Expr>),
    Block(Block),
    If(Box<Expr>, Block, Option<Box<Expr>>),
    Match(Box<Expr>, Vec<Arm>),
    Comptime(Block),
}

#[derive(Debug, Clone)]
pub struct Arm {
    pub pat: MatchPat,
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Debug, Clone)]
pub enum MatchPat {
    Wild,
    Bind(String),
    Lit(Expr),
    Range(Expr, Expr, bool),
    Tuple(Vec<MatchPat>),
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wild,
    Ident(String),
    Tuple(Vec<Pattern>),
}

#[derive(Debug, Clone)]
pub enum LoopAttr {
    None,
    Pipeline(Vec<(String, Expr)>),
    Unroll(Option<Expr>),
    Serial,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Let {
        pat: Pattern,
        ty: Option<TypeExpr>,
        placement: Option<String>,
        init: Option<Expr>,
    },
    Carry {
        name: String,
        ty: TypeExpr,
        init: Expr,
    },
    Assign {
        target: Expr,
        op: Option<BinOp>,
        value: Expr,
    },
    For {
        attr: LoopAttr,
        pat: Pattern,
        iter: Expr,
        body: Block,
    },
    Spawn(Expr),
    Parallel(Vec<Stmt>),
    Assert {
        cond: Expr,
        text: String,
        msg: Option<Expr>,
    },
    Return(Option<Expr>),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnKind {
    Fn,
    Comb,
    Comptime,
    Task,
}

#[derive(Debug, Clone)]
pub struct Generic {
    pub name: String,
    pub is_const: bool,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub kind: FnKind,
    pub generics: Vec<Generic>,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Block,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct TestDecl {
    pub name: String,
    pub body: Block,
    pub line: u32,
}

/// 每檔案行號空間的間隔：行號高位記錄檔案編號（見 `Program::loc`）
pub const FILE_STRIDE: u32 = 1_000_000;

pub struct Program {
    pub files: Vec<String>,
    pub consts_decl: Vec<ConstDecl>,
    pub fns: HashMap<String, Arc<FnDecl>>,
    pub tests: Vec<TestDecl>,
    /// 編譯期常數（load 階段由直譯器填入）
    pub consts: RwLock<HashMap<String, Value>>,
}

impl Program {
    /// 還原「檔案:行」。行號高位元記錄檔案編號。
    pub fn loc(&self, line: u32) -> String {
        let idx = (line / FILE_STRIDE) as usize;
        let name = self.files.get(idx).map(|s| s.as_str()).unwrap_or("?");
        format!("{}:{}", name, line % FILE_STRIDE)
    }

    /// 併入另一個檔案（像 `iverilog a.v b.v`）；函式名稱不可重複
    pub fn merge(&mut self, other: Program) -> Result<(), String> {
        self.files.extend(other.files);
        self.consts_decl.extend(other.consts_decl);
        self.tests.extend(other.tests);
        for (k, v) in other.fns {
            if self.fns.contains_key(&k) {
                return Err(format!("`{}` is defined in more than one file", k));
            }
            self.fns.insert(k, v);
        }
        Ok(())
    }
}
