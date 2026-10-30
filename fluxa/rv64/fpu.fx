// ════════════════════════════════════════════════════════════════════════════
//  F/D 浮點運算單元（v0.3 M5）：整數精確 IEEE-754
//
//  全部是 comb fn，輸入輸出皆為 64 位元位元模式（單精度在 f 暫存器中 NaN-boxing），
//  回傳 (結果, fflags)。演算法：把運算元展開成「整數有效數 × 2^指數」的精確值，
//  在精確整數上運算（有效數最多 118 位，精確整數上限 120 位），最後由 round_pack
//  依捨入模式一次捨入並判定 NX/UF/OF。RISC-V 不傳播 NaN payload：任何 NaN 結果都是
//  canonical NaN。underflow 的 tininess 在捨入之後判定（RISC-V 的選擇）。
// ════════════════════════════════════════════════════════════════════════════

const FL_NV: u5 = 16;                  // invalid
const FL_DZ: u5 = 8;                   // divide by zero
const FL_OF: u5 = 4;                   // overflow
const FL_UF: u5 = 2;                   // underflow
const FL_NX: u5 = 1;                   // inexact

const RM_RNE: u3 = 0;
const RM_RTZ: u3 = 1;
const RM_RDN: u3 = 2;
const RM_RUP: u3 = 3;
const RM_RMM: u3 = 4;

// ── 格式參數（d = true 為雙精度）──
comb fn fmb(d: bool) -> u32 { if d { 52 } else { 23 } }         // 小數位數
comb fn feb(d: bool) -> u32 { if d { 11 } else { 8 } }          // 指數位數
comb fn fbias(d: bool) -> i32 { if d { 1023 } else { 127 } }
comb fn fqnan(d: bool) -> u64 { if d { 0x7FF8000000000000 } else { 0x7FC00000 } }
comb fn fsbit(d: bool) -> u64 { 1 << (fmb(d) + feb(d)) }         // 符號位
comb fn finf(d: bool, s: bool) -> u64 {
    (((1 << feb(d)) - 1) << fmb(d)) | (if s { fsbit(d) } else { 0 })
}
comb fn fzero(d: bool, s: bool) -> u64 { if s { fsbit(d) } else { 0 } }

/// 單精度在 64 位元暫存器中 NaN-boxing：上半部必須全 1，否則視為 canonical NaN
comb fn unbox(d: bool, v: u64) -> u64 {
    if d { v } else if v[63:32] == 0xFFFFFFFF { v[31:0] } else { 0x7FC00000 }
}
comb fn box(d: bool, v: u64) -> u64 { if d { v } else { 0xFFFFFFFF00000000 | v[31:0] } }

// ── 展開 ──
/// 類別：0 = 零、1 = 有限非零、2 = ∞、3 = qNaN、4 = sNaN
comb fn fcls(d: bool, v: u64) -> u3 {
    let m = fmb(d);
    let emax = (1 << feb(d)) - 1;
    let e = (v >> m) & emax;
    let f = v & ((1 << m) - 1);
    if e == emax {
        if f == 0 { 2 } else if ((f >> (m - 1)) & 1) == 1 { 3 } else { 4 }
    } else if e == 0 && f == 0 { 0 } else { 1 }
}
comb fn fneg(d: bool, v: u64) -> bool { (v & fsbit(d)) != 0 }
/// 有限值 = fsig × 2^fexp（非正規數沒有隱藏位元）
comb fn fsig(d: bool, v: u64) -> u120 {
    let m = fmb(d);
    let e = (v >> m) & ((1 << feb(d)) - 1);
    let f = v & ((1 << m) - 1);
    if e == 0 { f } else { f | (1 << m) }
}
comb fn fexp(d: bool, v: u64) -> i32 {
    let m = fmb(d);
    let e = (v >> m) & ((1 << feb(d)) - 1);
    (if e == 0 { 1 } else { e }) - fbias(d) - m
}

comb fn bitlen(v: u120) -> u32 {
    let mut n: u32 = 0;
    let mut t = v;
    if (t >> 64) != 0 { t = t >> 64; n += 64; }
    if (t >> 32) != 0 { t = t >> 32; n += 32; }
    if (t >> 16) != 0 { t = t >> 16; n += 16; }
    if (t >> 8) != 0 { t = t >> 8; n += 8; }
    if (t >> 4) != 0 { t = t >> 4; n += 4; }
    if (t >> 2) != 0 { t = t >> 2; n += 2; }
    if (t >> 1) != 0 { t = t >> 1; n += 1; }
    if t != 0 { n += 1; }
    n
}

