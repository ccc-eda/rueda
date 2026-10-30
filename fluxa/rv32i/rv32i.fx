// ════════════════════════════════════════════════════════════════════════════
//  RV32I 處理器（Fluxa）
//
//   Fetch ──(pc,inst)──► Decode ──(pc,inst,cls,a,b)──► Execute ──(kind,rd,val,addr,sdata,f3)──► Mem
//     ▲                    ▲  │                           │                                      │
//     └──── redirect ──────┼──┼───────────────────────────┘                                      │
//        (分支/JALR 目標)   └──┴────────────────── wb (rd, value) ◄──────────────────────────────┘
//
//   • 四個 task 以有界 FIFO 相連，是一個 Kahn 程序網路：結果與排程、延遲無關。
//   • Decode 擁有暫存器檔與記分板；資料相依以「來源暫存器尚有未完成寫入 → 等寫回」解決。
//   • 控制相依以「停等」解決：分支與 JALR 在 Execute 解析，Fetch 阻塞等待重導向 token。
//   • 結束協定：ECALL / EBREAK / 非法指令會讓 Decode 傾印 36 個 token（見 D_* 常數）。
// ════════════════════════════════════════════════════════════════════════════

// ── 參數 ────────────────────────────────────────────────────────────────────
const IWORDS: u32 = 64;               // 指令記憶體字數（256 位元組）
const DWORDS: u32 = 64;               // 資料記憶體字數（256 位元組）
const MMIO_OUT: u32 = 0x10000000;     // 對此位址 SW → 送一個 token 到 io 通道
const FETCH_LIMIT: u32 = 3000;        // 看門狗：超過就當作非法指令陷阱
const MAX_INFLIGHT: u32 = 16;         // 記分板排空迴圈的上界（須 ≥ 管線中可同時未完成的寫入數）
const WB_DEPTH: u32 = 8;              // 寫回 FIFO 深度（下限分析見 README）

// 傾印格式
const DUMP_LEN: u32 = 36;
const D_CAUSE: u32 = 0;               // 0 = ECALL, 1 = EBREAK, 2 = 非法指令／看門狗
const D_PC: u32 = 1;                  // 陷阱指令的 pc
const D_RETIRED: u32 = 2;             // 陷阱前已發射的指令數
const D_STALLS: u32 = 3;              // Decode 因資料相依等待寫回的次數
const D_X: u32 = 4;                   // x0..x31

// 指令類別（classify 的回傳值）
const C_ILLEGAL: u4 = 0;
const C_LUI: u4 = 1;
const C_AUIPC: u4 = 2;
const C_JAL: u4 = 3;
const C_JALR: u4 = 4;
const C_BRANCH: u4 = 5;
const C_LOAD: u4 = 6;
const C_STORE: u4 = 7;
const C_OPIMM: u4 = 8;
const C_OP: u4 = 9;
const C_FENCE: u4 = 10;
const C_HALT: u4 = 11;

// ════════════════════════════════════════════════════════════════════════════
//  組譯器：每個助憶碼都是一個函式，參數型別即編碼欄位寬度，
//  所以 addi(1, 0, 3000) 這種超出 i12 的立即值會在組譯時就被位寬檢查擋下。
// ════════════════════════════════════════════════════════════════════════════
fn enc_r(f7: u7, rs2: u5, rs1: u5, f3: u3, rd: u5, op: u7) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}
fn enc_i(imm: i12, rs1: u5, f3: u3, rd: u5, op: u7) -> u32 {
    (imm.wrap::<u12>() << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}
fn enc_s(imm: i12, rs2: u5, rs1: u5, f3: u3, op: u7) -> u32 {
    let u = imm.wrap::<u12>();
    (u[11:5] << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (u[4:0] << 7) | op
}
fn enc_b(imm: i13, rs2: u5, rs1: u5, f3: u3, op: u7) -> u32 {
    assert imm[0:0] == 0, "branch offset must be even";
    let u = imm.wrap::<u13>();
    (u[12:12] << 31) | (u[10:5] << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12)
        | (u[4:1] << 8) | (u[11:11] << 7) | op
}
fn enc_u(imm: u20, rd: u5, op: u7) -> u32 {
    (imm << 12) | (rd << 7) | op
}
fn enc_j(imm: i21, rd: u5, op: u7) -> u32 {
    assert imm[0:0] == 0, "jump offset must be even";
    let u = imm.wrap::<u21>();
    (u[20:20] << 31) | (u[10:1] << 21) | (u[11:11] << 20) | (u[19:12] << 12) | (rd << 7) | op
}

fn add(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 0, rd, 0x33) }
fn sub(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0x20, rs2, rs1, 0, rd, 0x33) }
fn sll(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 1, rd, 0x33) }
fn slt(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 2, rd, 0x33) }
fn sltu(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 3, rd, 0x33) }
fn xor(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 4, rd, 0x33) }
fn srl(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 5, rd, 0x33) }
fn sra(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0x20, rs2, rs1, 5, rd, 0x33) }
fn or(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 6, rd, 0x33) }
fn and(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 7, rd, 0x33) }

