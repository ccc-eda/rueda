//! A small WebAssembly binary writer (as veryl-emu's, with any function types, data segments
//! and multi-value results): one memory exported as "mem", functions, exports.

pub const I32: u8 = 0x7f;
pub const I64: u8 = 0x7e;

pub fn uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 { out.push(b); return; }
        out.push(b | 0x80);
    }
}

pub fn sleb(out: &mut Vec<u8>, mut v: i64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0) { out.push(b); return; }
        out.push(b | 0x80);
    }
}

/// a function type: parameters, results
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FType(pub Vec<u8>, pub Vec<u8>);

pub struct Func {
    pub name: String,
    pub ty: FType,
    pub locals: Vec<u8>,
    pub code: Vec<u8>,
}

impl Func {
    pub fn new(name: &str, ty: FType) -> Self { Func { name: name.into(), ty, locals: vec![], code: vec![] } }
    /// a new local of type `t`; returns its index
    pub fn local(&mut self, t: u8) -> u32 { self.locals.push(t); (self.ty.0.len() + self.locals.len() - 1) as u32 }
    pub fn op(&mut self, b: u8) -> &mut Self { self.code.push(b); self }
    pub fn u(&mut self, v: u64) -> &mut Self { uleb(&mut self.code, v); self }
    pub fn get(&mut self, l: u32) -> &mut Self { self.op(0x20).u(l as u64) }
    pub fn set(&mut self, l: u32) -> &mut Self { self.op(0x21).u(l as u64) }
    pub fn tee(&mut self, l: u32) -> &mut Self { self.op(0x22).u(l as u64) }
    pub fn i32c(&mut self, v: i32) -> &mut Self { self.code.push(0x41); sleb(&mut self.code, v as i64); self }
    pub fn i64c(&mut self, v: i64) -> &mut Self { self.code.push(0x42); sleb(&mut self.code, v); self }
    pub fn load64(&mut self, off: u32) -> &mut Self { self.op(0x29).u(3).u(off as u64) }
    pub fn store64(&mut self, off: u32) -> &mut Self { self.op(0x37).u(3).u(off as u64) }
    pub fn load32(&mut self, off: u32) -> &mut Self { self.op(0x28).u(2).u(off as u64) }
    pub fn store32(&mut self, off: u32) -> &mut Self { self.op(0x36).u(2).u(off as u64) }
    pub fn call(&mut self, f: u32) -> &mut Self { self.op(0x10).u(f as u64) }
    pub fn block(&mut self) -> &mut Self { self.op(0x02).op(0x40) }
    pub fn loop_(&mut self) -> &mut Self { self.op(0x03).op(0x40) }
    pub fn if_(&mut self) -> &mut Self { self.op(0x04).op(0x40) }
    pub fn else_(&mut self) -> &mut Self { self.op(0x05) }
    pub fn end(&mut self) -> &mut Self { self.op(0x0b) }
    pub fn br(&mut self, d: u32) -> &mut Self { self.op(0x0c).u(d as u64) }
    pub fn br_if(&mut self, d: u32) -> &mut Self { self.op(0x0d).u(d as u64) }
    pub fn ret(&mut self) -> &mut Self { self.op(0x0f) }
    /// memory.copy (dst, src, len on the stack)
    pub fn memcopy(&mut self) -> &mut Self { self.op(0xfc).u(10).op(0).op(0) }

    fn encode(&self) -> Vec<u8> {
        let mut body = vec![];
        let mut runs: Vec<(u32, u8)> = vec![];
        for &t in &self.locals {
            match runs.last_mut() { Some((n, tt)) if *tt == t => *n += 1, _ => runs.push((1, t)) }
        }
        uleb(&mut body, runs.len() as u64);
        for (n, t) in runs { uleb(&mut body, n as u64); body.push(t); }
        body.extend_from_slice(&self.code);
        body.push(0x0b);
        let mut out = vec![];
        uleb(&mut out, body.len() as u64);
        out.extend(body);
        out
    }
}