// ── 捨入 ──
/// 要不要進位：lsb = 保留部分的最低位、c = 捨去部分與一半比較（-1 / 0 / 1）、nz = 捨去部分非 0
comb fn rround(rm: u3, s: bool, lsb: bool, c: i2, nz: bool) -> bool {
    match rm {
        0 => c > 0 || (c == 0 && lsb),             // RNE
        1 => false,                                 // RTZ
        2 => nz && s,                               // RDN
        3 => nz && !s,                              // RUP
        _ => c >= 0 && nz,                          // RMM
    }
}

/// sig 右移 sh 位並捨入：(保留部分, 是否進位, 是否不精確)。sh ≤ 0 時是精確左移。
comb fn shift_round(sig: u120, sh: i32, rm: u3, s: bool) -> (u120, bool, bool) {
    if sh <= 0 {
        (sig << (0 - sh), false, false)
    } else {
        let kept = sig >> sh;
        let rem = sig - (kept << sh);
        let c: i2 = if sh > 120 { -1 }
                    else if rem < (1 << (sh - 1)) { -1 }
                    else if rem == (1 << (sh - 1)) { 0 }
                    else { 1 };
        (kept, rround(rm, s, (kept & 1) == 1, c, rem != 0), rem != 0)
    }
}

/// 把精確值 (-1)^s × sig × 2^e（sig > 0）捨入成格式 d 的位元模式並算出旗標。
/// 編碼技巧：mag = ((最低位指數 + m + bias − 1) << m) + 保留部分——
/// 正規數的隱藏位元與非正規數→最小正規數的進位都自然落進指數欄。
comb fn round_pack(d: bool, s: bool, e: i32, sig: u120, rm: u3) -> (u64, u5) {
    let m = fmb(d);
    let bias = fbias(d);
    let emin = 1 - bias;
    let emsb = e + bitlen(sig) - 1;                         // 最高位的指數
    let elsb = if emsb < emin { emin - m } else { emsb - m };   // 結果最低位的指數
    let (kept, up, nx) = shift_round(sig, elsb - e, rm, s);
    let mag = ((elsb + m + bias - 1) << m) + kept + (if up { 1 } else { 0 });
    let inf = finf(d, false);
    if mag >= inf {
        // 溢位：依捨入方向給 ∞ 或最大有限值
        let to_inf = rm == RM_RNE || rm == RM_RMM || (rm == RM_RUP && !s) || (rm == RM_RDN && s);
        ((if to_inf { inf } else { inf - 1 }) | fzero(d, s), FL_OF | FL_NX)
    } else {
        // tininess（捨入後）：以「指數無界、精度 p 位」捨入的結果是否仍 < 2^emin
        let mut tiny = false;
        if emsb < emin {
            let (k2, up2, _) = shift_round(sig, emsb - m - e, rm, s);
            tiny = !(emsb == emin - 1 && k2 + (if up2 { 1 } else { 0 }) == (1 << (m + 1)));
        }
        (mag | fzero(d, s), (if nx { FL_NX } else { 0 }) | (if nx && tiny { FL_UF } else { 0 }))
    }
}