fn addi(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 0, rd, 0x13) }
fn slti(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 2, rd, 0x13) }
fn sltiu(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 3, rd, 0x13) }
fn xori(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 4, rd, 0x13) }
fn ori(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 6, rd, 0x13) }
fn andi(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 7, rd, 0x13) }
fn slli(rd: u5, rs1: u5, sh: u5) -> u32 { enc_i(sh, rs1, 1, rd, 0x13) }
fn srli(rd: u5, rs1: u5, sh: u5) -> u32 { enc_i(sh, rs1, 5, rd, 0x13) }
fn srai(rd: u5, rs1: u5, sh: u5) -> u32 { enc_i(1024 + sh, rs1, 5, rd, 0x13) }

fn lb(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 0, rd, 0x03) }
fn lh(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 1, rd, 0x03) }
fn lw(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 2, rd, 0x03) }
fn lbu(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 4, rd, 0x03) }
fn lhu(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 5, rd, 0x03) }
fn sb(rs2: u5, rs1: u5, off: i12) -> u32 { enc_s(off, rs2, rs1, 0, 0x23) }
fn sh(rs2: u5, rs1: u5, off: i12) -> u32 { enc_s(off, rs2, rs1, 1, 0x23) }
fn sw(rs2: u5, rs1: u5, off: i12) -> u32 { enc_s(off, rs2, rs1, 2, 0x23) }

fn beq(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 0, 0x63) }
fn bne(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 1, 0x63) }
fn blt(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 4, 0x63) }
fn bge(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 5, 0x63) }
fn bltu(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 6, 0x63) }
fn bgeu(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 7, 0x63) }

fn lui(rd: u5, imm: u20) -> u32 { enc_u(imm, rd, 0x37) }
fn auipc(rd: u5, imm: u20) -> u32 { enc_u(imm, rd, 0x17) }
fn jal(rd: u5, off: i21) -> u32 { enc_j(off, rd, 0x6F) }
fn jalr(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 0, rd, 0x67) }

fn ecall() -> u32 { 0x00000073 }
fn ebreak() -> u32 { 0x00100073 }
fn fence() -> u32 { 0x0FF0000F }
fn nop() -> u32 { addi(0, 0, 0) }

/// 載入 32 位元常數：lui + addi（補償 addi 的符號擴展）
fn li(rd: u5, imm: u32) -> [u32; 2] {
    let hi = ((imm + 0x800) >> 12) & 0xFFFFF;
    let lo = (imm & 0xFFF).wrap::<i12>();
    [lui(rd, hi), addi(rd, rd, lo)]
}

// ════════════════════════════════════════════════════════════════════════════
//  解碼（全是 comb fn：零週期、無狀態）
// ════════════════════════════════════════════════════════════════════════════
comb fn imm_i(inst: u32) -> i32 { inst[31:20].wrap::<i12>() }
comb fn imm_s(inst: u32) -> i32 { ((inst[31:25] << 5) | inst[11:7]).wrap::<i12>() }
comb fn imm_b(inst: u32) -> i32 {
    ((inst[31:31] << 12) | (inst[7:7] << 11) | (inst[30:25] << 5) | (inst[11:8] << 1)).wrap::<i13>()
}
comb fn imm_u(inst: u32) -> u32 { inst[31:12] << 12 }
comb fn imm_j(inst: u32) -> i32 {
    ((inst[31:31] << 20) | (inst[19:12] << 12) | (inst[20:20] << 11) | (inst[30:21] << 1)).wrap::<i21>()
}

