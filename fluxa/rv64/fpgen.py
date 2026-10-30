#!/usr/bin/env python3
"""rv64 F/D 對照向量產生器（v0.3 M5）。

獨立的參考實作：以 Python Fraction 表示精確值，捨入用「有理數比較 + 二分搜尋」決定
（與 fpu.fx 的位元/整數演算法完全不同的方法），依 RISC-V 規格處理 NaN、∞、±0、
五種捨入模式與 fflags（tininess 在捨入後判定、NaN 一律 canonical）。

用法：python3 rv64/fpgen.py > rv64/tests_fpu.fx
輸出固定（隨機種子固定），所以檔案可以重新產生並以 diff 檢查。
"""
import random
from fractions import Fraction as Fr

NV, DZ, OF, UF, NX = 16, 8, 4, 2, 1
RNE, RTZ, RDN, RUP, RMM = 0, 1, 2, 3, 4


class Fmt:
    def __init__(self, ebits, mbits):
        self.e, self.m = ebits, mbits
        self.bias = (1 << (ebits - 1)) - 1
        self.emin = 1 - self.bias
        self.sbit = 1 << (ebits + mbits)
        self.inf = ((1 << ebits) - 1) << mbits
        self.qnan = self.inf | (1 << (mbits - 1))
        self.maxfin = (2 - Fr(1, 1 << mbits)) * Fr(2) ** self.bias


S = Fmt(8, 23)
D = Fmt(11, 52)


def decode(f, v):
    s = (v & f.sbit) != 0
    e = (v >> f.m) & ((1 << f.e) - 1)
    m = v & ((1 << f.m) - 1)
    if e == (1 << f.e) - 1:
        if m == 0:
            return ("inf", s)
        return ("nan", (m >> (f.m - 1)) == 0)      # (類別, 是否 sNaN)
    if e == 0 and m == 0:
        return ("zero", s)
    if e == 0:
        mag = Fr(m) * Fr(2) ** (f.emin - f.m)
    else:
        mag = Fr(m + (1 << f.m)) * Fr(2) ** (e - f.bias - f.m)
    return ("fin", s, mag)


def value(f, v):
    """有號 Fraction（零為 0；∞ 以 ±2×最大有限值代表，只供排序）"""
    d = decode(f, v)
    if d[0] == "zero":
        return Fr(0)
    if d[0] == "inf":                                  # 只用於排序比較：任何大於最大有限值的數都可
        return -2 * f.maxfin if d[1] else 2 * f.maxfin
    return -d[2] if d[1] else d[2]


def round_up(rm, s, n, half, exact):
    """half：捨去部分與一半的比較（-1/0/1）"""
    if exact:
        return False
    if rm == RNE:
        return half > 0 or (half == 0 and n % 2 == 1)
    if rm == RTZ:
        return False
    if rm == RDN:
        return s
    if rm == RUP:
        return not s
    return half >= 0                                   # RMM


def floor_div(cmp, q, hi):
    """最大的 n（0 ≤ n ≤ hi）使得 值 ≥ n·q；cmp(t) 比較 值 與 t"""
    lo = 0
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if cmp(mid * q) >= 0:
            lo = mid
        else:
            hi = mid - 1
    return lo


def round_generic(f, s, cmp, rm):
    """把正值（只能透過 cmp 比較）捨入成格式 f：回傳 (位元, 旗標)"""
    e = 0
    while cmp(Fr(2) ** e) < 0:
        e -= 1
    while cmp(Fr(2) ** (e + 1)) >= 0:
        e += 1

    def rnd(qexp):
        q = Fr(2) ** qexp
        n = floor_div(cmp, q, 1 << (f.m + 2))
        exact = cmp(n * q) == 0
        half = cmp((n + Fr(1, 2)) * q)
        return (n + (1 if round_up(rm, s, n, half, exact) else 0)) * q, exact

    r, exact = rnd(max(e, f.emin) - f.m)
    flags = 0 if exact else NX
    if r > f.maxfin:
        to_inf = rm in (RNE, RMM) or (rm == RUP and not s) or (rm == RDN and s)
        mag = f.inf if to_inf else encode_mag(f, f.maxfin)
        return mag | (f.sbit if s else 0), OF | NX
    if not exact and e < f.emin:
        r_unbounded, _ = rnd(e - f.m)
        if r_unbounded < Fr(2) ** f.emin:
            flags |= UF
    return encode_mag(f, r) | (f.sbit if s else 0), flags


