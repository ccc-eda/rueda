// ════════════════════════════════════════════════════════════════════════════
//  Rv64Soc in Fluxa: a RISC-V machine Linux boots on (OpenSBI + Linux + busybox)
//
//  The same machine as Veryl's Rv64Soc (veryl/examples/rv64) and riscv-emu (riscv/rvemu), so
//  the same image boots unchanged and the hart's commit log can be compared with riscv-emu's
//  line by line (TRACE):
//
//    Hart     RV64IMAC + Zicsr + Zicntr + Zifencei + Sstc + Svadu, M / S / U, Sv39 (a TLB of
//             512 leaves), the RAM (0x8000_0000), the CLINT (msip, mtimecmp, mtime: one tick per
//             instruction; wfi with nothing pending jumps to the next timer)
//    Devices  the UART (16550 subset, PLIC source 10), the PLIC (contexts M and S), GPIO (16 pins,
//             SiFive registers, pin k = PLIC source 16 + k), the finisher (0x10_0000)
//
//  The hart reaches the devices through a request / response pair of channels; every response
//  brings the external interrupt lines back. What comes from outside (UART bytes, GPIO inputs) the
//  devices ask the environment for (`env_req` / `env`): Fluxa has no non-blocking receive, so the
//  hart polls every POLL instructions and when it waits for an interrupt.
//
//  This hart is one task (a multi-cycle core like Veryl's Rv64Core); fluxa/rv64/rv64.fx is the
//  pipelined RV64G of tasks. It follows riscv/rvemu/src/cpu.rs and bus.rs.
// ════════════════════════════════════════════════════════════════════════════

const H_RAM_BASE: u64 = 0x80000000;
const H_FOREVER: u64 = 0xFFFFFFFFFFFFFFFF;
const H_MAX: u64 = 0xFFFFFFFFFFFFFFFF;
const H_INT: u64 = 0x8000000000000000;
const H_POLL: u64 = 16384;               // instructions between looks at the outside

// mstatus
const H_SIE: u64 = 0x2;
const H_MIE: u64 = 0x8;
const H_SPIE: u64 = 0x20;
const H_MPIE: u64 = 0x80;
const H_SPP: u64 = 0x100;
const H_MPP: u64 = 0x1800;
const H_FS: u64 = 0x6000;
const H_MPRV: u64 = 0x20000;
const H_SUM: u64 = 0x40000;
const H_MXR: u64 = 0x80000;
const H_TVM: u64 = 0x100000;
const H_TW: u64 = 0x200000;
const H_TSR: u64 = 0x400000;
const H_MSTATUS_W: u64 = 0x7E79AA;       // SIE MIE SPIE MPIE SPP MPP FS MPRV SUM MXR TVM TW TSR
const H_SSTATUS_W: u64 = 0xC6122;        // SIE SPIE SPP FS SUM MXR
const H_XLEN: u64 = 0xA00000000;         // UXL = SXL = 2
// interrupts
const H_SSIP: u64 = 0x2;
const H_MSIP: u64 = 0x8;
const H_STIP: u64 = 0x20;
const H_MTIP: u64 = 0x80;
const H_SEIP: u64 = 0x200;
const H_MEIP: u64 = 0x800;
// menvcfg
const H_STCE: u64 = 0x8000000000000000;
const H_ADUE: u64 = 0x2000000000000000;

// the CSR file: slots
const S_MSTATUS: u32 = 0;
const S_MEDELEG: u32 = 1;
const S_MIDELEG: u32 = 2;
const S_MIE: u32 = 3;
const S_MIPSW: u32 = 4;
const S_MTVEC: u32 = 5;
const S_MEPC: u32 = 6;
const S_MCAUSE: u32 = 7;
const S_MTVAL: u32 = 8;
const S_MSCRATCH: u32 = 9;
const S_MCOUNTEREN: u32 = 10;
const S_MENVCFG: u32 = 11;
const S_STVEC: u32 = 12;
const S_SEPC: u32 = 13;
const S_SCAUSE: u32 = 14;
const S_STVAL: u32 = 15;
const S_SSCRATCH: u32 = 16;
const S_SCOUNTEREN: u32 = 17;
const S_SENVCFG: u32 = 18;
const S_SATP: u32 = 19;
const S_STIMECMP: u32 = 20;
const S_PMPCFG0: u32 = 21;
const S_PMPCFG2: u32 = 22;
const S_PMPADDR: u32 = 23;               // 23..38
const S_INSTRET_OFF: u32 = 39;
const S_CYCLE_OFF: u32 = 40;
const S_MTIMECMP: u32 = 41;
const S_N: u32 = 42;

// device requests
const D_READ: u2 = 0;
const D_WRITE: u2 = 1;
const D_POLL: u2 = 2;                     // look at the outside (it answers at once)
const D_WAIT: u2 = 3;                     // wait for something from outside (the hart has nothing to do)

// ════════════════════════════════════════════════════════════════════════════
//  bits
// ════════════════════════════════════════════════════════════════════════════

comb fn h_not(x: u64) -> u64 { x ^ H_MAX }
comb fn h_bit(x: u64, k: u6) -> bool { ((x >> k) & 1) == 1 }
comb fn h_sext8(v: u64) -> u64 { v[7:0].wrap::<i8>().wrap::<u64>() }
comb fn h_sext16(v: u64) -> u64 { v[15:0].wrap::<i16>().wrap::<u64>() }
comb fn h_sext32(v: u64) -> u64 { v[31:0].wrap::<i32>().wrap::<u64>() }
comb fn h_sext12(v: u64) -> u64 { v[11:0].wrap::<i12>().wrap::<u64>() }
comb fn h_s(v: u64) -> i64 { v.wrap::<i64>() }

/// a << sh (sh < 64) modulo 2^64, in two steps (Fluxa's integers stop at 120 bits)
comb fn h_shl(a: u64, sh: u6) -> u64 { ((a << (sh >> 1)).wrap::<u64>() << ((sh + 1) >> 1)).wrap::<u64>() }

/// the low `n` bytes of v (n = 1, 2, 4, 8)
comb fn h_mask(n: u4) -> u64 { if n == 8 { H_MAX } else if n == 4 { 0xFFFFFFFF } else if n == 2 { 0xFFFF } else { 0xFF } }

/// 64 × 64 → (high, low) 64 bits, by 32-bit pieces
comb fn h_mul(a: u64, b: u64) -> (u64, u64) {
    let (ah, al, bh, bl) = (a[63:32], a[31:0], b[63:32], b[31:0]);
    let sum_mid = ah * bl + al * bh;
    let s_lo = al * bl + ((sum_mid.wrap::<u64>() & 0xFFFFFFFF) << 32);
    ((sum_mid >> 32) + ah * bh + (s_lo >> 64), s_lo.wrap::<u64>())
}

/// M: mul, mulh, mulhsu, mulhu, div, divu, rem, remu
comb fn h_m(f3: u3, a: u64, b: u64) -> u64 {
    let (hi, lo) = h_mul(a, b);
    match f3 {
        0 => lo,
        1 => (hi - (if a[63:63] == 1 { b } else { 0 }) - (if b[63:63] == 1 { a } else { 0 })).wrap::<u64>(),
        2 => (hi - (if a[63:63] == 1 { b } else { 0 })).wrap::<u64>(),
        3 => hi,
        4 => if b == 0 { H_MAX } else { (h_s(a) / h_s(b)).wrap::<u64>() },
        5 => if b == 0 { H_MAX } else { a / b },
        6 => if b == 0 { a } else { (h_s(a) % h_s(b)).wrap::<u64>() },
        _ => if b == 0 { a } else { a % b },
    }
}