pub const I64_EQZ: u8 = 0x50;
pub const I64_EQ: u8 = 0x51;
pub const I64_NE: u8 = 0x52;
pub const I64_LT_S: u8 = 0x53;
pub const I64_LT_U: u8 = 0x54;
pub const I64_GT_U: u8 = 0x56;
pub const I64_LE_S: u8 = 0x57;
pub const I64_LE_U: u8 = 0x58;
pub const I64_GE_U: u8 = 0x5a;
pub const I64_ADD: u8 = 0x7c;
pub const I64_SUB: u8 = 0x7d;
pub const I64_MUL: u8 = 0x7e;
pub const I64_DIV_S: u8 = 0x7f;
pub const I64_DIV_U: u8 = 0x80;
pub const I64_REM_S: u8 = 0x81;
pub const I64_REM_U: u8 = 0x82;
pub const I64_AND: u8 = 0x83;
pub const I64_OR: u8 = 0x84;
pub const I64_XOR: u8 = 0x85;
pub const I64_SHL: u8 = 0x86;
pub const I64_SHR_S: u8 = 0x87;
pub const I64_SHR_U: u8 = 0x88;
pub const I32_EQZ: u8 = 0x45;
pub const I32_NE: u8 = 0x47;
pub const I32_LT_S: u8 = 0x48;
pub const I32_GE_U: u8 = 0x4f;
pub const I32_ADD: u8 = 0x6a;
pub const I32_SUB: u8 = 0x6b;
pub const I32_MUL: u8 = 0x6c;
pub const I32_REM_U: u8 = 0x70;
pub const I32_AND: u8 = 0x71;
pub const I32_OR: u8 = 0x72;
pub const I32_WRAP_I64: u8 = 0xa7;
pub const I64_EXTEND_I32_U: u8 = 0xad;
pub const SELECT: u8 = 0x1b;

/// a whole module: memory of `pages` 64 KB pages, functions, exports (name, function), data
/// segments (address, bytes)
pub fn module(pages: u32, funcs: &[Func], exports: &[(String, u32)], data: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut m = b"\0asm\x01\0\0\0".to_vec();
    let section = |m: &mut Vec<u8>, id: u8, body: Vec<u8>| { m.push(id); uleb(m, body.len() as u64); m.extend(body); };
    let mut types: Vec<FType> = vec![];
    for f in funcs { if !types.contains(&f.ty) { types.push(f.ty.clone()); } }
    let mut t = vec![];
    uleb(&mut t, types.len() as u64);
    for ty in &types {
        t.push(0x60);
        uleb(&mut t, ty.0.len() as u64);
        t.extend(&ty.0);
        uleb(&mut t, ty.1.len() as u64);
        t.extend(&ty.1);
    }
    section(&mut m, 1, t);
    let mut f = vec![];
    uleb(&mut f, funcs.len() as u64);
    for fu in funcs { uleb(&mut f, types.iter().position(|t| *t == fu.ty).unwrap() as u64); }
    section(&mut m, 3, f);
    let mut me = vec![];
    uleb(&mut me, 1);
    me.push(0);
    uleb(&mut me, pages as u64);
    section(&mut m, 5, me);
    let mut ex = vec![];
    uleb(&mut ex, exports.len() as u64 + 1);
    let name = |ex: &mut Vec<u8>, s: &str| { uleb(ex, s.len() as u64); ex.extend(s.as_bytes()); };
    name(&mut ex, "mem");
    ex.push(2);
    uleb(&mut ex, 0);
    for (n, i) in exports {
        name(&mut ex, n);
        ex.push(0);
        uleb(&mut ex, *i as u64);
    }
    section(&mut m, 7, ex);
    // data count (bulk memory)
    let mut dc = vec![];
    uleb(&mut dc, data.len() as u64);
    section(&mut m, 12, dc);
    let mut c = vec![];
    uleb(&mut c, funcs.len() as u64);
    for fu in funcs { c.extend(fu.encode()); }
    section(&mut m, 10, c);
    let mut d = vec![];
    uleb(&mut d, data.len() as u64);
    for (at, bytes) in data {
        d.push(0);
        d.push(0x41);
        sleb(&mut d, *at as i32 as i64);
        d.push(0x0b);
        uleb(&mut d, bytes.len() as u64);
        d.extend(bytes);
    }
    section(&mut m, 11, d);
    let mut names = vec![];
    uleb(&mut names, funcs.len() as u64);
    for (i, fu) in funcs.iter().enumerate() {
        uleb(&mut names, i as u64);
        uleb(&mut names, fu.name.len() as u64);
        names.extend(fu.name.as_bytes());
    }
    let mut ns = vec![];
    uleb(&mut ns, 4);
    ns.extend(b"name");
    ns.push(1);
    uleb(&mut ns, names.len() as u64);
    ns.extend(names);
    section(&mut m, 0, ns);
    m
}