/// 指令分類，同時是合法性檢查：保留的 funct 組合一律歸為 C_ILLEGAL
comb fn classify(inst: u32) -> u4 {
    let op = inst[6:0];
    let f3 = inst[14:12];
    let f7 = inst[31:25];
    match op {
        0x37 => C_LUI,
        0x17 => C_AUIPC,
        0x6F => C_JAL,
        0x67 => if f3 == 0 { C_JALR } else { C_ILLEGAL },
        0x63 => if f3 == 2 || f3 == 3 { C_ILLEGAL } else { C_BRANCH },
        0x03 => if f3 == 3 || f3 > 5 { C_ILLEGAL } else { C_LOAD },
        0x23 => if f3 > 2 { C_ILLEGAL } else { C_STORE },
        0x13 => if f3 == 1 {
                    if f7 == 0 { C_OPIMM } else { C_ILLEGAL }
                } else if f3 == 5 {
                    if f7 == 0 || f7 == 0x20 { C_OPIMM } else { C_ILLEGAL }
                } else { C_OPIMM },
        0x33 => if f7 == 0 || (f7 == 0x20 && (f3 == 0 || f3 == 5)) { C_OP } else { C_ILLEGAL },
        0x0F => C_FENCE,
        0x73 => if inst == 0x00000073 || inst == 0x00100073 { C_HALT } else { C_ILLEGAL },
        _ => C_ILLEGAL,
    }
}

comb fn uses_rs1(c: u4) -> bool {
    c == C_JALR || c == C_BRANCH || c == C_LOAD || c == C_STORE || c == C_OPIMM || c == C_OP
}
comb fn uses_rs2(c: u4) -> bool { c == C_BRANCH || c == C_STORE || c == C_OP }
comb fn writes_rd(c: u4) -> bool {
    c == C_LUI || c == C_AUIPC || c == C_JAL || c == C_JALR || c == C_LOAD || c == C_OPIMM || c == C_OP
}
comb fn trap_cause(inst: u32) -> u2 {
    if inst == 0x00000073 { 0 } else if inst == 0x00100073 { 1 } else { 2 }
}

/// 整數 ALU：f3 選運算，alt 區分 ADD/SUB 與 SRL/SRA
comb fn alu(f3: u3, alt: bit, a: u32, b: u32) -> u32 {
    let sh = b[4:0];
    match f3 {
        0 => if alt == 1 { (a - b).wrap::<u32>() } else { (a + b).wrap::<u32>() },
        1 => (a << sh).wrap::<u32>(),
        2 => (a.wrap::<i32>() < b.wrap::<i32>()) as u32,
        3 => (a < b) as u32,
        4 => a ^ b,
        5 => if alt == 1 { (a.wrap::<i32>() >> sh).wrap::<u32>() } else { a >> sh },
        6 => a | b,
        _ => a & b,
    }
}