def encode_mag(f, r):
    if r == 0:
        return 0
    if r < Fr(2) ** f.emin:
        frac = r / Fr(2) ** (f.emin - f.m)
        assert frac.denominator == 1
        return int(frac)
    e = 0
    while Fr(2) ** e > r:
        e -= 1
    while Fr(2) ** (e + 1) <= r:
        e += 1
    frac = r / Fr(2) ** (e - f.m) - (1 << f.m)
    assert frac.denominator == 1 and 0 <= frac < (1 << f.m)
    return ((e + f.bias) << f.m) | int(frac)


def round_exact(f, x, rm):
    """精確有理數 x ≠ 0"""
    s = x < 0
    a = -x if s else x
    return round_generic(f, s, lambda t: (a > t) - (a < t), rm)


def zero(f, s):
    return f.sbit if s else 0


def inf(f, s):
    return f.inf | (f.sbit if s else 0)


def is_nan(d):
    return d[0] == "nan"


def is_snan(d):
    return d[0] == "nan" and d[1]


def nan_res(f, *ds):
    return f.qnan, NV if any(is_snan(d) for d in ds) else 0


def sign_of(d):
    return d[1]


def op_add(f, a, b, rm, sub=False):
    da, db = decode(f, a), decode(f, b)
    if is_nan(da) or is_nan(db):
        return nan_res(f, da, db)
    sa, sb = sign_of(da), sign_of(db) != sub
    if da[0] == "inf" and db[0] == "inf":
        return (f.qnan, NV) if sa != sb else (inf(f, sa), 0)
    if da[0] == "inf":
        return inf(f, sa), 0
    if db[0] == "inf":
        return inf(f, sb), 0
    x = value(f, a) + (-value(f, b) if sub else value(f, b))
    if x == 0:
        if da[0] == "zero" and db[0] == "zero" and sa == sb:
            return zero(f, sa), 0
        return zero(f, rm == RDN), 0
    return round_exact(f, x, rm)


def op_mul(f, a, b, rm):
    da, db = decode(f, a), decode(f, b)
    if is_nan(da) or is_nan(db):
        return nan_res(f, da, db)
    s = sign_of(da) != sign_of(db)
    if {da[0], db[0]} == {"inf", "zero"}:
        return f.qnan, NV
    if "inf" in (da[0], db[0]):
        return inf(f, s), 0
    if "zero" in (da[0], db[0]):
        return zero(f, s), 0
    return round_exact(f, value(f, a) * value(f, b), rm)


def op_div(f, a, b, rm):
    da, db = decode(f, a), decode(f, b)
    if is_nan(da) or is_nan(db):
        return nan_res(f, da, db)
    s = sign_of(da) != sign_of(db)
    if da[0] == db[0] and da[0] in ("inf", "zero"):
        return f.qnan, NV
    if da[0] == "inf":
        return inf(f, s), 0
    if db[0] == "zero":
        return inf(f, s), DZ
    if da[0] == "zero" or db[0] == "inf":
        return zero(f, s), 0
    return round_exact(f, value(f, a) / value(f, b), rm)


def op_sqrt(f, a, rm):
    da = decode(f, a)
    if is_nan(da):
        return nan_res(f, da)
    if da[0] == "zero":
        return a, 0
    if sign_of(da):
        return f.qnan, NV
    if da[0] == "inf":
        return a, 0
    x = da[2]
    # 比較 sqrt(x) 與 t ⇔ 比較 x 與 t²
    return round_generic(f, False, lambda t: (x > t * t) - (x < t * t), rm)