// ── 對齊與精確加法 ──
/// 正規化到 116 位（最高位在 bit 115）
comb fn norm116(sig: u120, e: i32) -> (u120, i32) {
    let sh = 116 - bitlen(sig);
    (sig << sh, e - sh)
}
/// 右移 k 位，任何被移出的 1 都「黏」進最低位（sticky jam）
comb fn jam(x: u120, k: i32) -> u120 {
    if k == 0 { x }
    else if k > 118 { if x != 0 { 1 } else { 0 } }
    else {
        let q = x >> k;
        q | (if (q << k) != x { 1 } else { 0 })
    }
}
/// 兩個非零精確值相加後捨入。兩者先正規化成 116 位：指數差小時對齊不會丟位元（精確），
/// 丟位元時差距 ≥ 10 位、相消最多 1 位，留在 53 位結果下方的保護位元遠多於 2 位，
/// 所以 jam 不影響捨入結果。
comb fn add_core(d: bool, sx: bool, x: u120, ex: i32, sy: bool, y: u120, ey: i32, rm: u3) -> (u64, u5) {
    let (nx, nex) = norm116(x, ex);
    let (ny, ney) = norm116(y, ey);
    let big_x = nex >= ney;
    let eb = if big_x { nex } else { ney };
    let xa = if big_x { nx } else { jam(nx, ney - nex) };
    let ya = if big_x { jam(ny, nex - ney) } else { ny };
    let v = (if sx { 0 - xa } else { xa }) + (if sy { 0 - ya } else { ya });
    if v == 0 {
        (fzero(d, rm == RM_RDN), 0)                          // 精確相消：+0（RDN 為 −0）
    } else {
        round_pack(d, v < 0, eb, if v < 0 { 0 - v } else { v }, rm)
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  運算（a、b、c 已 unbox；結果未 box）
// ════════════════════════════════════════════════════════════════════════════

comb fn nan_result(d: bool, ca: u3, cb: u3) -> (u64, u5) {
    (fqnan(d), if ca == 4 || cb == 4 { FL_NV } else { 0 })
}

comb fn fadd(d: bool, a: u64, b: u64, rm: u3, sub: bool) -> (u64, u5) {
    let ca = fcls(d, a);
    let cb = fcls(d, b);
    let sa = fneg(d, a);
    let sb = fneg(d, b) != sub;
    if ca >= 3 || cb >= 3 { nan_result(d, ca, cb) }
    else if ca == 2 && cb == 2 { if sa != sb { (fqnan(d), FL_NV) } else { (finf(d, sa), 0) } }
    else if ca == 2 { (finf(d, sa), 0) }
    else if cb == 2 { (finf(d, sb), 0) }
    else if ca == 0 && cb == 0 { (fzero(d, if sa == sb { sa } else { rm == RM_RDN }), 0) }
    else if ca == 0 { round_pack(d, sb, fexp(d, b), fsig(d, b), rm) }
    else if cb == 0 { round_pack(d, sa, fexp(d, a), fsig(d, a), rm) }
    else { add_core(d, sa, fsig(d, a), fexp(d, a), sb, fsig(d, b), fexp(d, b), rm) }
}

comb fn fmul(d: bool, a: u64, b: u64, rm: u3) -> (u64, u5) {
    let ca = fcls(d, a);
    let cb = fcls(d, b);
    let s = fneg(d, a) != fneg(d, b);
    if ca >= 3 || cb >= 3 { nan_result(d, ca, cb) }
    else if (ca == 2 && cb == 0) || (ca == 0 && cb == 2) { (fqnan(d), FL_NV) }
    else if ca == 2 || cb == 2 { (finf(d, s), 0) }
    else if ca == 0 || cb == 0 { (fzero(d, s), 0) }
    else { round_pack(d, s, fexp(d, a) + fexp(d, b), fsig(d, a) * fsig(d, b), rm) }
}

comb fn fdiv(d: bool, a: u64, b: u64, rm: u3) -> (u64, u5) {
    let ca = fcls(d, a);
    let cb = fcls(d, b);
    let s = fneg(d, a) != fneg(d, b);
    if ca >= 3 || cb >= 3 { nan_result(d, ca, cb) }
    else if (ca == 2 && cb == 2) || (ca == 0 && cb == 0) { (fqnan(d), FL_NV) }
    else if ca == 2 { (finf(d, s), 0) }
    else if cb == 0 { (finf(d, s), FL_DZ) }
    else if ca == 0 || cb == 2 { (fzero(d, s), 0) }
    else {
        // 被除數正規化到 116 位 → 商至少 63 位；餘數非 0 時 jam 進最低位
        let (na, ea) = norm116(fsig(d, a), fexp(d, a));
        let sb = fsig(d, b);
        let q = na / sb;
        let sticky = if q * sb != na { 1 } else { 0 };
        round_pack(d, s, ea - fexp(d, b) - 1, (q << 1) | sticky, rm)
    }
}

comb fn isqrt(v: u120) -> u120 {
    let mut r: u120 = 0;
    for i in 0..59 {
        let t = r | (1 << (58 - i));
        if t * t <= v { r = t; }
    }
    r
}

comb fn fsqrt(d: bool, a: u64, rm: u3) -> (u64, u5) {
    let ca = fcls(d, a);
    let s = fneg(d, a);
    if ca >= 3 { nan_result(d, ca, 0) }
    else if ca == 0 { (a, 0) }                                // ±0 → ±0
    else if s { (fqnan(d), FL_NV) }                           // 負數（含 −∞）
    else if ca == 2 { (a, 0) }
    else {
        // 正規化到 115 或 116 位，使指數為偶數；根至少 57 位，不是完全平方就 jam
        let (n0, e0) = norm116(fsig(d, a), fexp(d, a));
        let odd = (e0 & 1) != 0;
        let n1 = if odd { n0 >> 1 } else { n0 };             // n0 最低位必為 0，右移精確
        let e1 = if odd { e0 + 1 } else { e0 };
        let r = isqrt(n1);
        let sticky = if r * r != n1 { 1 } else { 0 };
        round_pack(d, false, (e1 >> 1) - 1, (r << 1) | sticky, rm)
    }
}

/// FMA：(−1)^negp·(a×b) + (−1)^negc·c，只捨入一次
comb fn ffma(d: bool, a: u64, b: u64, c: u64, rm: u3, negp: bool, negc: bool) -> (u64, u5) {
    let ca = fcls(d, a);
    let cb = fcls(d, b);
    let cc = fcls(d, c);
    let sp = (fneg(d, a) != fneg(d, b)) != negp;
    let sc = fneg(d, c) != negc;
    let inv_mul = (ca == 2 && cb == 0) || (ca == 0 && cb == 2);
    if ca >= 3 || cb >= 3 || cc >= 3 {
        // ∞×0 + qNaN 也要舉 NV（RISC-V 規定）
        (fqnan(d), if ca == 4 || cb == 4 || cc == 4 || inv_mul { FL_NV } else { 0 })
    }
    else if inv_mul { (fqnan(d), FL_NV) }
    else if ca == 2 || cb == 2 { if cc == 2 && sc != sp { (fqnan(d), FL_NV) } else { (finf(d, sp), 0) } }
    else if cc == 2 { (finf(d, sc), 0) }
    else if ca == 0 || cb == 0 {
        if cc == 0 { (fzero(d, if sp == sc { sp } else { rm == RM_RDN }), 0) }
        else { round_pack(d, sc, fexp(d, c), fsig(d, c), rm) }
    }
    else if cc == 0 { round_pack(d, sp, fexp(d, a) + fexp(d, b), fsig(d, a) * fsig(d, b), rm) }
    else { add_core(d, sp, fsig(d, a) * fsig(d, b), fexp(d, a) + fexp(d, b), sc, fsig(d, c), fexp(d, c), rm) }
}

/// 全序比較用的鍵：−0 與 +0 的鍵相同（皆為 0）
comb fn fkey(d: bool, v: u64) -> i128 {
    let mag = v & (fsbit(d) - 1);
    if fneg(d, v) { 0 - mag } else { mag }
}

/// fmin / fmax：一個 NaN 回另一個、兩個 NaN 回 canonical；−0 < +0；sNaN 舉 NV
comb fn fminmax(d: bool, a: u64, b: u64, max: bool) -> (u64, u5) {
    let ca = fcls(d, a);
    let cb = fcls(d, b);
    let nv = if ca == 4 || cb == 4 { FL_NV } else { 0 };
    if ca >= 3 && cb >= 3 { (fqnan(d), nv) }
    else if ca >= 3 { (b, nv) }
    else if cb >= 3 { (a, nv) }
    else {
        let ka = fkey(d, a);
        let kb = fkey(d, b);
        let a_lt = ka < kb || (ka == kb && fneg(d, a) && !fneg(d, b));
        (if a_lt != max { a } else { b }, 0)
    }
}

/// feq（f3 = 2，只有 sNaN 舉 NV）、flt（1）、fle（0）（任何 NaN 都舉 NV）
comb fn fcmp(d: bool, a: u64, b: u64, f3: u3) -> (u64, u5) {
    let ca = fcls(d, a);
    let cb = fcls(d, b);
    if ca >= 3 || cb >= 3 {
        (0, if f3 != 2 || ca == 4 || cb == 4 { FL_NV } else { 0 })
    } else {
        let ka = fkey(d, a);
        let kb = fkey(d, b);
        let t = if f3 == 2 { ka == kb } else if f3 == 1 { ka < kb } else { ka <= kb };
        (if t { 1 } else { 0 }, 0)
    }
}

comb fn fclass(d: bool, a: u64) -> u64 {
    let c = fcls(d, a);
    let s = fneg(d, a);
    let sub = c == 1 && ((a >> fmb(d)) & ((1 << feb(d)) - 1)) == 0;
    let k = if c == 4 { 8 } else if c == 3 { 9 }
            else if c == 2 { if s { 0 } else { 7 } }
            else if c == 0 { if s { 3 } else { 4 } }
            else if sub { if s { 2 } else { 5 } }
            else { if s { 1 } else { 6 } };
    1 << k
}

/// fsgnj（f3 = 0）、fsgnjn（1）、fsgnjx（2）
comb fn fsgnj(d: bool, a: u64, b: u64, f3: u3) -> u64 {
    let sb = fneg(d, b);
    let s = if f3 == 0 { sb } else if f3 == 1 { !sb } else { fneg(d, a) != sb };
    (a & (fsbit(d) - 1)) | fzero(d, s)
}

// ── 轉換 ──
/// 浮點 → 整數（k：0 = W、1 = WU、2 = L、3 = LU）。超出範圍或 NaN → NV 並夾限
/// （NaN 取最大值）；範圍內不精確 → NX。W/WU 的 32 位結果符號擴展到 64 位。
comb fn fcvt_to_int(d: bool, a: u64, rm: u3, k: u5) -> (u64, u5) {
    let lo: i128 = if k == 0 { 0 - (1 << 31) } else if k == 2 { 0 - (1 << 63) } else { 0 };
    let hi: i128 = if k == 0 { (1 << 31) - 1 } else if k == 1 { (1 << 32) - 1 }
                   else if k == 2 { (1 << 63) - 1 } else { (1 << 64) - 1 };
    let c = fcls(d, a);
    let s = fneg(d, a);
    let mut v: i128 = 0;
    let mut fl: u5 = 0;
    if c >= 3 {
        v = hi;
        fl = FL_NV;
    } else if c == 2 {
        v = if s { lo } else { hi };
        fl = FL_NV;
    } else if c == 1 {
        let e = fexp(d, a);
        let sig = fsig(d, a);
        if e >= 0 && e + bitlen(sig) > 65 {
            v = if s { lo } else { hi };                      // 必然超出範圍
            fl = FL_NV;
        } else {
            let (kept, up, nx) = shift_round(sig, 0 - e, rm, s);
            let mag = kept + (if up { 1 } else { 0 });
            let sv = if s { 0 - mag } else { mag };
            if sv < lo || sv > hi {
                v = if s { lo } else { hi };
                fl = FL_NV;
            } else {
                v = sv;
                fl = if nx { FL_NX } else { 0 };
            }
        }
    }
    (if k <= 1 { v.wrap::<i32>().wrap::<u64>() } else { v.wrap::<u64>() }, fl)
}

/// 整數 → 浮點（k 同上；x 是整數暫存器值，W/WU 只看低 32 位）
comb fn fcvt_from_int(d: bool, x: u64, rm: u3, k: u5) -> (u64, u5) {
    let v: i128 = if k == 0 { x.wrap::<i32>() } else if k == 1 { x.wrap::<u32>() }
                  else if k == 2 { x.wrap::<i64>() } else { x };
    if v == 0 { (0, 0) }
    else { round_pack(d, v < 0, 0, if v < 0 { 0 - v } else { v }, rm) }
}

/// 格式轉換：s → d 精確、d → s 需捨入；NaN 一律 canonical（sNaN 舉 NV）
comb fn fcvt_fmt(to_d: bool, a: u64, rm: u3) -> (u64, u5) {
    let from_d = !to_d;
    let c = fcls(from_d, a);
    let s = fneg(from_d, a);
    if c >= 3 { (fqnan(to_d), if c == 4 { FL_NV } else { 0 }) }
    else if c == 2 { (finf(to_d, s), 0) }
    else if c == 0 { (fzero(to_d, s), 0) }
    else { round_pack(to_d, s, fexp(from_d, a), fsig(from_d, a), rm) }
}

// ════════════════════════════════════════════════════════════════════════════
//  指令層：解碼與分派（Decode 用 fp_decode 決定暫存器檔與合法性，Execute 呼叫 fp_op/fma_op）
// ════════════════════════════════════════════════════════════════════════════

/// OP-FP（0x53）解碼：(合法, rs1 是整數暫存器, 用 rs2, rd 是整數暫存器, 用 rm)
comb fn fp_decode(inst: u32) -> (bool, bool, bool, bool, bool) {
    let f5 = inst[31:27];
    let fmt = inst[26:25];
    let f3 = inst[14:12];
    let rs2 = inst[24:20];
    let ok = fmt <= 1;                                       // 只支援 S 與 D
    if f5 <= 0x03 { (ok, false, true, false, true) }                         // fadd fsub fmul fdiv
    else if f5 == 0x0B { (ok && rs2 == 0, false, false, false, true) }       // fsqrt
    else if f5 == 0x04 { (ok && f3 <= 2, false, true, false, false) }        // fsgnj[n|x]
    else if f5 == 0x05 { (ok && f3 <= 1, false, true, false, false) }        // fmin fmax
    else if f5 == 0x08 { ((fmt == 0 && rs2 == 1) || (fmt == 1 && rs2 == 0), false, false, false, true) }  // fcvt.s.d / fcvt.d.s
    else if f5 == 0x14 { (ok && f3 <= 2, false, true, true, false) }         // fle flt feq
    else if f5 == 0x18 { (ok && rs2 <= 3, false, false, true, true) }        // fcvt.{w,wu,l,lu}.fmt
    else if f5 == 0x1A { (ok && rs2 <= 3, true, false, false, true) }        // fcvt.fmt.{w,wu,l,lu}
    else if f5 == 0x1C { (ok && rs2 == 0 && f3 <= 1, false, false, true, false) }   // fmv.x.fmt / fclass
    else if f5 == 0x1E { (ok && rs2 == 0 && f3 == 0, true, false, false, false) }   // fmv.fmt.x
    else { (false, false, false, false, false) }
}

comb fn boxed(d: bool, r: (u64, u5)) -> (u64, u5) { (box(d, r.0), r.1) }

/// OP-FP 執行。inst 的 rm 欄已由 Decode 換成實際捨入模式。a = rs1 值（f 或 x）、b = rs2 值。
comb fn fp_op(inst: u32, a: u64, b: u64) -> (u64, u5) {
    let f5 = inst[31:27];
    let d = inst[25:25] == 1;
    let f3 = inst[14:12];
    let rs2 = inst[24:20];
    let ua = unbox(d, a);
    let ub = unbox(d, b);
    if f5 == 0x00 { boxed(d, fadd(d, ua, ub, f3, false)) }
    else if f5 == 0x01 { boxed(d, fadd(d, ua, ub, f3, true)) }
    else if f5 == 0x02 { boxed(d, fmul(d, ua, ub, f3)) }
    else if f5 == 0x03 { boxed(d, fdiv(d, ua, ub, f3)) }
    else if f5 == 0x0B { boxed(d, fsqrt(d, ua, f3)) }
    else if f5 == 0x04 { (box(d, fsgnj(d, ua, ub, f3)), 0) }
    else if f5 == 0x05 { boxed(d, fminmax(d, ua, ub, f3 == 1)) }
    else if f5 == 0x08 { boxed(d, fcvt_fmt(d, unbox(!d, a), f3)) }
    else if f5 == 0x14 { fcmp(d, ua, ub, f3) }
    else if f5 == 0x18 { fcvt_to_int(d, ua, f3, rs2) }
    else if f5 == 0x1A { boxed(d, fcvt_from_int(d, a, f3, rs2)) }
    else if f5 == 0x1C {
        if f3 == 1 { (fclass(d, ua), 0) }
        else if d { (a, 0) }
        else { (a.wrap::<i32>().wrap::<u64>(), 0) }          // fmv.x.w：原始低 32 位符號擴展
    }
    else { (box(d, b0(d, a)), 0) }                           // fmv.fmt.x
}
comb fn b0(d: bool, a: u64) -> u64 { if d { a } else { a[31:0] } }

/// FMADD（0x43）、FMSUB（0x47）、FNMSUB（0x4B）、FNMADD（0x4F）
comb fn fma_op(inst: u32, a: u64, b: u64, c: u64) -> (u64, u5) {
    let d = inst[25:25] == 1;
    let op = inst[6:0];
    let negp = op == 0x4B || op == 0x4F;
    let negc = op == 0x47 || op == 0x4F;
    boxed(d, ffma(d, unbox(d, a), unbox(d, b), unbox(d, c), inst[14:12], negp, negc))
}