comb fn branch_taken(f3: u3, a: u32, b: u32) -> bool {
    match f3 {
        0 => a == b,
        1 => a != b,
        4 => a.wrap::<i32>() < b.wrap::<i32>(),
        5 => a.wrap::<i32>() >= b.wrap::<i32>(),
        6 => a < b,
        _ => a >= b,
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  管線
// ════════════════════════════════════════════════════════════════════════════

/// 取指：持有指令記憶體與 pc。
///   JAL         → 自己算目標，不停等
///   分支 / JALR → 等 Execute 送回下一個 pc（沒有分支預測）
///   ECALL / 非法 → 送出後停止
task Fetch(prog: [u32; IWORDS], to_dec: out chan<(u32, u32)>, redirect: in chan<u32>) {
    carry pc: u32 = 0;
    serial for _ in 0..FETCH_LIMIT {
        let word = pc >> 2;
        let inst = if (pc & 3) == 0 && word < IWORDS { prog[word] } else { 0 };   // 越界／未對齊 → 0 = 非法
        to_dec.send((pc, inst));
        let c = classify(inst);
        if c == C_HALT || c == C_ILLEGAL {
            return;
        }
        if c == C_JAL {
            pc = (pc + imm_j(inst)).wrap::<u32>();
        } else if c == C_JALR || c == C_BRANCH {
            pc = redirect.recv();
        } else {
            pc = (pc + 4).wrap::<u32>();
        }
    }
    to_dec.send((pc, 0));     // 看門狗到期：以非法指令陷阱收場
}

/// 解碼 + 暫存器檔 + 記分板
///   pend[r] = 已發射但尚未寫回的寫入數。來源暫存器的 pend 非零就停發，
///   改為接收寫回 token（寫回順序＝程式順序），直到相依解除。
task Decode(ifq: in chan<(u32, u32)>,
            to_ex: out chan<(u32, u32, u4, u32, u32)>,
            wbq: in chan<(u5, u32)>,
            dump: out chan<u32>) {
    let mut x: [u32; 32] = [0; 32];
    let mut pend: [u4; 32] = [0; 32];
    let mut inflight: u5 = 0;
    let mut retired: u32 = 0;
    let mut stalls: u32 = 0;

    for (pc, inst) in ifq {
        let c = classify(inst);
        let rd = inst[11:7];
        let rs1 = inst[19:15];
        let rs2 = inst[24:20];
        let u1 = uses_rs1(c);
        let u2 = uses_rs2(c);

        // 資料相依：等到來源暫存器沒有未完成的寫入
        for _ in 0..MAX_INFLIGHT {
            if (u1 && pend[rs1] != 0) || (u2 && pend[rs2] != 0) {
                let (r, v) = wbq.recv();
                x[r] = v;
                pend[r] = pend[r] - 1;
                inflight = inflight - 1;
                stalls += 1;
            }
        }

        // 陷阱：排空管線、傾印狀態、結束（return 會讓下游任務隨通道關閉而依序結束）
        if c == C_HALT || c == C_ILLEGAL {
            for _ in 0..MAX_INFLIGHT {
                if inflight != 0 {
                    let (r, v) = wbq.recv();
                    x[r] = v;
                    pend[r] = pend[r] - 1;
                    inflight = inflight - 1;
                }
            }
            dump.send(trap_cause(inst));
            dump.send(pc);
            dump.send(retired);
            dump.send(stalls);
            for i in 0..32 { dump.send(x[i]); }
            return;
        }

        if writes_rd(c) && rd != 0 {
            pend[rd] = pend[rd] + 1;
            inflight = inflight + 1;
        }
        retired += 1;
        to_ex.send((pc, inst, c, x[rs1], x[rs2]));
    }
}

/// 執行：ALU、分支判斷、位址計算。分支與 JALR 把下一個 pc 送回 Fetch。
task Execute(from_dec: in chan<(u32, u32, u4, u32, u32)>,
             to_mem: out chan<(u2, u5, u32, u32, u32, u3)>,
             redirect: out chan<u32>) {
    for (pc, inst, c, a, b) in from_dec {
        let rd = inst[11:7];
        let f3 = inst[14:12];
        let alt = inst[30:30];
        let link = (pc + 4).wrap::<u32>();

        // kind：0 = 無寫回，1 = 寫回 val，2 = 載入（寫回 Mem 讀到的值），3 = 儲存
        let (kind, val, addr, sdata) = match c {
            C_LUI => (1, imm_u(inst), 0, 0),
            C_AUIPC => (1, (pc + imm_u(inst)).wrap::<u32>(), 0, 0),
            C_JAL => (1, link, 0, 0),
            C_JALR => {
                redirect.send(((a + imm_i(inst)).wrap::<u32>()) & 0xFFFFFFFE);
                (1, link, 0, 0)
            }
            C_BRANCH => {
                let taken = branch_taken(f3, a, b);
                redirect.send(if taken { (pc + imm_b(inst)).wrap::<u32>() } else { link });
                (0, 0, 0, 0)
            }
            C_LOAD => (2, 0, (a + imm_i(inst)).wrap::<u32>(), 0),
            C_STORE => (3, 0, (a + imm_s(inst)).wrap::<u32>(), b),
            C_OPIMM => (1, alu(f3, if f3 == 5 { alt } else { 0 }, a, imm_i(inst).wrap::<u32>()), 0, 0),
            C_OP => (1, alu(f3, alt, a, b), 0, 0),
            _ => (0, 0, 0, 0),
        };

        // 目的暫存器是 x0 的指令不寫回（Decode 的記分板也不會為它計數）
        let k = if (kind == 1 || kind == 2) && rd == 0 { 0 } else { kind };
        to_mem.send((k, rd, val, addr, sdata, f3));
    }
}

/// 記憶體階段：持有資料記憶體（小端、位元組定址）。
///   對 MMIO_OUT 的 SW 不寫記憶體，而是送一個 token 到 io 通道。
task Mem(from_ex: in chan<(u2, u5, u32, u32, u32, u3)>,
         wb: out chan<(u5, u32)>,
         io: out chan<u32>,
         data: [u32; DWORDS]) {
    let mut dmem: mem<u32, DWORDS> = data;

    for (kind, rd, val, addr, sdata, f3) in from_ex {
        let word = addr >> 2;
        let sh = addr[1:0] * 8;

        if kind == 1 {
            wb.send((rd, val));
        } else if kind == 2 {
            let bad = (f3 == 2 && addr[1:0] != 0) || ((f3 == 1 || f3 == 5) && addr[0:0] != 0);
            assert !bad, "misaligned load";
            assert word < DWORDS, "load address out of range";
            let w = dmem[word];
            let part = w >> sh;
            let v = match f3 {
                0 => part[7:0].wrap::<i8>().wrap::<u32>(),      // LB
                1 => part[15:0].wrap::<i16>().wrap::<u32>(),    // LH
                2 => w,                                         // LW
                4 => part[7:0],                                 // LBU
                _ => part[15:0],                                // LHU
            };
            wb.send((rd, v));
        } else if kind == 3 {
            if addr == MMIO_OUT {
                io.send(sdata);
            } else {
                let bad = (f3 == 2 && addr[1:0] != 0) || (f3 == 1 && addr[0:0] != 0);
                assert !bad, "misaligned store";
                assert word < DWORDS, "store address out of range";
                let m = match f3 { 0 => 0xFF, 1 => 0xFFFF, _ => 0xFFFFFFFF };
                let w = dmem[word];
                dmem[word] = (w & ~(m << sh)) | ((sdata & m) << sh);
            }
        }
    }
}

/// 頂層：把四個階段用 FIFO 接起來。
/// WBD = 寫回 FIFO 深度。太小會讓 Mem 在 Decode 來不及排空時卡住 → 死結（見 err_wb_depth.fx）。
task Core<const WBD: u32>(prog: [u32; IWORDS], data: [u32; DWORDS],
                          io: out chan<u32>, dump: out chan<u32>) {
    assert WBD <= 8, "WBD > 8 could overflow the 4-bit scoreboard counters";
    let (f2d_tx, f2d_rx) = chan::<(u32, u32)>(depth = 1);
    let (d2e_tx, d2e_rx) = chan::<(u32, u32, u4, u32, u32)>(depth = 1);
    let (e2m_tx, e2m_rx) = chan::<(u2, u5, u32, u32, u32, u3)>(depth = 1);
    let (wb_tx, wb_rx)   = chan::<(u5, u32)>(depth = WBD);
    let (rd_tx, rd_rx)   = chan::<u32>(depth = 1);

    spawn Fetch(prog, f2d_tx, rd_rx);
    spawn Decode(f2d_rx, d2e_tx, wb_rx, dump);
    spawn Execute(d2e_rx, e2m_tx, rd_tx);
    spawn Mem(e2m_rx, wb_tx, io, data);
}

// ════════════════════════════════════════════════════════════════════════════
//  測試用 harness：載入程式、跑到陷阱、回傳 (傾印, MMIO 輸出)
// ════════════════════════════════════════════════════════════════════════════
fn image(p: _) -> [u32; IWORDS] { concat(p, [0; IWORDS - len(p)]) }      // 不足的部分補 0（= 非法指令）
fn dimage(d: _) -> [u32; DWORDS] { concat(d, [0; DWORDS - len(d)]) }

fn run_with(p: _, d: _) {
    // io 深度：MMIO 寫入次數 ≤ 已發射指令數 ≤ FETCH_LIMIT，
    // 且測試端會先把 dump 收完才收 io，所以 io 必須裝得下全部輸出，否則管線未到陷阱就卡死。
    let (io_tx, io_rx) = chan::<u32>(depth = FETCH_LIMIT);
    let (dump_tx, dump_rx) = chan::<u32>(depth = 40);
    spawn Core::<WB_DEPTH>(image(p), dimage(d), io_tx, dump_tx);
    let mut regs: [u32; DUMP_LEN] = [0; DUMP_LEN];
    for i in 0..DUMP_LEN { regs[i] = dump_rx.recv(); }
    let mut out = [];
    for v in io_rx { out = concat(out, [v]); }       // Mem 結束時關閉 io
    (regs, out)
}
fn run(p: _) { run_with(p, []) }

fn cause(r: _) -> u32 { r.0[D_CAUSE] }
fn trap_pc(r: _) -> u32 { r.0[D_PC] }
fn retired(r: _) -> u32 { r.0[D_RETIRED] }
fn stalls(r: _) -> u32 { r.0[D_STALLS] }
fn reg(r: _, n: u32) -> u32 { r.0[D_X + n] }
fn output(r: _) { r.1 }