def op_fma(f, a, b, c, rm, negp, negc):
    da, db, dc = decode(f, a), decode(f, b), decode(f, c)
    inv = {da[0], db[0]} == {"inf", "zero"}
    if is_nan(da) or is_nan(db) or is_nan(dc):
        return f.qnan, NV if (inv or any(is_snan(d) for d in (da, db, dc))) else 0
    if inv:
        return f.qnan, NV
    sp = (sign_of(da) != sign_of(db)) != negp
    sc = sign_of(dc) != negc
    if "inf" in (da[0], db[0]):
        if dc[0] == "inf" and sc != sp:
            return f.qnan, NV
        return inf(f, sp), 0
    if dc[0] == "inf":
        return inf(f, sc), 0
    p = value(f, a) * value(f, b)
    p = -p if negp else p
    cv = -value(f, c) if negc else value(f, c)
    x = p + cv
    if x == 0:
        p_zero = p == 0
        if p_zero and dc[0] == "zero" and sp == sc:
            return zero(f, sp), 0
        return zero(f, rm == RDN), 0
    return round_exact(f, x, rm)


def op_minmax(f, a, b, is_max):
    da, db = decode(f, a), decode(f, b)
    fl = NV if is_snan(da) or is_snan(db) else 0
    if is_nan(da) and is_nan(db):
        return f.qnan, fl
    if is_nan(da):
        return b, fl
    if is_nan(db):
        return a, fl
    va, vb = value(f, a), value(f, b)
    if va == vb:                                      # 只有 ±0 需要區分
        neg_a = sign_of(da)
        pick_a = (not neg_a) if is_max else neg_a
        return (a if pick_a else b), 0
    return (a if (va > vb) == is_max else b), 0


def op_cmp(f, a, b, f3):
    da, db = decode(f, a), decode(f, b)
    if is_nan(da) or is_nan(db):
        signaling = f3 != 2
        return 0, NV if (signaling or is_snan(da) or is_snan(db)) else 0
    va, vb = value(f, a), value(f, b)
    return int(va == vb if f3 == 2 else va < vb if f3 == 1 else va <= vb), 0


def op_class(f, a):
    d = decode(f, a)
    if d[0] == "nan":
        return 1 << (8 if d[1] else 9)
    s = d[1]
    if d[0] == "inf":
        return 1 << (0 if s else 7)
    if d[0] == "zero":
        return 1 << (3 if s else 4)
    sub = d[2] < Fr(2) ** f.emin
    return 1 << ((2 if s else 5) if sub else (1 if s else 6))


INT_RANGE = {0: (-(1 << 31), (1 << 31) - 1), 1: (0, (1 << 32) - 1),
             2: (-(1 << 63), (1 << 63) - 1), 3: (0, (1 << 64) - 1)}


def to_reg(k, v):
    """整數結果放進 64 位元暫存器（W/WU 符號擴展 32 位）"""
    if k <= 1:
        v &= 0xFFFFFFFF
        return v | 0xFFFFFFFF00000000 if v & 0x80000000 else v
    return v & 0xFFFFFFFFFFFFFFFF


def round_int(x, rm):
    s = x < 0
    a = -x if s else x
    n = a.numerator // a.denominator
    frac = a - n
    exact = frac == 0
    half = (frac > Fr(1, 2)) - (frac < Fr(1, 2))
    n += 1 if round_up(rm, s, n, half, exact) else 0
    return (-n if s else n), exact


def op_to_int(f, a, rm, k):
    lo, hi = INT_RANGE[k]
    d = decode(f, a)
    if d[0] == "nan":
        return to_reg(k, hi), NV
    if d[0] == "inf":
        return to_reg(k, lo if d[1] else hi), NV
    if d[0] == "zero":
        return 0, 0
    v, exact = round_int(value(f, a), rm)
    if v < lo or v > hi:
        return to_reg(k, lo if d[1] else hi), NV
    return to_reg(k, v), 0 if exact else NX


def op_from_int(f, x, rm, k):
    if k == 0:
        v = x & 0xFFFFFFFF
        v = v - (1 << 32) if v & 0x80000000 else v
    elif k == 1:
        v = x & 0xFFFFFFFF
    elif k == 2:
        v = x - (1 << 64) if x & (1 << 63) else x
    else:
        v = x
    if v == 0:
        return 0, 0
    return round_exact(f, Fr(v), rm)