/// M, the W forms
comb fn h_mw(f3: u3, a: u64, b: u64) -> u64 {
    let (x, y) = (a.wrap::<u32>(), b.wrap::<u32>());
    let (sx, sy) = (a.wrap::<i32>(), b.wrap::<i32>());
    match f3 {
        0 => (x * y).wrap::<i32>().wrap::<u64>(),
        4 => if y == 0 { H_MAX } else { (sx / sy).wrap::<i32>().wrap::<u64>() },
        5 => if y == 0 { H_MAX } else { (x / y).wrap::<i32>().wrap::<u64>() },
        6 => if y == 0 { sx.wrap::<u64>() } else { (sx % sy).wrap::<i32>().wrap::<u64>() },
        _ => if y == 0 { sx.wrap::<u64>() } else { (x % y).wrap::<i32>().wrap::<u64>() },
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  RVC: a 16-bit instruction as its 32-bit form (0: illegal)
// ════════════════════════════════════════════════════════════════════════════

comb fn h_r(f7: u64, rs2: u64, rs1: u64, f3: u64, rd: u64, op: u64) -> u64 { (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op }
comb fn h_i(imm: u64, rs1: u64, f3: u64, rd: u64, op: u64) -> u64 { ((imm & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op }
comb fn h_st(imm: u64, rs2: u64, rs1: u64, f3: u64) -> u64 { (((imm >> 5) & 0x7F) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | ((imm & 31) << 7) | 0x23 }
comb fn h_b(imm: u64, rs2: u64, rs1: u64, f3: u64) -> u64 {
    (((imm >> 12) & 1) << 31) | (((imm >> 5) & 0x3F) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (((imm >> 1) & 15) << 8) | (((imm >> 11) & 1) << 7) | 0x63
}
comb fn h_j(imm: u64, rd: u64) -> u64 { (((imm >> 20) & 1) << 31) | (((imm >> 1) & 0x3FF) << 21) | (((imm >> 11) & 1) << 20) | (((imm >> 12) & 0xFF) << 12) | (rd << 7) | 0x6F }
/// v (bits wide) sign-extended, as 32 bits
comb fn h_sx(v: u64, bits: u6) -> u64 { if ((v >> (bits - 1)) & 1) == 1 { (v | (0xFFFFFFFF ^ ((1 << bits) - 1))) & 0xFFFFFFFF } else { v } }

comb fn h_expand(c: u64) -> u64 {
    let rd = c[11:7];
    let rs2 = c[6:2];
    let rdp = c[4:2] + 8;
    let rs1p = c[9:7] + 8;
    let imm6 = h_sx((c[12:12] << 5) | c[6:2], 6);
    let q = c[1:0];
    let f = c[15:13];
    if q == 0 {
        match f {
            0 => {
                let imm = (c[12:11] << 4) | (c[10:7] << 6) | (c[6:6] << 2) | (c[5:5] << 3);
                if imm == 0 { 0 } else { h_i(imm, 2, 0, rdp, 0x13) }
            }
            2 => h_i((c[12:10] << 3) | (c[6:6] << 2) | (c[5:5] << 6), rs1p, 2, rdp, 0x03),
            3 => h_i((c[12:10] << 3) | (c[6:5] << 6), rs1p, 3, rdp, 0x03),
            6 => h_st((c[12:10] << 3) | (c[6:6] << 2) | (c[5:5] << 6), rdp, rs1p, 2),
            7 => h_st((c[12:10] << 3) | (c[6:5] << 6), rdp, rs1p, 3),
            _ => 0,
        }
    } else if q == 1 {
        match f {
            0 => h_i(imm6, rd, 0, rd, 0x13),
            1 => if rd == 0 { 0 } else { h_i(imm6, rd, 0, rd, 0x1B) },
            2 => h_i(imm6, 0, 0, rd, 0x13),
            3 => if rd == 2 {
                     let imm = h_sx((c[12:12] << 9) | (c[6:6] << 4) | (c[5:5] << 6) | (c[4:3] << 7) | (c[2:2] << 5), 10);
                     if imm == 0 { 0 } else { h_i(imm, 2, 0, 2, 0x13) }
                 } else if imm6 == 0 { 0 } else { ((imm6 << 12) & 0xFFFFFFFF) | (rd << 7) | 0x37 },
            4 => {
                let sh = (c[12:12] << 5) | c[6:2];
                let k = c[11:10];
                if k == 0 { h_i(sh, rs1p, 5, rs1p, 0x13) }
                else if k == 1 { h_i(sh | 0x400, rs1p, 5, rs1p, 0x13) }
                else if k == 2 { h_i(imm6, rs1p, 7, rs1p, 0x13) }
                else {
                    let s = (c[12:12] << 2) | c[6:5];
                    match s {
                        0 => h_r(0x20, rdp, rs1p, 0, rs1p, 0x33),
                        1 => h_r(0, rdp, rs1p, 4, rs1p, 0x33),
                        2 => h_r(0, rdp, rs1p, 6, rs1p, 0x33),
                        3 => h_r(0, rdp, rs1p, 7, rs1p, 0x33),
                        4 => h_r(0x20, rdp, rs1p, 0, rs1p, 0x3B),
                        5 => h_r(0, rdp, rs1p, 0, rs1p, 0x3B),
                        _ => 0,
                    }
                }
            }
            5 => h_j(h_sx((c[12:12] << 11) | (c[11:11] << 4) | (c[10:9] << 8) | (c[8:8] << 10) | (c[7:7] << 6) | (c[6:6] << 7) | (c[5:3] << 1) | (c[2:2] << 5), 12), 0),
            _ => h_b(h_sx((c[12:12] << 8) | (c[11:10] << 3) | (c[6:5] << 6) | (c[4:3] << 1) | (c[2:2] << 5), 9), 0, rs1p, c[13:13]),
        }
    } else {
        match f {
            0 => h_i((c[12:12] << 5) | c[6:2], rd, 1, rd, 0x13),
            2 => if rd == 0 { 0 } else { h_i((c[12:12] << 5) | (c[6:4] << 2) | (c[3:2] << 6), 2, 2, rd, 0x03) },
            3 => if rd == 0 { 0 } else { h_i((c[12:12] << 5) | (c[6:5] << 3) | (c[4:2] << 6), 2, 3, rd, 0x03) },
            4 => if c[12:12] == 0 {
                     if rs2 == 0 { if rd == 0 { 0 } else { h_i(0, rd, 0, 0, 0x67) } } else { h_r(0, rs2, 0, 0, rd, 0x33) }
                 } else {
                     if rs2 == 0 { if rd == 0 { 0x00100073 } else { h_i(0, rd, 0, 1, 0x67) } } else { h_r(0, rs2, rd, 0, rd, 0x33) }
                 },
            6 => h_st((c[12:9] << 2) | (c[8:7] << 6), rs2, 2, 2),
            7 => h_st((c[12:10] << 3) | (c[9:7] << 6), rs2, 2, 3),
            _ => 0,
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  interrupts, CSRs, permissions
// ════════════════════════════════════════════════════════════════════════════

/// mip: the software bits, the timers, the devices' lines
comb fn h_mip(cs: [u64; S_N], mtime: u64, msip: bool, meip: bool, seip: bool) -> u64 {
    let stce = (cs[S_MENVCFG] & H_STCE) != 0;
    (cs[S_MIPSW] & (H_SSIP | H_SEIP | (if stce { 0 } else { H_STIP })))
        | (if mtime >= cs[S_MTIMECMP] { H_MTIP } else { 0 })
        | (if msip { H_MSIP } else { 0 })
        | (if meip { H_MEIP } else { 0 })
        | (if seip { H_SEIP } else { 0 })
        | (if stce && mtime >= cs[S_STIMECMP] { H_STIP } else { 0 })
}

/// the interrupt to take now: (any, cause number)
comb fn h_interrupt(cs: [u64; S_N], prv: u2, mip: u64) -> (bool, u64) {
    let pend = mip & cs[S_MIE];
    let ms = cs[S_MSTATUS];
    let m_on = prv < 3 || (ms & H_MIE) != 0;
    let s_on = prv < 1 || (prv == 1 && (ms & H_SIE) != 0);
    let m = if m_on { pend & h_not(cs[S_MIDELEG]) } else { 0 };
    let s = if s_on { pend & cs[S_MIDELEG] } else { 0 };
    let p = if m != 0 { m } else { s };
    if p == 0 { (false, 0) }
    else if h_bit(p, 11) { (true, 11) } else if h_bit(p, 3) { (true, 3) } else if h_bit(p, 7) { (true, 7) }
    else if h_bit(p, 9) { (true, 9) } else if h_bit(p, 1) { (true, 1) } else { (true, 5) }
}

/// when a timer interrupt can start next
comb fn h_timer_next(cs: [u64; S_N]) -> u64 {
    let t = cs[S_MTIMECMP];
    if (cs[S_MENVCFG] & H_STCE) != 0 && cs[S_STIMECMP] < t { cs[S_STIMECMP] } else { t }
}

comb fn h_counter_ok(cs: [u64; S_N], prv: u2, n: u64) -> bool {
    let bit = (n & 31).wrap::<u6>();
    (prv == 3 || h_bit(cs[S_MCOUNTEREN], bit)) && (prv >= 1 || h_bit(cs[S_SCOUNTEREN], bit))
}

/// read CSR n: (exists and allowed, value)
comb fn h_csr_read(cs: [u64; S_N], prv: u2, n: u64, mtime: u64, instret: u64, mip: u64) -> (bool, u64) {
    let minstret = (instret + cs[S_INSTRET_OFF]).wrap::<u64>();
    let mcycle = (instret + cs[S_CYCLE_OFF]).wrap::<u64>();
    let ms = cs[S_MSTATUS];
    let sd = if (ms & H_FS) == H_FS { H_INT } else { 0 };
    if n == 0xC00 { (h_counter_ok(cs, prv, n), mcycle) }
    else if n == 0xC02 { (h_counter_ok(cs, prv, n), minstret) }
    else if n == 0xC01 { (h_counter_ok(cs, prv, n), mtime) }
    else if n >= 0xC03 && n <= 0xC1F { (h_counter_ok(cs, prv, n), 0) }
    else if n == 0xB00 { (true, mcycle) }
    else if n == 0xB02 { (true, minstret) }
    else if (n >= 0xB03 && n <= 0xB1F) || (n >= 0x323 && n <= 0x33F) || (n >= 0x7A0 && n <= 0x7A3) || n == 0x320 { (true, 0) }
    else if n == 0x300 { (true, ms | sd) }
    else if n == 0x301 { (true, 0x8000000000141105) }
    else if n == 0x302 { (true, cs[S_MEDELEG]) }
    else if n == 0x303 { (true, cs[S_MIDELEG]) }
    else if n == 0x304 { (true, cs[S_MIE]) }
    else if n == 0x305 { (true, cs[S_MTVEC]) }
    else if n == 0x306 { (true, cs[S_MCOUNTEREN]) }
    else if n == 0x30A { (true, cs[S_MENVCFG]) }
    else if n == 0x340 { (true, cs[S_MSCRATCH]) }
    else if n == 0x341 { (true, cs[S_MEPC]) }
    else if n == 0x342 { (true, cs[S_MCAUSE]) }
    else if n == 0x343 { (true, cs[S_MTVAL]) }
    else if n == 0x344 { (true, mip) }
    else if n == 0x3A0 { (true, cs[S_PMPCFG0]) }
    else if n == 0x3A2 { (true, cs[S_PMPCFG2]) }
    else if n == 0x3A1 || n == 0x3A3 { (false, 0) }
    else if n >= 0x3A4 && n <= 0x3AF { (true, 0) }
    else if n >= 0x3B0 && n <= 0x3BF { (true, cs[S_PMPADDR + (n - 0x3B0)]) }
    else if n >= 0x3C0 && n <= 0x3EF { (true, 0) }
    else if n >= 0xF11 && n <= 0xF15 { (true, 0) }
    else if n == 0x100 { let s = ms & (H_SSTATUS_W | 0x300000000); (true, s | sd) }
    else if n == 0x104 { (true, cs[S_MIE] & cs[S_MIDELEG]) }
    else if n == 0x105 { (true, cs[S_STVEC]) }
    else if n == 0x106 { (true, cs[S_SCOUNTEREN]) }
    else if n == 0x10A { (true, cs[S_SENVCFG]) }
    else if n == 0x140 { (true, cs[S_SSCRATCH]) }
    else if n == 0x141 { (true, cs[S_SEPC]) }
    else if n == 0x142 { (true, cs[S_SCAUSE]) }
    else if n == 0x143 { (true, cs[S_STVAL]) }
    else if n == 0x144 { (true, mip & cs[S_MIDELEG]) }
    else if n == 0x14D { (!(prv < 3 && ((cs[S_MENVCFG] & H_STCE) == 0 || (cs[S_MCOUNTEREN] & 2) == 0)), cs[S_STIMECMP]) }
    else if n == 0x180 { (!(prv == 1 && (ms & H_TVM) != 0), cs[S_SATP]) }
    else { (false, 0) }
}

/// write CSR n: (exists, the slot, its new value, the TLB is flushed); the slot S_N writes nothing
comb fn h_csr_write(cs: [u64; S_N], n: u64, v: u64, instret: u64) -> (bool, u32, u64, bool) {
    let ms = cs[S_MSTATUS];
    if n == 0xB00 { (true, S_CYCLE_OFF, (v - instret - 1).wrap::<u64>(), false) }
    else if n == 0xB02 { (true, S_INSTRET_OFF, (v - instret - 1).wrap::<u64>(), false) }
    else if (n >= 0xB03 && n <= 0xB1F) || (n >= 0x323 && n <= 0x33F) || (n >= 0x7A0 && n <= 0x7A3) || n == 0x320 || n == 0x301 { (true, S_N, 0, false) }
    else if n == 0x300 {
        let w = if ((v >> 11) & 3) == 2 { v & h_not(H_MPP) } else { v };
        (true, S_MSTATUS, (ms & h_not(H_MSTATUS_W)) | (w & H_MSTATUS_W), false)
    }
    else if n == 0x302 { (true, S_MEDELEG, v & 0xB3FF, false) }
    else if n == 0x303 { (true, S_MIDELEG, v & (H_SSIP | H_STIP | H_SEIP), false) }
    else if n == 0x304 { (true, S_MIE, v & (H_SSIP | H_MSIP | H_STIP | H_MTIP | H_SEIP | H_MEIP), false) }
    else if n == 0x305 { (true, S_MTVEC, v & h_not(2), false) }
    else if n == 0x306 { (true, S_MCOUNTEREN, v & 7, false) }
    else if n == 0x30A { (true, S_MENVCFG, v & (H_STCE | H_ADUE | 1), false) }
    else if n == 0x340 { (true, S_MSCRATCH, v, false) }
    else if n == 0x341 { (true, S_MEPC, v & h_not(1), false) }
    else if n == 0x342 { (true, S_MCAUSE, v, false) }
    else if n == 0x343 { (true, S_MTVAL, v, false) }
    else if n == 0x344 { (true, S_MIPSW, (cs[S_MIPSW] & h_not(H_SSIP | H_STIP | H_SEIP)) | (v & (H_SSIP | H_STIP | H_SEIP)), false) }
    else if n == 0x3A0 { (true, S_PMPCFG0, v, false) }
    else if n == 0x3A2 { (true, S_PMPCFG2, v, false) }
    else if (n >= 0x3A4 && n <= 0x3AF) || (n >= 0x3C0 && n <= 0x3EF) { (true, S_N, 0, false) }
    else if n >= 0x3B0 && n <= 0x3BF { (true, S_PMPADDR + (n - 0x3B0).wrap::<u32>(), v & 0x3FFFFFFFFFFFFF, false) }
    else if n == 0x100 { (true, S_MSTATUS, (ms & h_not(H_SSTATUS_W)) | (v & H_SSTATUS_W), false) }
    else if n == 0x104 { (true, S_MIE, (cs[S_MIE] & h_not(cs[S_MIDELEG])) | (v & cs[S_MIDELEG]), false) }
    else if n == 0x105 { (true, S_STVEC, v & h_not(2), false) }
    else if n == 0x106 { (true, S_SCOUNTEREN, v & 7, false) }
    else if n == 0x10A { (true, S_SENVCFG, v & 1, false) }
    else if n == 0x140 { (true, S_SSCRATCH, v, false) }
    else if n == 0x141 { (true, S_SEPC, v & h_not(1), false) }
    else if n == 0x142 { (true, S_SCAUSE, v, false) }
    else if n == 0x143 { (true, S_STVAL, v, false) }
    else if n == 0x144 { (true, S_MIPSW, (cs[S_MIPSW] & h_not(H_SSIP & cs[S_MIDELEG])) | (v & H_SSIP & cs[S_MIDELEG]), false) }
    else if n == 0x14D { (true, S_STIMECMP, v, false) }
    else if n == 0x180 {
        let mode = v >> 60;
        if mode == 0 || mode == 8 { (true, S_SATP, v, true) } else { (true, S_N, 0, false) }
    }
    else { (false, S_N, 0, false) }
}

/// may an access (0 fetch, 1 load, 2 store) at privilege prv use a page with these PTE bits?
comb fn h_perm(pte: u64, acc: u2, prv: u2, ms: u64) -> bool {
    let (r, w, x, u) = (h_bit(pte, 1), h_bit(pte, 2), h_bit(pte, 3), h_bit(pte, 4));
    if prv == 0 && !u { false }
    else if prv == 1 && u && (acc == 0 || (ms & H_SUM) == 0) { false }
    else if acc == 0 { x } else if acc == 1 { r || (x && (ms & H_MXR) != 0) } else { w }
}

comb fn h_page_fault(acc: u2) -> u64 { if acc == 0 { 12 } else if acc == 1 { 13 } else { 15 } }
comb fn h_access_fault(acc: u2) -> u64 { if acc == 0 { 1 } else if acc == 1 { 5 } else { 7 } }

/// is [pa, pa + n) in the RAM of `words` 64-bit words?
comb fn h_in_ram(pa: u64, n: u64, words: u64) -> bool { pa >= H_RAM_BASE && pa - H_RAM_BASE + n <= words * 8 }

// ════════════════════════════════════════════════════════════════════════════
//  the hart
// ════════════════════════════════════════════════════════════════════════════

/// RAMW: the RAM's 64-bit words (256 MB = 1 << 25). `load` puts words into the RAM before the hart
/// starts ((word index, value), to the end of the stream). TRACE: a token on `trace` for each
/// instruction done (riscv-emu's -t line: privilege, pc, the instruction, the register written).
task Hart<const RAMW: u32, const TRACE: bool>(load: in chan<(u32, u64)>,
        dreq: out chan<(u2, u64, u4, u64)>, dresp: in chan<(u64, bool, bool)>,
        trace: out chan<(u2, u64, u32, bool, u5, u64)>) {
    let mut ram: mem<u64, RAMW> = [0; RAMW];
    for (a, v) in load { ram[a] = v; }

    let mut x: [u64; 32] = [0; 32];
    let mut cs: [u64; S_N] = [0; S_N];
    cs[S_MSTATUS] = H_XLEN;
    cs[S_STIMECMP] = H_MAX;
    cs[S_MTIMECMP] = H_MAX;
    let mut pc: u64 = H_RAM_BASE;
    let mut prv: u2 = 3;
    let mut mtime: u64 = 0;
    let mut msip: bool = false;
    let mut meip: bool = false;
    let mut seip: bool = false;
    let mut instret: u64 = 0;
    let mut resv: u64 = 0;                    // LR's reservation (physical address), 0 = none
    let mut check: bool = true;
    let mut timer_next: u64 = 0;
    let mut waiting: bool = false;
    let mut last_insn: u64 = 0;
    let mut next_poll: u64 = H_POLL;
    // the TLB: 512 leaves by the low bits of the virtual page number
    let mut tlb_tag: [u64; 512] = [H_MAX; 512];
    let mut tlb_page: [u64; 512] = [0; 512];
    let mut tlb_pte: [u64; 512] = [0; 512];

    serial for _ in 0..H_FOREVER {
        // ── the outside: every POLL instructions (UART bytes, GPIO inputs)
        if mtime >= next_poll {
            next_poll = (mtime + H_POLL).wrap::<u64>();
            dreq.send((D_POLL, 0, 0, 0));
            let (_, me, se) = dresp.recv();
            meip = me;
            seip = se;
            check = true;
        }
        if mtime >= timer_next { timer_next = H_MAX; check = true; }
        // a trap to take at the end of this round: (cause, tval)
        let mut tk = false;
        let mut tc: u64 = 0;
        let mut tv: u64 = 0;
        if check {
            check = false;
            let (irq, code) = h_interrupt(cs, prv, h_mip(cs, mtime, msip, meip, seip));
            if irq { waiting = false; tk = true; tc = H_INT | code; tv = 0; }
        }
        if !tk && waiting {
            if (h_mip(cs, mtime, msip, meip, seip) & cs[S_MIE]) != 0 {
                waiting = false;
            } else {
                // sleep until the next timer, or until something comes from outside
                let t = h_timer_next(cs);
                if t == H_MAX || t <= mtime {
                    dreq.send((D_WAIT, 0, 0, 0));
                    let (_, me, se) = dresp.recv();
                    meip = me;
                    seip = se;
                    check = true;
                } else {
                    mtime = t;
                }
            }
        } else if !tk {
            // ── one instruction: fetch (one or two translations), then its data access (one)
            let last_pc = pc;
            let last_prv = prv;
            let mut wrote = false;
            let mut wrd: u5 = 0;
            let mut wval: u64 = 0;
            let mut fault = false;
            let mut insn: u64 = 0;           // as fetched (16 or 32 bits)
            let mut inst: u64 = 0;           // the 32-bit form
            let mut len: u64 = 4;
            let mut next: u64 = 0;
            // the access being done: phase 0 = fetch, 1 = the second half of a fetch across a page,
            // 2 = data, 3 = none
            let mut ph: u2 = 0;
            let mut va: u64 = pc;
            let mut acc: u2 = 0;
            let mut size: u4 = 4;
            let mut sdata: u64 = 0;          // what a store writes
            let mut amo = false;
            let mut rd: u5 = 0;
            let mut lf3: u3 = 0;             // the load's f3 (sign and size)
            let mut lo16: u64 = 0;
            let mut decoded = false;
            serial for _ in 0..3 {
                if ph < 3 && !fault {
                    // ── translation of va
                    let tprv: u2 = if acc != 0 && (cs[S_MSTATUS] & H_MPRV) != 0 { ((cs[S_MSTATUS] >> 11) & 3).wrap::<u2>() } else { prv };
                    let mut pa: u64 = va;
                    if tprv != 3 && (cs[S_SATP] >> 60) != 0 {
                        let vpn = va >> 12;
                        let e: u32 = (vpn & 511).wrap::<u32>();
                        let ep = tlb_pte[e];
                        if tlb_tag[e] == vpn && (ep & 0x40) != 0 && (acc != 2 || (ep & 0x80) != 0) {
                            if h_perm(ep, acc, tprv, cs[S_MSTATUS]) { pa = tlb_page[e] | (va & 0xFFF); }
                            else { fault = true; tc = h_page_fault(acc); tv = va; }
                        } else {
                            // the page table walk
                            if h_sext39(va) != va {
                                fault = true; tc = h_page_fault(acc); tv = va;
                            } else {
                                let mut a: u64 = (cs[S_SATP] & 0xFFFFFFFFFFF) << 12;
                                let mut level: u2 = 2;
                                let mut walking = true;
                                serial for _ in 0..3 {
                                    if walking {
                                        let pta = a + ((va >> (12 + 9 * level)) & 511) * 8;
                                        if !h_in_ram(pta, 8, RAMW) {
                                            walking = false; fault = true; tc = h_access_fault(acc); tv = va;
                                        } else {
                                            let wi: u32 = ((pta - H_RAM_BASE) >> 3).wrap::<u32>();
                                            let pte = ram[wi];
                                            let (v, r, w, xx) = (h_bit(pte, 0), h_bit(pte, 1), h_bit(pte, 2), h_bit(pte, 3));
                                            if !v || (!r && w) || (pte >> 54) != 0 {
                                                walking = false; fault = true; tc = h_page_fault(acc); tv = va;
                                            } else if !r && !xx {
                                                if level == 0 { walking = false; fault = true; tc = h_page_fault(acc); tv = va; }
                                                else { level = level - 1; a = ((pte >> 10) & 0xFFFFFFFFFFF) << 12; }
                                            } else {
                                                walking = false;
                                                let ppn = (pte >> 10) & 0xFFFFFFFFFFF;
                                                let lmask: u64 = (1 << (9 * level)) - 1;
                                                if level > 0 && (ppn & lmask) != 0 { fault = true; tc = h_page_fault(acc); tv = va; }
                                                else if !h_perm(pte, acc, tprv, cs[S_MSTATUS]) { fault = true; tc = h_page_fault(acc); tv = va; }
                                                else {
                                                    let need: u64 = if acc == 2 { 0xC0 } else { 0x40 };
                                                    let mut p2 = pte;
                                                    if (pte & need) != need {
                                                        if (cs[S_MENVCFG] & H_ADUE) == 0 { fault = true; tc = h_page_fault(acc); tv = va; }
                                                        else { p2 = pte | need; ram[wi] = p2; }
                                                    }
                                                    if !fault {
                                                        let mask: u64 = (1 << (12 + 9 * level)) - 1;
                                                        let page = (((ppn << 12) & h_not(mask)) | (va & mask)) & h_not(0xFFF);
                                                        tlb_tag[e] = vpn;
                                                        tlb_page[e] = page;
                                                        tlb_pte[e] = p2 & 0xFF;
                                                        pa = page | (va & 0xFFF);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // ── the access
                    if !fault && ph == 0 {
                        // an instruction at pc: within one page, or its first half
                        if !h_in_ram(pa, 2, RAMW) {
                            fault = true; tc = 1; tv = pc;
                        } else {
                            let wi: u32 = ((pa - H_RAM_BASE) >> 3).wrap::<u32>();
                            let o = pa & 7;
                            let word = ram[wi];
                            let low = (word >> (8 * o)) & 0xFFFF;
                            if (low & 3) != 3 {
                                insn = low; ph = 2;
                            } else if o < 6 {
                                insn = (word >> (8 * o)) & 0xFFFFFFFF; ph = 2;
                            } else if (pc & 0xFFF) == 0xFFE {
                                // the second half on the next page
                                lo16 = low; ph = 1; va = (pc + 2).wrap::<u64>();
                            } else if !h_in_ram(pa + 2, 2, RAMW) {
                                fault = true; tc = 1; tv = pc;
                            } else {
                                let w2 = ram[wi + 1];
                                insn = low | ((w2 & 0xFFFF) << 16); ph = 2;
                            }
                        }
                    } else if !fault && ph == 1 {
                        if !h_in_ram(pa, 2, RAMW) { fault = true; tc = 1; tv = pc; }
                        else {
                            let wi: u32 = ((pa - H_RAM_BASE) >> 3).wrap::<u32>();
                            let word = ram[wi];
                            insn = lo16 | (((word >> (8 * (pa & 7))) & 0xFFFF) << 16);
                            ph = 2;
                        }
                    } else if !fault && ph == 2 && !amo && acc == 1 {
                        // ── a load (or LR)
                        let mut v: u64 = 0;
                        if h_in_ram(pa, size, RAMW) {
                            let wi: u32 = ((pa - H_RAM_BASE) >> 3).wrap::<u32>();
                            v = (ram[wi] >> (8 * (pa & 7))) & h_mask(size);
                        } else if (pa >> 16) == 0x200 {
                            v = h_clint_read(pa, size, mtime, cs[S_MTIMECMP], msip);
                            check = true;
                        } else if !h_dev(pa) {
                            fault = true; tc = 5; tv = va;
                        } else {
                            dreq.send((D_READ, pa, size, 0));
                            let (dv, me, se) = dresp.recv();
                            meip = me;
                            seip = se;
                            check = true;
                            v = dv & h_mask(size);
                        }
                        if !fault {
                            let lv = if lf3 == 0 { h_sext8(v) } else if lf3 == 1 { h_sext16(v) } else if lf3 == 2 { h_sext32(v) } else { v };
                            if (inst & 0x7F) == 0x2F { resv = pa; }
                            wrote = rd != 0; wrd = rd; wval = lv;
                            if rd != 0 { x[rd] = lv; }
                        }
                        ph = 3;
                    } else if !fault && ph == 2 {
                        // ── a store, SC or AMO
                        if amo && !h_in_ram(pa, size, RAMW) {
                            fault = true; tc = 7; tv = va;
                        } else if h_in_ram(pa, size, RAMW) {
                            let wi: u32 = ((pa - H_RAM_BASE) >> 3).wrap::<u32>();
                            let sh = 8 * (pa & 7);
                            let m = h_mask(size);
                            let old_word = ram[wi];
                            let f5 = inst >> 27;
                            if amo && f5 == 3 {
                                // SC
                                if resv == pa {
                                    ram[wi] = (old_word & h_not((m << sh).wrap::<u64>())) | (((sdata & m) << sh).wrap::<u64>());
                                    wrote = rd != 0; wrd = rd; wval = 0;
                                    if rd != 0 { x[rd] = 0; }
                                } else {
                                    wrote = rd != 0; wrd = rd; wval = 1;
                                    if rd != 0 { x[rd] = 1; }
                                }
                                resv = 0;
                            } else if amo {
                                let raw = (old_word >> sh) & m;
                                let old = if size == 4 { h_sext32(raw) } else { raw };
                                let new = h_amo(f5, old, sdata, size);
                                ram[wi] = (old_word & h_not((m << sh).wrap::<u64>())) | (((new & m) << sh).wrap::<u64>());
                                wrote = rd != 0; wrd = rd; wval = old;
                                if rd != 0 { x[rd] = old; }
                            } else if size == 8 {
                                ram[wi] = sdata;
                            } else {
                                ram[wi] = (old_word & h_not((m << sh).wrap::<u64>())) | (((sdata & m) << sh).wrap::<u64>());
                            }
                        } else if (pa >> 16) == 0x200 {
                            // the CLINT
                            let (nmtime, ncmp, nmsip) = h_clint_write(pa, size, sdata, mtime, cs[S_MTIMECMP], msip);
                            mtime = nmtime;
                            cs[S_MTIMECMP] = ncmp;
                            msip = nmsip;
                            timer_next = h_timer_next(cs);
                            check = true;
                        } else if !h_dev(pa) {
                            fault = true; tc = 7; tv = va;
                        } else {
                            dreq.send((D_WRITE, pa, size, sdata));
                            let (_, me, se) = dresp.recv();
                            meip = me;
                            seip = se;
                            check = true;
                        }
                        ph = 3;
                    }
                    // ── an instruction fetched: run it (a load or store sets up phase 2)
                    if !fault && ph == 2 && !decoded {
                        decoded = true;
                        last_insn = insn;
                        if (insn & 3) == 3 { inst = insn; len = 4; } else { inst = h_expand(insn); len = 2; }
                        next = (pc + len).wrap::<u64>();
                        let ill = if len == 2 { insn } else { inst };
                        if inst == 0 {
                            fault = true; tc = 2; tv = insn; ph = 3;
                        } else {
                            let op = inst & 0x7F;
                            rd = inst[11:7];
                            let rs1 = inst[19:15];
                            let rs2 = inst[24:20];
                            let f3 = inst[14:12];
                            let f7 = inst >> 25;
                            let a = x[rs1];
                            let b = x[rs2];
                            let imm_i = h_sext12(inst >> 20);
                            if op == 0x03 {
                                if f3 == 7 { fault = true; tc = 2; tv = ill; ph = 3; }
                                else {
                                    size = if (f3 & 3) == 0 { 1 } else if (f3 & 3) == 1 { 2 } else if (f3 & 3) == 2 { 4 } else { 8 };
                                    lf3 = if f3 >= 4 { 7 } else { f3 };
                                    va = (a + imm_i).wrap::<u64>();
                                    if (va & (size - 1)) != 0 { fault = true; tc = 4; tv = va; ph = 3; }
                                    else { acc = 1; }
                                }
                            } else if op == 0x23 {
                                if f3 > 3 { fault = true; tc = 2; tv = ill; ph = 3; }
                                else {
                                    size = if f3 == 0 { 1 } else if f3 == 1 { 2 } else if f3 == 2 { 4 } else { 8 };
                                    va = (a + h_sext12(((inst >> 25) << 5) | ((inst >> 7) & 31))).wrap::<u64>();
                                    sdata = b;
                                    if (va & (size - 1)) != 0 { fault = true; tc = 6; tv = va; ph = 3; }
                                    else { acc = 2; }
                                }
                            } else if op == 0x2F {
                                let f5 = inst >> 27;
                                if f3 != 2 && f3 != 3 { fault = true; tc = 2; tv = ill; ph = 3; }
                                else {
                                    size = if f3 == 2 { 4 } else { 8 };
                                    va = a;
                                    sdata = b;
                                    if f5 == 2 {
                                        if rs2 != 0 { fault = true; tc = 2; tv = ill; ph = 3; }
                                        else if (a & (size - 1)) != 0 { fault = true; tc = 4; tv = a; ph = 3; }
                                        else { acc = 1; lf3 = if size == 4 { 2 } else { 3 }; }
                                    } else if !h_amo_ok(f5) { fault = true; tc = 2; tv = ill; ph = 3; }
                                    else if (a & (size - 1)) != 0 { fault = true; tc = 6; tv = a; ph = 3; }
                                    else { acc = 2; amo = true; }
                                }
                            } else if op == 0x73 && f3 == 0 {
                                // ── system: ecall, ebreak, mret, sret, wfi, sfence.vma
                                ph = 3;
                                if inst == 0x00000073 { fault = true; tc = 8 + prv; tv = 0; }
                                else if inst == 0x00100073 { fault = true; tc = 3; tv = pc; }
                                else if inst == 0x30200073 {
                                    if prv < 3 { fault = true; tc = 2; tv = ill; }
                                    else {
                                        let s = cs[S_MSTATUS];
                                        let p = ((s >> 11) & 3).wrap::<u2>();
                                        let mut ns = (s & h_not(H_MPP | H_MIE)) | H_MPIE | (if (s & H_MPIE) != 0 { H_MIE } else { 0 });
                                        if p != 3 { ns = ns & h_not(H_MPRV); }
                                        cs[S_MSTATUS] = ns;
                                        prv = p;
                                        next = cs[S_MEPC];
                                        check = true;
                                    }
                                } else if inst == 0x10200073 {
                                    if prv < 1 || (prv == 1 && (cs[S_MSTATUS] & H_TSR) != 0) { fault = true; tc = 2; tv = ill; }
                                    else {
                                        let s = cs[S_MSTATUS];
                                        prv = ((s >> 8) & 1).wrap::<u2>();
                                        cs[S_MSTATUS] = (s & h_not(H_SPP | H_SIE | H_MPRV)) | H_SPIE | (if (s & H_SPIE) != 0 { H_SIE } else { 0 });
                                        next = cs[S_SEPC];
                                        check = true;
                                    }
                                } else if inst == 0x10500073 {
                                    if prv == 0 || (prv == 1 && (cs[S_MSTATUS] & H_TW) != 0) { fault = true; tc = 2; tv = ill; }
                                    else if (h_mip(cs, mtime, msip, meip, seip) & cs[S_MIE]) == 0 { waiting = true; }
                                } else if f7 == 0x09 && rd == 0 {
                                    if prv == 0 || (prv == 1 && (cs[S_MSTATUS] & H_TVM) != 0) { fault = true; tc = 2; tv = ill; }
                                    else { for i in 0..512 { tlb_tag[i] = H_MAX; } }
                                } else { fault = true; tc = 2; tv = ill; }
                            } else if op == 0x73 {
                                // ── Zicsr
                                ph = 3;
                                if f3 == 4 { fault = true; tc = 2; tv = ill; }
                                else {
                                    let n = inst >> 20;
                                    let src = if f3 >= 5 { rs1 } else { a };
                                    let write = !(((f3 & 3) == 2 || (f3 & 3) == 3) && rs1 == 0);
                                    if ((n >> 8) & 3) > prv || (write && (n >> 10) == 3) { fault = true; tc = 2; tv = ill; }
                                    else {
                                        let (okr, old) = h_csr_read(cs, prv, n, mtime, instret, h_mip(cs, mtime, msip, meip, seip));
                                        if !okr { fault = true; tc = 2; tv = ill; }
                                        else {
                                            if write {
                                                let nv = if (f3 & 3) == 1 { src } else if (f3 & 3) == 2 { old | src } else { old & h_not(src) };
                                                if n == 0x180 && prv == 1 && (cs[S_MSTATUS] & H_TVM) != 0 { fault = true; tc = 2; tv = ill; }
                                                else {
                                                    check = true;
                                                    let (okw, slot, sv, flush) = h_csr_write(cs, n, nv, instret);
                                                    if !okw { fault = true; tc = 2; tv = ill; }
                                                    else {
                                                        if slot < S_N { cs[slot] = sv; }
                                                        if flush { for i in 0..512 { tlb_tag[i] = H_MAX; } }
                                                        if n == 0x30A || n == 0x14D { timer_next = h_timer_next(cs); }
                                                    }
                                                }
                                            }
                                            if !fault { wrote = rd != 0; wrd = rd; wval = old; if rd != 0 { x[rd] = old; } }
                                        }
                                    }
                                }
                            } else {
                                // ── everything else: no memory
                                ph = 3;
                                let (ok, wr, v, nx) = h_exec(inst, a, b, pc, next, imm_i);
                                if !ok { fault = true; tc = 2; tv = ill; }
                                else {
                                    if wr { wrote = rd != 0; wrd = rd; wval = v; if rd != 0 { x[rd] = v; } }
                                    next = nx;
                                }
                            }
                        }
                    }
                }
            }
            if fault {
                wrote = false;
                tk = true;
            } else {
                instret = (instret + 1).wrap::<u64>();
                pc = next;
            }
            // (riscv-emu's log has the instructions done: a trap has no line)
            if TRACE && !fault {
                trace.send((last_prv, last_pc, last_insn.wrap::<u32>(), wrote, wrd, wval));
            }
            mtime = (mtime + 1).wrap::<u64>();
        }
        if tk {
            // ── the trap
            let irq = (tc >> 63) == 1;
            let code = (tc & 63).wrap::<u6>();
            let deleg = if irq { cs[S_MIDELEG] } else { cs[S_MEDELEG] };
            resv = 0;
            let s = cs[S_MSTATUS];
            if prv <= 1 && h_bit(deleg, code) {
                cs[S_SCAUSE] = tc;
                cs[S_SEPC] = pc;
                cs[S_STVAL] = tv;
                cs[S_MSTATUS] = (s & h_not(H_SPP | H_SPIE | H_SIE)) | (if (s & H_SIE) != 0 { H_SPIE } else { 0 }) | (if prv == 1 { H_SPP } else { 0 });
                prv = 1;
                pc = ((cs[S_STVEC] & h_not(3)) + (if irq && (cs[S_STVEC] & 1) != 0 { 4 * code } else { 0 })).wrap::<u64>();
            } else {
                cs[S_MCAUSE] = tc;
                cs[S_MEPC] = pc;
                cs[S_MTVAL] = tv;
                cs[S_MSTATUS] = (s & h_not(H_MPP | H_MPIE | H_MIE)) | (if (s & H_MIE) != 0 { H_MPIE } else { 0 }) | (prv << 11);
                prv = 3;
                pc = ((cs[S_MTVEC] & h_not(3)) + (if irq && (cs[S_MTVEC] & 1) != 0 { 4 * code } else { 0 })).wrap::<u64>();
            }
            check = true;
        }
    }
}

comb fn h_sext39(va: u64) -> u64 { va[38:0].wrap::<i39>().wrap::<u64>() }

/// the devices that answer (a read of anything else is an access fault)
comb fn h_dev(pa: u64) -> bool { (pa >> 26) == 3 || (pa >> 8) == 0x100000 || (pa >> 12) == 0x10060 || (pa >> 12) == 0x100 }

comb fn h_amo_ok(f5: u64) -> bool { f5 == 3 || f5 == 0 || f5 == 1 || f5 == 4 || f5 == 8 || f5 == 12 || f5 == 16 || f5 == 20 || f5 == 24 || f5 == 28 }

/// an AMO's new value from the old (sign-extended for W) and the operand
comb fn h_amo(f5: u64, old: u64, b: u64, size: u4) -> u64 {
    let (o, v) = if size == 4 { (old.wrap::<i32>().wrap::<i64>(), b.wrap::<i32>().wrap::<i64>()) } else { (old.wrap::<i64>(), b.wrap::<i64>()) };
    let (uo, uv) = if size == 4 { (old.wrap::<u32>().wrap::<u64>(), b.wrap::<u32>().wrap::<u64>()) } else { (old, b) };
    match f5 {
        0 => (old + b).wrap::<u64>(),
        1 => b,
        4 => old ^ b,
        8 => old | b,
        12 => old & b,
        16 => if o < v { old } else { b },
        20 => if o > v { old } else { b },
        24 => if uo < uv { old } else { b },
        _ => if uo > uv { old } else { b },
    }
}

/// the CLINT (0x0200_0000): msip at 0, mtimecmp at 0x4000, mtime at 0xBFF8 (4-byte accesses: halves)
comb fn h_clint_read(pa: u64, size: u4, mtime: u64, cmp: u64, msip: bool) -> u64 {
    let off = pa & 0xFFFF;
    let hi = (pa & 4) != 0;
    let v = if (off >> 3) == 0x17FF { mtime } else if (off >> 3) == 0x800 { cmp } else if off == 0 { if msip { 1 } else { 0 } } else { 0 };
    if size == 8 || ((off >> 3) != 0x17FF && (off >> 3) != 0x800) { v } else if hi { v >> 32 } else { v & 0xFFFFFFFF }
}

comb fn h_clint_write(pa: u64, size: u4, v: u64, mtime: u64, cmp: u64, msip: bool) -> (u64, u64, bool) {
    let off = pa & 0xFFFF;
    let hi = (pa & 4) != 0;
    if (off >> 3) == 0x800 { (mtime, h_set_half(cmp, v, size, hi), msip) }
    else if (off >> 3) == 0x17FF { (h_set_half(mtime, v, size, hi), cmp, msip) }
    else if off == 0 { (mtime, cmp, (v & 1) != 0) }
    else { (mtime, cmp, msip) }
}

/// a register of 8 bytes written by a 4- or 8-byte access (address bit 2 picks the half)
comb fn h_set_half(r: u64, v: u64, size: u4, hi: bool) -> u64 {
    if size == 8 { v } else if hi { (r & 0xFFFFFFFF) | ((v & 0xFFFFFFFF) << 32) } else { (r & 0xFFFFFFFF00000000) | (v & 0xFFFFFFFF) }
}

/// an instruction without memory: (legal, writes rd, the value, the next pc)
comb fn h_exec(i: u64, a: u64, b: u64, pc: u64, next: u64, imm_i: u64) -> (bool, bool, u64, u64) {
    let op = i & 0x7F;
    let f3 = i[14:12];
    let f7 = i >> 25;
    if op == 0x37 { (true, true, h_sext32(i & 0xFFFFF000), next) }
    else if op == 0x17 { (true, true, (pc + h_sext32(i & 0xFFFFF000)).wrap::<u64>(), next) }
    else if op == 0x6F {
        let imm = (((i >> 31) << 20) | (((i >> 12) & 0xFF) << 12) | (((i >> 20) & 1) << 11) | (((i >> 21) & 0x3FF) << 1)).wrap::<i21>().wrap::<u64>();
        (true, true, next, (pc + imm).wrap::<u64>())
    }
    else if op == 0x67 {
        if f3 != 0 { (false, false, 0, next) } else { (true, true, next, ((a + imm_i).wrap::<u64>()) & h_not(1)) }
    }
    else if op == 0x63 {
        let imm = (((i >> 31) << 12) | (((i >> 7) & 1) << 11) | (((i >> 25) & 0x3F) << 5) | (((i >> 8) & 15) << 1)).wrap::<i13>().wrap::<u64>();
        let ok = f3 != 2 && f3 != 3;
        let take = if f3 == 0 { a == b } else if f3 == 1 { a != b } else if f3 == 4 { h_s(a) < h_s(b) } else if f3 == 5 { h_s(a) >= h_s(b) } else if f3 == 6 { a < b } else { a >= b };
        (ok, false, 0, if ok && take { (pc + imm).wrap::<u64>() } else { next })
    }
    else if op == 0x13 {
        let sh = (i >> 20) & 63;
        if f3 == 1 { if (i >> 26) != 0 { (false, false, 0, next) } else { (true, true, h_shl(a, sh.wrap::<u6>()), next) } }
        else if f3 == 5 {
            if (i >> 26) == 0 { (true, true, a >> sh, next) }
            else if (i >> 26) == 0x10 { (true, true, (h_s(a) >> sh).wrap::<u64>(), next) }
            else { (false, false, 0, next) }
        }
        else {
            let v = if f3 == 0 { (a + imm_i).wrap::<u64>() } else if f3 == 2 { if h_s(a) < h_s(imm_i) { 1 } else { 0 } } else if f3 == 3 { if a < imm_i { 1 } else { 0 } }
                    else if f3 == 4 { a ^ imm_i } else if f3 == 6 { a | imm_i } else { a & imm_i };
            (true, true, v, next)
        }
    }
    else if op == 0x1B {
        let sh = (i >> 20) & 31;
        let x = a & 0xFFFFFFFF;
        if f3 == 0 { (true, true, h_sext32((a + imm_i).wrap::<u64>()), next) }
        else if f3 == 1 { if f7 != 0 { (false, false, 0, next) } else { (true, true, h_sext32(x << sh), next) } }
        else if f3 == 5 {
            if f7 == 0 { (true, true, h_sext32(x >> sh), next) }
            else if f7 == 0x20 { (true, true, (x.wrap::<i32>() >> sh).wrap::<u64>(), next) }
            else { (false, false, 0, next) }
        }
        else { (false, false, 0, next) }
    }
    else if op == 0x33 {
        let sh = (b & 63).wrap::<u6>();
        if f7 == 1 { (true, true, h_m(f3, a, b), next) }
        else if f7 == 0 {
            let v = if f3 == 0 { (a + b).wrap::<u64>() } else if f3 == 1 { h_shl(a, sh) } else if f3 == 2 { if h_s(a) < h_s(b) { 1 } else { 0 } }
                    else if f3 == 3 { if a < b { 1 } else { 0 } } else if f3 == 4 { a ^ b } else if f3 == 5 { a >> sh } else if f3 == 6 { a | b } else { a & b };
            (true, true, v, next)
        }
        else if f7 == 0x20 && f3 == 0 { (true, true, (a - b).wrap::<u64>(), next) }
        else if f7 == 0x20 && f3 == 5 { (true, true, (h_s(a) >> sh).wrap::<u64>(), next) }
        else { (false, false, 0, next) }
    }
    else if op == 0x3B {
        let x = a & 0xFFFFFFFF;
        let y = b & 31;
        if f7 == 1 { if f3 == 0 || f3 >= 4 { (true, true, h_mw(f3, a, b), next) } else { (false, false, 0, next) } }
        else if f7 == 0 && f3 == 0 { (true, true, h_sext32((a + b).wrap::<u64>()), next) }
        else if f7 == 0x20 && f3 == 0 { (true, true, h_sext32((a - b).wrap::<u64>()), next) }
        else if f7 == 0 && f3 == 1 { (true, true, h_sext32(x << y), next) }
        else if f7 == 0 && f3 == 5 { (true, true, h_sext32(x >> y), next) }
        else if f7 == 0x20 && f3 == 5 { (true, true, (x.wrap::<i32>() >> y).wrap::<u64>(), next) }
        else { (false, false, 0, next) }
    }
    else if op == 0x0F { (f3 == 0 || f3 == 1, false, 0, next) }
    else { (false, false, 0, next) }
}

// ════════════════════════════════════════════════════════════════════════════
//  the devices
// ════════════════════════════════════════════════════════════════════════════

/// the UART, the PLIC, GPIO and the finisher (riscv/rvemu/src/bus.rs, register for register).
/// Requests from the hart: (kind, address, size, data); each answer: (data, MEIP, SEIP).
/// The outside: `env_req` asks it (true: wait until something comes), `env` answers
/// (a UART byte or none, the GPIO pins driven from outside); `tx` the UART's bytes, `gpio` the pins
/// after each change, `fin` the finisher's code.
task Devices(req: in chan<(u2, u64, u4, u64)>, resp: out chan<(u64, bool, bool)>,
             env_req: out chan<bool>, env: in chan<(bool, u8, u16)>,
             tx: out chan<u8>, gpio: out chan<u16>, fin: out chan<u32>) {
    // UART
    let mut ier: u8 = 0;
    let mut lcr: u8 = 0;
    let mut mcr: u8 = 0;
    let mut scr: u8 = 0;
    let mut fcr: bool = false;
    let mut dll: u8 = 0;
    let mut dlm: u8 = 0;
    let mut rx_full: bool = false;
    let mut rx_byte: u8 = 0;
    let mut thre_pend: bool = false;
    let mut pend: bool = false;              // a byte from outside not taken yet
    let mut pend_byte: u8 = 0;
    // GPIO
    let mut g: [u16; 17] = [0; 17];          // the registers 0..16 (value at 0 = iv, the pins as last seen)
    let mut pins_in: u16 = 0;
    let mut last_pins: u16 = 0;
    // PLIC
    let mut prio: [u3; 32] = [0; 32];
    let mut en0: u32 = 0;
    let mut en1: u32 = 0;
    let mut thr0: u3 = 0;
    let mut thr1: u3 = 0;
    let mut claimed: u32 = 0;

    for (k, a, size, v) in req {
        let mut out: u64 = 0;
        if k == D_POLL || k == D_WAIT {
            // the outside: a byte (when there is room for one) and the input pins
            env_req.send(k == D_WAIT);
            let (has, b, pins) = env.recv();
            if has && !pend { pend = true; pend_byte = b; }
            pins_in = pins;
            g = d_gpio_update(g, pins_in);
        } else if (a >> 26) == 3 {
            // PLIC
            let off = a & 0x3FFFFFF;
            let src = d_src(g, ier, rx_full, thre_pend);
            if k == D_READ {
                if (off >> 7) == 0 { out = prio[((off >> 2) & 31).wrap::<u32>()]; }
                else if off == 0x1000 { out = src & d_not32(claimed); }
                else if off == 0x2000 { out = en0; }
                else if off == 0x2080 { out = en1; }
                else if off == 0x200000 { out = thr0; }
                else if off == 0x201000 { out = thr1; }
                else if off == 0x200004 || off == 0x201004 {
                    let (best, _) = d_best(prio, src, claimed, if off == 0x200004 { en0 } else { en1 });
                    if best != 0 { claimed = claimed | (1 << best).wrap::<u32>(); }
                    out = best;
                }
            } else {
                if (off >> 7) == 0 { prio[((off >> 2) & 31).wrap::<u32>()] = (v & 7).wrap::<u3>(); }
                else if off == 0x2000 { en0 = v.wrap::<u32>(); }
                else if off == 0x2080 { en1 = v.wrap::<u32>(); }
                else if off == 0x200000 { thr0 = (v & 7).wrap::<u3>(); }
                else if off == 0x201000 { thr1 = (v & 7).wrap::<u3>(); }
                else if off == 0x200004 || off == 0x201004 { claimed = claimed & d_not32((1 << (v & 31)).wrap::<u32>()); }
            }
        } else if (a >> 8) == 0x100000 {
            // UART
            let dlab = (lcr & 0x80) != 0;
            let r = a & 7;
            if k == D_READ {
                let rx_int = (ier & 1) != 0 && rx_full;
                let tx_int = (ier & 2) != 0 && thre_pend;
                if r == 0 { if dlab { out = dll; } else { rx_full = false; out = rx_byte; } }
                else if r == 1 { out = if dlab { dlm } else { ier }; }
                else if r == 2 {
                    if !rx_int && tx_int { thre_pend = false; }
                    out = (if fcr { 0xC0 } else { 0 }) | (if rx_int { 4 } else if tx_int { 2 } else { 1 });
                }
                else if r == 3 { out = lcr; }
                else if r == 4 { out = mcr; }
                else if r == 5 { out = 0x60 | (if rx_full { 1 } else { 0 }); }
                else if r == 6 { out = 0; }
                else { out = scr; }
            } else {
                let b = v.wrap::<u8>();
                if r == 0 { if dlab { dll = b; } else { tx.send(b); thre_pend = true; } }
                else if r == 1 { if dlab { dlm = b; } else { if (b & 2) != 0 && (ier & 2) == 0 { thre_pend = true; } ier = b & 15; } }
                else if r == 2 { fcr = (b & 1) != 0; }
                else if r == 3 { lcr = b; }
                else if r == 4 { mcr = b; }
                else if r == 7 { scr = b; }
            }
        } else if (a >> 12) == 0x10060 {
            // GPIO
            let r: u32 = ((a & 0xFF) >> 2).wrap::<u32>();
            if k == D_READ {
                if r <= 16 { out = g[r]; }
            } else {
                let w = v.wrap::<u16>();
                if r == 7 || r == 9 || r == 11 || r == 13 { g[r] = g[r] & (w ^ 0xFFFF); }
                else if r >= 1 && r <= 16 { g[r] = w; }
                g = d_gpio_update(g, pins_in);
                let p = d_pins(g, pins_in);
                if p != last_pins { last_pins = p; gpio.send(p); }
            }
        } else if (a >> 12) == 0x100 {
            // the finisher: a write of a non-zero low half
            if k == D_WRITE && (a & 0xFFF) == 0 && (v & 0xFFFF) != 0 { fin.send(v.wrap::<u32>()); }
        }
        // the receive buffer takes a byte waiting (after a read of it, or from outside)
        if pend && !rx_full { rx_full = true; rx_byte = pend_byte; pend = false; }
        let src = d_src(g, ier, rx_full, thre_pend);
        let (_, p0) = d_best(prio, src, claimed, en0);
        let (_, p1) = d_best(prio, src, claimed, en1);
        resp.send((out, src != 0 && p0 > thr0, src != 0 && p1 > thr1));
    }
}

comb fn d_not32(x: u32) -> u32 { x ^ 0xFFFFFFFF }

/// the pins: the hart's where the output is enabled, else from outside
comb fn d_pins(g: [u16; 17], pins_in: u16) -> u16 {
    let out = g[3] ^ g[16];
    (g[2] & out) | ((g[2] ^ 0xFFFF) & pins_in)
}

/// look at the pins: edges and levels into the pending bits (input enable applied)
comb fn d_gpio_update(g: [u16; 17], pins_in: u16) -> [u16; 17] {
    let mut h = g;
    let iv = d_pins(g, pins_in) & g[1];
    h[7] = g[7] | (iv & (g[0] ^ 0xFFFF));
    h[9] = g[9] | ((iv ^ 0xFFFF) & g[0]);
    h[11] = g[11] | iv;
    h[13] = g[13] | ((iv ^ 0xFFFF) & g[1]);
    h[0] = iv;
    h
}

/// the PLIC's sources: GPIO pins 16..31, the UART 10
comb fn d_src(g: [u16; 17], ier: u8, rx_full: bool, thre_pend: bool) -> u32 {
    let gi = (g[7] & g[6]) | (g[9] & g[8]) | (g[11] & g[10]) | (g[13] & g[12]);
    let uart = ((ier & 1) != 0 && rx_full) || ((ier & 2) != 0 && thre_pend);
    (gi.wrap::<u32>() << 16) | (if uart { 0x400 } else { 0 })
}

/// the highest-priority pending source of a context and its priority
comb fn d_best(prio: [u3; 32], src: u32, claimed: u32, en: u32) -> (u32, u3) {
    let pend = src & d_not32(claimed) & en & 0xFFFFFFFE;
    let mut best: u32 = 0;
    let mut bp: u3 = 0;
    for i in 1..32 {
        if ((pend >> i) & 1) == 1 && prio[i] > bp { best = i; bp = prio[i]; }
    }
    (best, bp)
}

// ════════════════════════════════════════════════════════════════════════════
//  the machine
// ════════════════════════════════════════════════════════════════════════════

/// the hart and the devices; the RAM gets `load`'s words first
task Soc<const RAMW: u32, const TRACE: bool>(load: in chan<(u32, u64)>,
        env_req: out chan<bool>, env: in chan<(bool, u8, u16)>,
        tx: out chan<u8>, gpio: out chan<u16>, fin: out chan<u32>,
        trace: out chan<(u2, u64, u32, bool, u5, u64)>) {
    let (qt, qr) = chan::<(u2, u64, u4, u64)>(depth = 1);
    let (rt, rr) = chan::<(u64, bool, bool)>(depth = 1);
    spawn Hart::<RAMW, TRACE>(load, qt, rr, trace);
    spawn Devices(qr, rt, env_req, env, tx, gpio, fin);
}