def op_cvt_fmt(to, frm, a, rm):
    d = decode(frm, a)
    if d[0] == "nan":
        return to.qnan, NV if d[1] else 0
    if d[0] == "inf":
        return inf(to, d[1]), 0
    if d[0] == "zero":
        return zero(to, d[1]), 0
    return round_exact(to, value(frm, a), rm)


# ── 指令編碼與 NaN-boxing ──
def box(d, v):
    return v if d else (0xFFFFFFFF00000000 | v)


def enc_fp(f5, fmt, rs2, rm):
    return (f5 << 27) | (fmt << 25) | (rs2 << 20) | (2 << 15) | (rm << 12) | (3 << 7) | 0x53


def enc_fma(op, fmt, rm):
    return (4 << 27) | (fmt << 25) | (2 << 20) | (1 << 15) | (rm << 12) | (3 << 7) | op


# ── 測試值 ──
def specials(f):
    vals = [0, f.inf, f.qnan, f.inf | 1, 1, (1 << f.m) - 1, 1 << f.m,
            f.inf - 1, f.bias << f.m, (f.bias << f.m) | 1, (f.bias + 1) << f.m,
            (f.bias - 1) << f.m, ((f.bias + 1) << f.m) | (1 << (f.m - 1)),
            (f.bias << f.m) | ((1 << f.m) - 1)]
    return vals + [v | f.sbit for v in vals]


def rand_val(f, rng):
    s = rng.random() < 0.5
    r = rng.random()
    emax = (1 << f.e) - 2
    if r < 0.1:
        e = 0                                          # 非正規數
    elif r < 0.2:
        e = rng.randint(1, 4)                          # 接近最小正規數
    elif r < 0.3:
        e = rng.randint(emax - 3, emax)                # 接近溢位
    else:
        e = rng.randint(f.bias - 30, f.bias + 30)
    frac = rng.getrandbits(f.m)
    if rng.random() < 0.2:                             # 低位多為 0 → 容易精確或半途
        frac &= ~((1 << rng.randint(1, f.m)) - 1)
    return (f.sbit if s else 0) | (e << f.m) | frac


def near(f, v, rng):
    """與 v 相近的值（相消、近乎相等）"""
    w = v ^ rng.getrandbits(rng.randint(1, 8))
    return w ^ (f.sbit if rng.random() < 0.5 else 0)


def gen():
    rng = random.Random(20261030)
    tests = []                                         # (名稱, [(inst, a, b, c, want, flags)])
    for dbl in (False, True):
        f = D if dbl else S
        fmt = 1 if dbl else 0
        tag = "d" if dbl else "s"
        sp = specials(f)

        def pairs(n):
            out = [(a, b) for a in sp[::2] for b in sp[1::3]]
            for _ in range(n):
                a = rand_val(f, rng)
                b = near(f, a, rng) if rng.random() < 0.3 else rand_val(f, rng)
                out.append((a, b))
            return out

        # 二元算術：每種捨入模式
        for name, f5, fn in (("add", 0, lambda a, b, rm: op_add(f, a, b, rm)),
                             ("sub", 1, lambda a, b, rm: op_add(f, a, b, rm, True)),
                             ("mul", 2, lambda a, b, rm: op_mul(f, a, b, rm)),
                             ("div", 3, lambda a, b, rm: op_div(f, a, b, rm))):
            vec = []
            for rm in range(5):
                for a, b in pairs(40):
                    r, fl = fn(a, b, rm)
                    vec.append((enc_fp(f5, fmt, 2, rm), box(dbl, a), box(dbl, b), 0, box(dbl, r), fl))
            tests.append((f"f{name}_{tag}", vec))
        vec = []
        for rm in range(5):
            for a in sp + [rand_val(f, rng) & ~f.sbit for _ in range(50)]:
                r, fl = op_sqrt(f, a, rm)
                vec.append((enc_fp(0x0B, fmt, 0, rm), box(dbl, a), 0, 0, box(dbl, r), fl))
        tests.append((f"fsqrt_{tag}", vec))
        # FMA 四種 × 捨入模式
        vec = []
        for op, negp, negc in ((0x43, False, False), (0x47, False, True), (0x4B, True, False), (0x4F, True, True)):
            for rm in range(5):
                trip = [(rng.choice(sp), rng.choice(sp), rng.choice(sp)) for _ in range(8)]
                for _ in range(20):
                    a, b = rand_val(f, rng), rand_val(f, rng)
                    pr = value(f, a) * value(f, b) if decode(f, a)[0] == "fin" and decode(f, b)[0] == "fin" else None
                    if pr is not None and rng.random() < 0.5 and pr != 0 and abs(pr) <= f.maxfin:
                        c, _ = round_exact(f, -pr if not negc else pr, rng.randint(0, 4))   # 近乎相消
                        c = near(f, c, rng) if rng.random() < 0.5 else c
                    else:
                        c = rand_val(f, rng)
                    trip.append((a, b, c))
                for a, b, c in trip:
                    r, fl = op_fma(f, a, b, c, rm, negp, negc)
                    vec.append((enc_fma(op, fmt, rm), box(dbl, a), box(dbl, b), box(dbl, c), box(dbl, r), fl))
        tests.append((f"ffma_{tag}", vec))
        # 邊界：(1) 精確積落在 (2^emin·(1−2^−(m+2)), 2^emin)——捨入後 tininess 的分界
        # （RNE/RUP 等進位到 2^emin → 不是 tiny、只有 NX；RTZ/RDN → tiny → UF）
        # (2) FMA 的 ∞×0 + qNaN 也要舉 NV
        vec = []
        lo_t = Fr(2) ** f.emin * (1 - Fr(1, 1 << (f.m + 2)))
        found = 0
        while found < 12:
            a = ((rng.randint(f.bias // 2 - 3, f.bias // 2 + 3)) << f.m) | rng.getrandbits(f.m)
            b, _ = round_exact(f, Fr(2) ** f.emin / value(f, a), rng.randint(0, 4))
            pr = value(f, a) * value(f, b)
            if lo_t < pr < Fr(2) ** f.emin:
                found += 1
                sgn = f.sbit if found % 2 else 0
                for rm in range(5):
                    r, fl = op_mul(f, a | sgn, b, rm)
                    vec.append((enc_fp(2, fmt, 2, rm), box(dbl, a | sgn), box(dbl, b), 0, box(dbl, r), fl))
        one = f.bias << f.m
        for a, b, c in ((f.inf, 0, f.qnan), (0, f.inf | f.sbit, f.qnan), (f.inf, 0, f.inf | 1),
                        (f.inf, one, f.inf | f.sbit), (0, 0, f.sbit), (f.sbit, one, f.sbit)):
            for op, negp, negc in ((0x43, False, False), (0x47, False, True), (0x4B, True, False), (0x4F, True, True)):
                for rm in (RNE, RDN):
                    r, fl = op_fma(f, a, b, c, rm, negp, negc)
                    vec.append((enc_fma(op, fmt, rm), box(dbl, a), box(dbl, b), box(dbl, c), box(dbl, r), fl))
        tests.append((f"fedge_{tag}", vec))
        # min/max/比較/fclass/sgnj
        vec = []
        for a, b in pairs(30) + [(f.sbit, 0), (0, f.sbit)]:
            for f3 in (0, 1):
                r, fl = op_minmax(f, a, b, f3 == 1)
                vec.append((enc_fp(0x05, fmt, 2, f3), box(dbl, a), box(dbl, b), 0, box(dbl, r), fl))
            for f3 in (0, 1, 2):
                r, fl = op_cmp(f, a, b, f3)
                vec.append((enc_fp(0x14, fmt, 2, f3), box(dbl, a), box(dbl, b), 0, r, fl))
                sb = (b & f.sbit) != 0
                sa = (a & f.sbit) != 0
                ns = sb if f3 == 0 else (not sb) if f3 == 1 else (sa != sb)
                r = (a & ~f.sbit) | (f.sbit if ns else 0)
                vec.append((enc_fp(0x04, fmt, 2, f3), box(dbl, a), box(dbl, b), 0, box(dbl, r), 0))
            vec.append((enc_fp(0x1C, fmt, 0, 1), box(dbl, a), 0, 0, op_class(f, a), 0))
        tests.append((f"fminmax_cmp_class_{tag}", vec))
        # 浮點 → 整數（四種 × 捨入模式），整數 → 浮點
        vec = []
        ints = [0, 1, 0xFFFFFFFFFFFFFFFF, 0x7FFFFFFF, 0x80000000, 0xFFFFFFFF, 0x7FFFFFFFFFFFFFFF,
                0x8000000000000000, 0xFFFFFFFF80000000, 0x123456789ABCDEF1, 16777217, (1 << 53) + 1]
        fvals = sp + [rand_val(f, rng) for _ in range(20)]
        for _ in range(30):                            # 整數附近的值
            x = rng.choice([1, -1]) * Fr(rng.randint(0, 1 << rng.randint(1, 66)), rng.choice([1, 2, 4, 3, 10]))
            if x != 0 and abs(x) <= f.maxfin:
                fvals.append(round_exact(f, x, RNE)[0])
        for rm in range(5):
            for k in range(4):
                for a in fvals:
                    r, fl = op_to_int(f, a, rm, k)
                    vec.append((enc_fp(0x18, fmt, k, rm), box(dbl, a), 0, 0, r, fl))
                for x in ints + [rng.getrandbits(64) for _ in range(6)]:
                    r, fl = op_from_int(f, x, rm, k)
                    vec.append((enc_fp(0x1A, fmt, k, rm), x, 0, 0, box(dbl, r), fl))
        tests.append((f"fcvt_int_{tag}", vec))
    # 格式轉換
    vec = []
    for rm in range(5):
        for a in specials(D) + [rand_val(D, rng) for _ in range(30)] + \
                 [(((D.bias + e) << 52) | rng.getrandbits(52)) for e in (-126, -127, -140, -149, -150, 127, 128)]:
            r, fl = op_cvt_fmt(S, D, a, rm)
            vec.append((enc_fp(0x08, 0, 1, rm), a, 0, 0, box(False, r), fl))
        for a in specials(S) + [rand_val(S, rng) for _ in range(10)]:
            r, fl = op_cvt_fmt(D, S, a, rm)
            vec.append((enc_fp(0x08, 1, 0, rm), box(False, a), 0, 0, r, fl))
    tests.append(("fcvt_fmt", vec))
    return tests


def main():
    print("// 由 rv64/fpgen.py 產生，請勿手改（python3 rv64/fpgen.py > rv64/tests_fpu.fx）")
    print("// 每個向量：(指令字, rs1 值, rs2 值, rs3 值, 期望結果, 期望 fflags)；直接驅動 fp_op / fma_op")
    print()
    print("fn fp_check(inst: _, a: _, b: _, c: _, want: _, fl: _) -> u32 {")
    print("    let mut bad: u32 = 0;")
    print("    for k in 0..len(inst) {")
    print("        let r = if inst[k][6:0] == 0x53 { fp_op(inst[k], a[k], b[k]) } else { fma_op(inst[k], a[k], b[k], c[k]) };")
    print("        if r.0 != want[k] || r.1 != fl[k] { bad += 1; }")
    print("    }")
    print("    bad")
    print("}")
    for name, vec in gen():
        print()
        print(f"/// {len(vec)} 個向量")
        print(f"test {name} {{")
        for col, label in ((0, "inst"), (1, "a"), (2, "b"), (3, "c"), (4, "want"), (5, "fl")):
            vals = ", ".join(hex(v[col]) for v in vec)
            print(f"    let {label} = [{vals}];")
        print("    assert fp_check(inst, a, b, c, want, fl) == 0, \"FPU result differs from the reference\";")
        print("}")


if __name__ == "__main__":
    main()
