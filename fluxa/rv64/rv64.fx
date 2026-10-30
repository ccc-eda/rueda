// ════════════════════════════════════════════════════════════════════════════
//  RV64G 處理器（Fluxa）— v0.3 里程碑 M1–M6（浮點運算單元在 fpu.fx）
//
//   Fetch ──(pc,inst)──► Decode ──(pc,inst,cls,a,b)──► Execute ──► Mem ──► WB
//
//   與 rv32i 相同的骨架：四級 task + 深度 1 FIFO、Decode 持暫存器檔與記分板、
//   分支/JALR 停等 redirect、狀態傾印收尾。差異：
//   • 資料路徑 64 位元；W 型指令（結果符號擴展）與 M 擴充（含 W 型變體）。
//   • 精確整數上限 120 位元 ⇒ 64 位元左移分兩段（sll64）、
//     64×64 乘積用 32 位元學校乘法分塊（mul_hi_lo）。
//   • M2：Zicsr + M 模式 trap —— CSR 檔在 Decode；所有例外（非法/ecall/ebreak/
//     未對齊/越界存取/CSR 違例）都在 Decode 偵測，trap 以「跳到 mtvec」的 token
//     經 Execute 送回 Fetch（與分支同一條 redirect 通道，不需要新通道）；
//     mret 經同一機制跳回 mepc。結束模擬 = 存取自訂 CSR 0x7C0（simexit）。
//   • M3：S/U 模式（medeleg 委派、sret、sstatus/sie 視窗、MPRV/SUM/MXR/TVM/TSR）
//     + Sv39 MMU。翻譯在 Decode（統一 TLB + 硬體頁表走訪）：走訪讀寫以 in-band
//     token 經 Execute → Mem（自動排在先前的 store 之後），回應走 wbq（先排空記分板，
//     下一個 wb token 必是走訪結果）。Fetch 有一筆 ITLB，未命中時送 F_XLATE 請 Decode
//     翻譯 pc 所在頁，經 redirect 收到「實體頁 | 1」或取指頁錯誤的 trap 目標。
// ════════════════════════════════════════════════════════════════════════════

// ── 參數 ────────────────────────────────────────────────────────────────────
const IWORDS: u32 = 256;              // 指令記憶體字數（32 位元字 = 1 KB，實體位址 0 起）
const DWORDS: u32 = 2048;             // 資料記憶體字數（64 位元字 = 16 KB = 4 頁，放得下三層頁表）
const MMIO_OUT: u32 = 0x10000000;     // 對此位址 SW → 送一個 token 到 io 通道
const FETCH_LIMIT: u32 = 3000;        // 看門狗：超過就當作非法指令陷阱
const MAX_INFLIGHT: u32 = 16;         // 記分板排空迴圈的上界
const WB_DEPTH: u32 = 8;              // 寫回 FIFO 深度
const TLB_N: u32 = 8;                 // 統一 TLB 項數（只影響走訪次數，不影響輸出）

// 傾印格式
const DUMP_LEN: u32 = 69;
const D_CAUSE: u32 = 0;               // 0 = ECALL, 1 = EBREAK, 2 = 非法指令／看門狗
const D_PC: u32 = 1;
const D_RETIRED: u32 = 2;
const D_STALLS: u32 = 3;
const D_X: u32 = 4;                   // x0..x31
const D_PRIV: u32 = 36;               // 結束時的特權模式（0 = U, 1 = S, 3 = M）
const D_F: u32 = 37;                  // f0..f31（64 位元原始位元模式）

// 指令類別（classify 的回傳值）
const C_ILLEGAL: u5 = 0;
const C_LUI: u5 = 1;
const C_AUIPC: u5 = 2;
const C_JAL: u5 = 3;
const C_JALR: u5 = 4;
const C_BRANCH: u5 = 5;
const C_LOAD: u5 = 6;
const C_STORE: u5 = 7;
const C_OPIMM: u5 = 8;
const C_OP: u5 = 9;
const C_OPIMM32: u5 = 10;             // OP-IMM-32（addiw/slliw/srliw/sraiw）
const C_OP32: u5 = 11;                // OP-32（addw/subw/sllw/srlw/sraw）
const C_M: u5 = 12;                   // M 擴充（64 位元）
const C_M32: u5 = 13;                 // M 擴充 W 型（mulw/divw/…）
const C_FENCE: u5 = 14;
const C_ECALL: u5 = 15;
const C_EBREAK: u5 = 16;
const C_MRET: u5 = 17;
const C_ZICSR: u5 = 18;               // csrrw/s/c 與立即型
const C_TRAP: u5 = 19;                // Decode 製造的「跳到 trap 目標」token
const C_CSR: u5 = 20;                 // Decode 製造的「把 a 寫回 rd」token
const C_SRET: u5 = 21;
const C_SFENCE: u5 = 22;              // sfence.vma
const C_XLATE: u5 = 23;               // Fetch 的翻譯請求（Decode 內部）／回應 token（送 Execute）
const C_PTW_R: u5 = 24;               // Decode 製造的走訪讀取（LD）token
const C_PTW_W: u5 = 25;               // Decode 製造的 A/D 回寫（SD）token
const C_AMO: u5 = 26;                 // A 擴充：LR/SC/AMO*
const C_FLOAD: u5 = 27;               // FLW/FLD
const C_FSTORE: u5 = 28;              // FSW/FSD
const C_FP: u5 = 29;                  // OP-FP（fpu.fx 的 fp_op）
const C_FMA: u5 = 30;                 // FMADD/FMSUB/FNMSUB/FNMADD

// Fetch → Decode 的 token 標籤
const F_INST: u2 = 0;                 // (pc, inst)：一條指令
const F_XLATE: u2 = 1;                // 請 Decode 翻譯 pc 所在頁
const F_WDOG: u2 = 2;                 // 看門狗到期
const F_IFAULT: u2 = 3;               // 取指實體位址超出指令 ROM（Decode 報 instruction access fault）

// redirect 值的低兩位：bit 0 = 取指翻譯回應（實體頁 | 1）；bit 1 = 翻譯情境可能改變
// （trap、xRET、sfence.vma、CSR 寫入）→ Fetch 讓 ITLB 失效。真正的目標 pc 必 4 對齊。
const R_CTX: u64 = 2;
const MTIMECMP_CSR: u12 = 0x7C1;      // 自訂 M 模式 CSR：mtimecmp（取代 CLINT 的 MMIO 暫存器）

const WDOG_PC: u64 = 0xFFFFFFFF;      // 看門狗收尾時傾印的 pc
const SIMEXIT_CSR: u12 = 0x7C0;       // 自訂 CSR：任何存取 = 結束模擬（任何特權模式皆可）

// mstatus 欄位：SIE[1] MIE[3] SPIE[5] MPIE[7] SPP[8] MPP[12:11] MPRV[17] SUM[18] MXR[19]
//               FS[14:13] TVM[20] TW[21] TSR[22]；UXL[33:32] = SXL[35:34] = 2（唯讀，64 位元）；
//               SD[63] = (FS == Dirty)，讀取時即時算出
const MSTATUS_W: u64 = 0x7E79AA;      // 可寫欄位
const MSTATUS_XL: u64 = 0xA00000000;  // UXL = SXL = 2
const SSTATUS_R: u64 = 0x80000003000C6122;   // sstatus 視窗：SIE SPIE SPP FS SUM MXR UXL SD
const SSTATUS_W: u64 = 0xC6122;
const MISA: u64 = 0x8000000000141129; // MXL = 2；I M A F D S U
const FS_DIRTY: u64 = 0x6000;

// PTE 旗標
const PTE_V: u64 = 0x01;
const PTE_R: u64 = 0x02;
const PTE_W: u64 = 0x04;
const PTE_X: u64 = 0x08;
const PTE_U: u64 = 0x10;
const PTE_G: u64 = 0x20;
const PTE_A: u64 = 0x40;
const PTE_D: u64 = 0x80;
const PTW_LD: u32 = 0x3083;           // 走訪讀取 token 借用的編碼：ld x1, 0(x0)（Execute/Mem 取 f3 = 3、rd ≠ 0）
const PTW_SD: u32 = 0x3023;           // A/D 回寫 token：sd（f3 = 3）

// ════════════════════════════════════════════════════════════════════════════
//  組譯器
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

const OP: u7 = 0x33;
const OP32: u7 = 0x3B;
const OPIMM: u7 = 0x13;
const OPIMM32: u7 = 0x1B;
const LOAD: u7 = 0x03;
const STORE: u7 = 0x23;
const BRANCH: u7 = 0x63;

fn add(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 0, rd, OP) }
fn sub(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0x20, rs2, rs1, 0, rd, OP) }
fn sll(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 1, rd, OP) }
fn slt(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 2, rd, OP) }
fn sltu(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 3, rd, OP) }
fn xor(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 4, rd, OP) }
fn srl(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 5, rd, OP) }
fn sra(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0x20, rs2, rs1, 5, rd, OP) }
fn or(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 6, rd, OP) }
fn and(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 7, rd, OP) }

fn addw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 0, rd, OP32) }
fn subw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0x20, rs2, rs1, 0, rd, OP32) }
fn sllw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 1, rd, OP32) }
fn srlw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0, rs2, rs1, 5, rd, OP32) }
fn sraw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(0x20, rs2, rs1, 5, rd, OP32) }

fn mul(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 0, rd, OP) }
fn mulh(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 1, rd, OP) }
fn mulhsu(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 2, rd, OP) }
fn mulhu(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 3, rd, OP) }
fn div(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 4, rd, OP) }
fn divu(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 5, rd, OP) }
fn rem(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 6, rd, OP) }
fn remu(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 7, rd, OP) }

fn mulw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 0, rd, OP32) }
fn divw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 4, rd, OP32) }
fn divuw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 5, rd, OP32) }
fn remw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 6, rd, OP32) }
fn remuw(rd: u5, rs1: u5, rs2: u5) -> u32 { enc_r(1, rs2, rs1, 7, rd, OP32) }

fn addi(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 0, rd, OPIMM) }
fn slti(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 2, rd, OPIMM) }
fn sltiu(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 3, rd, OPIMM) }
fn xori(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 4, rd, OPIMM) }
fn ori(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 6, rd, OPIMM) }
fn andi(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 7, rd, OPIMM) }
fn slli(rd: u5, rs1: u5, sh: u6) -> u32 { enc_i(sh, rs1, 1, rd, OPIMM) }
fn srli(rd: u5, rs1: u5, sh: u6) -> u32 { enc_i(sh, rs1, 5, rd, OPIMM) }
fn srai(rd: u5, rs1: u5, sh: u6) -> u32 { enc_i(0x400 + sh, rs1, 5, rd, OPIMM) }

fn addiw(rd: u5, rs1: u5, imm: i12) -> u32 { enc_i(imm, rs1, 0, rd, OPIMM32) }
fn slliw(rd: u5, rs1: u5, sh: u5) -> u32 { enc_i(sh, rs1, 1, rd, OPIMM32) }
fn srliw(rd: u5, rs1: u5, sh: u5) -> u32 { enc_i(sh, rs1, 5, rd, OPIMM32) }
fn sraiw(rd: u5, rs1: u5, sh: u5) -> u32 { enc_i(0x400 + sh, rs1, 5, rd, OPIMM32) }

fn lb(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 0, rd, LOAD) }
fn lh(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 1, rd, LOAD) }
fn lw(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 2, rd, LOAD) }
fn ld(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 3, rd, LOAD) }
fn lbu(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 4, rd, LOAD) }
fn lhu(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 5, rd, LOAD) }
fn lwu(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 6, rd, LOAD) }

fn sb(rs2: u5, rs1: u5, off: i12) -> u32 { enc_s(off, rs2, rs1, 0, STORE) }
fn sh(rs2: u5, rs1: u5, off: i12) -> u32 { enc_s(off, rs2, rs1, 1, STORE) }
fn sw(rs2: u5, rs1: u5, off: i12) -> u32 { enc_s(off, rs2, rs1, 2, STORE) }
fn sd(rs2: u5, rs1: u5, off: i12) -> u32 { enc_s(off, rs2, rs1, 3, STORE) }

fn beq(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 0, BRANCH) }
fn bne(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 1, BRANCH) }
fn blt(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 4, BRANCH) }
fn bge(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 5, BRANCH) }
fn bltu(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 6, BRANCH) }
fn bgeu(rs1: u5, rs2: u5, off: i13) -> u32 { enc_b(off, rs2, rs1, 7, BRANCH) }

fn lui(rd: u5, imm: u20) -> u32 { enc_u(imm, rd, 0x37) }
fn auipc(rd: u5, imm: u20) -> u32 { enc_u(imm, rd, 0x17) }
fn jal(rd: u5, off: i21) -> u32 { enc_j(off, rd, 0x6F) }
fn jalr(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 0, rd, 0x67) }

fn ecall() -> u32 { 0x00000073 }
fn ebreak() -> u32 { 0x00100073 }
fn mret() -> u32 { 0x30200073 }
fn sret() -> u32 { 0x10200073 }
fn wfi() -> u32 { 0x10500073 }
fn sfence_vma(rs1: u5, rs2: u5) -> u32 { enc_r(0x09, rs2, rs1, 0, 0, 0x73) }

// F/D：fmt 0 = S、1 = D；rm 7 = 動態（用 frm）
const RM_DYN: u3 = 7;
fn flw(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 2, rd, 0x07) }
fn fld(rd: u5, rs1: u5, off: i12) -> u32 { enc_i(off, rs1, 3, rd, 0x07) }
fn fsw(rs2: u5, rs1: u5, off: i12) -> u32 { enc_s(off, rs2, rs1, 2, 0x27) }
fn fsd(rs2: u5, rs1: u5, off: i12) -> u32 { enc_s(off, rs2, rs1, 3, 0x27) }
fn fpop(f5: u5, fmt: u2, rd: u5, rs1: u5, rs2: u5, rm: u3) -> u32 { enc_r((f5 << 2) | fmt, rs2, rs1, rm, rd, 0x53) }
fn fma4(op: u7, fmt: u2, rd: u5, rs1: u5, rs2: u5, rs3: u5, rm: u3) -> u32 {
    (rs3 << 27) | (fmt << 25) | (rs2 << 20) | (rs1 << 15) | (rm << 12) | (rd << 7) | op
}
fn fadd_s(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x00, 0, rd, rs1, rs2, RM_DYN) }
fn fadd_d(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x00, 1, rd, rs1, rs2, RM_DYN) }
fn fsub_d(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x01, 1, rd, rs1, rs2, RM_DYN) }
fn fmul_d(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x02, 1, rd, rs1, rs2, RM_DYN) }
fn fdiv_d(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x03, 1, rd, rs1, rs2, RM_DYN) }
fn fdiv_s(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x03, 0, rd, rs1, rs2, RM_DYN) }
fn fsqrt_d(rd: u5, rs1: u5) -> u32 { fpop(0x0B, 1, rd, rs1, 0, RM_DYN) }
fn fmadd_d(rd: u5, rs1: u5, rs2: u5, rs3: u5) -> u32 { fma4(0x43, 1, rd, rs1, rs2, rs3, RM_DYN) }
fn fsgnj_s(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x04, 0, rd, rs1, rs2, 0) }
fn fmin_d(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x05, 1, rd, rs1, rs2, 0) }
fn flt_d(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x14, 1, rd, rs1, rs2, 1) }
fn feq_s(rd: u5, rs1: u5, rs2: u5) -> u32 { fpop(0x14, 0, rd, rs1, rs2, 2) }
fn fclass_d(rd: u5, rs1: u5) -> u32 { fpop(0x1C, 1, rd, rs1, 0, 1) }
fn fmv_x_w(rd: u5, rs1: u5) -> u32 { fpop(0x1C, 0, rd, rs1, 0, 0) }
fn fmv_x_d(rd: u5, rs1: u5) -> u32 { fpop(0x1C, 1, rd, rs1, 0, 0) }
fn fmv_w_x(rd: u5, rs1: u5) -> u32 { fpop(0x1E, 0, rd, rs1, 0, 0) }
fn fmv_d_x(rd: u5, rs1: u5) -> u32 { fpop(0x1E, 1, rd, rs1, 0, 0) }
fn fcvt_d_l(rd: u5, rs1: u5) -> u32 { fpop(0x1A, 1, rd, rs1, 2, RM_DYN) }
fn fcvt_w_d(rd: u5, rs1: u5, rm: u3) -> u32 { fpop(0x18, 1, rd, rs1, 0, rm) }
fn fcvt_s_d(rd: u5, rs1: u5) -> u32 { fpop(0x08, 0, rd, rs1, 1, RM_DYN) }
fn fcvt_d_s(rd: u5, rs1: u5) -> u32 { fpop(0x08, 1, rd, rs1, 0, RM_DYN) }

// A 擴充：funct5 | aq | rl；f3 = 2（.W）或 3（.D）。aq/rl 在單 hart 行為模擬中沒有作用。
const AMO: u7 = 0x2F;
fn enc_amo(f5: u5, rs2: u5, rs1: u5, f3: u3, rd: u5) -> u32 { enc_r(f5 << 2, rs2, rs1, f3, rd, AMO) }
fn lr_w(rd: u5, rs1: u5) -> u32 { enc_amo(0x02, 0, rs1, 2, rd) }
fn sc_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x03, rs2, rs1, 2, rd) }
fn amoswap_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x01, rs2, rs1, 2, rd) }
fn amoadd_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x00, rs2, rs1, 2, rd) }
fn amoxor_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x04, rs2, rs1, 2, rd) }
fn amoand_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x0C, rs2, rs1, 2, rd) }
fn amoor_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x08, rs2, rs1, 2, rd) }
fn amomin_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x10, rs2, rs1, 2, rd) }
fn amomax_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x14, rs2, rs1, 2, rd) }
fn amominu_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x18, rs2, rs1, 2, rd) }
fn amomaxu_w(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x1C, rs2, rs1, 2, rd) }
fn lr_d(rd: u5, rs1: u5) -> u32 { enc_amo(0x02, 0, rs1, 3, rd) }
fn sc_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x03, rs2, rs1, 3, rd) }
fn amoswap_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x01, rs2, rs1, 3, rd) }
fn amoadd_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x00, rs2, rs1, 3, rd) }
fn amoxor_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x04, rs2, rs1, 3, rd) }
fn amoand_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x0C, rs2, rs1, 3, rd) }
fn amoor_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x08, rs2, rs1, 3, rd) }
fn amomin_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x10, rs2, rs1, 3, rd) }
fn amomax_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x14, rs2, rs1, 3, rd) }
fn amominu_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x18, rs2, rs1, 3, rd) }
fn amomaxu_d(rd: u5, rs2: u5, rs1: u5) -> u32 { enc_amo(0x1C, rs2, rs1, 3, rd) }
fn fence() -> u32 { 0x0FF0000F }
fn nop() -> u32 { addi(0, 0, 0) }

// Zicsr：CSR 位址是 12 位元無號欄，不能用 enc_i（0xF14 這類超出 i12 的值會被位寬檢查擋下）
fn enc_csr(csr: u12, rs1: u5, f3: u3, rd: u5) -> u32 {
    (csr << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | 0x73
}
fn csrrw(rd: u5, csr: u12, rs1: u5) -> u32 { enc_csr(csr, rs1, 1, rd) }
fn csrrs(rd: u5, csr: u12, rs1: u5) -> u32 { enc_csr(csr, rs1, 2, rd) }
fn csrrc(rd: u5, csr: u12, rs1: u5) -> u32 { enc_csr(csr, rs1, 3, rd) }
fn csrrwi(rd: u5, csr: u12, zimm: u5) -> u32 { enc_csr(csr, zimm, 5, rd) }
fn csrrsi(rd: u5, csr: u12, zimm: u5) -> u32 { enc_csr(csr, zimm, 6, rd) }
fn csrrci(rd: u5, csr: u12, zimm: u5) -> u32 { enc_csr(csr, zimm, 7, rd) }
fn csrr(rd: u5, csr: u12) -> u32 { csrrs(rd, csr, 0) }        // csrr = csrrs with x0

/// 結束模擬：存取自訂 CSR 0x7C0（慣例：測試程式的出口；rv32i 時代的 ecall 現在是真陷阱）
fn simexit() -> u32 { csrrw(0, SIMEXIT_CSR, 0) }

/// 載入 32 位元常數（lui + addi 補償符號擴展）：結果 = sext32(imm)
fn li(rd: u5, imm: u32) -> [u32; 2] {
    let hi = ((imm + 0x800) >> 12) & 0xFFFFF;
    let lo = (imm & 0xFFF).wrap::<i12>();
    [lui(rd, hi), addi(rd, rd, lo)]
}

/// 載入任意 64 位元常數：高 32 位（sext32 後左移 32）+ 低 32 位（零擴展）相加。
/// 注意 rd 不可為 31（內部借用 rd+1 作暫存）。
fn li64(rd: u5, hi: u32, lo: u32) -> [u32; 8] {
    let t = rd + 1;
    concat(li(rd, hi), concat([
        slli(rd, rd, 32)],
        concat(li(t, lo), concat([
        slli(t, t, 32),
        srli(t, t, 32),
        add(rd, rd, t)], []))))
}

// ════════════════════════════════════════════════════════════════════════════
//  解碼
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

/// 指令分類，同時是合法性檢查。注意 RV64 的 SLLI/SRLI/SRAI 移位量為 6 位元，
/// 合法性要檢查 inst[31:26]（imm[11:6]），不是 7 位元 funct7。
comb fn classify(inst: u32) -> u5 {
    let op = inst[6:0];
    let f3 = inst[14:12];
    let f7 = inst[31:25];
    let f6 = inst[31:26];               // RV64 立即移位的 6 位元操作碼欄
    match op {
        0x37 => C_LUI,
        0x17 => C_AUIPC,
        0x6F => C_JAL,
        0x67 => if f3 == 0 { C_JALR } else { C_ILLEGAL },
        0x63 => if f3 == 2 || f3 == 3 { C_ILLEGAL } else { C_BRANCH },
        0x03 => if f3 == 7 { C_ILLEGAL } else { C_LOAD },        // LB..LWU（0..6）
        0x23 => if f3 > 3 { C_ILLEGAL } else { C_STORE },        // SB..SD（0..3）
        0x13 => if f3 == 1 {
                    if f6 == 0 { C_OPIMM } else { C_ILLEGAL }    // SLLI
                } else if f3 == 5 {
                    if f6 == 0 || f6 == 0x10 { C_OPIMM } else { C_ILLEGAL }   // SRLI/SRAI
                } else { C_OPIMM },
        0x1B => if f3 == 0 { C_OPIMM32 }                         // ADDIW
                else if f3 == 1 { if f7 == 0 { C_OPIMM32 } else { C_ILLEGAL } }
                else if f3 == 5 { if f7 == 0 || f7 == 0x20 { C_OPIMM32 } else { C_ILLEGAL } }
                else { C_ILLEGAL },
        0x33 => if f7 == 0 { C_OP }
                else if f7 == 0x01 { C_M }
                else if f7 == 0x20 && (f3 == 0 || f3 == 5) { C_OP }
                else { C_ILLEGAL },
        0x3B => if f7 == 0x01 {
                    if f3 == 0 || f3 > 3 { C_M32 } else { C_ILLEGAL }   // MULW/DIVW/DIVUW/REMW/REMUW
                } else if f3 == 0 {
                    if f7 == 0 || f7 == 0x20 { C_OP32 } else { C_ILLEGAL }   // ADDW/SUBW
                } else if f3 == 1 {
                    if f7 == 0 { C_OP32 } else { C_ILLEGAL }            // SLLW
                } else if f3 == 5 {
                    if f7 == 0 || f7 == 0x20 { C_OP32 } else { C_ILLEGAL }   // SRLW/SRAW
                } else { C_ILLEGAL },
        0x0F => C_FENCE,
        0x2F => if (f3 == 2 || f3 == 3) && amo_f5_ok(inst[31:27], inst[24:20]) { C_AMO } else { C_ILLEGAL },
        0x07 => if f3 == 2 || f3 == 3 { C_FLOAD } else { C_ILLEGAL },
        0x27 => if f3 == 2 || f3 == 3 { C_FSTORE } else { C_ILLEGAL },
        0x53 => if fp_decode(inst).0 { C_FP } else { C_ILLEGAL },
        0x43 => if inst[26:25] <= 1 { C_FMA } else { C_ILLEGAL },
        0x47 => if inst[26:25] <= 1 { C_FMA } else { C_ILLEGAL },
        0x4B => if inst[26:25] <= 1 { C_FMA } else { C_ILLEGAL },
        0x4F => if inst[26:25] <= 1 { C_FMA } else { C_ILLEGAL },
        0x73 => if f3 == 0 {
                    if inst == 0x00000073 { C_ECALL }
                    else if inst == 0x00100073 { C_EBREAK }
                    else if inst == 0x30200073 { C_MRET }
                    else if inst == 0x10200073 { C_SRET }
                    else if inst == 0x10500073 { C_FENCE }      // WFI：無中斷，立即完成（= nop）
                    else if f7 == 0x09 && inst[11:7] == 0 { C_SFENCE }
                    else { C_ILLEGAL }
                } else if f3 == 1 || f3 == 2 || f3 == 3 || f3 == 5 || f3 == 6 || f3 == 7 {
                    C_ZICSR
                } else { C_ILLEGAL },
        _ => C_ILLEGAL,
    }
}

/// A 擴充的合法 funct5（LR 的 rs2 欄必須為 0）
comb fn amo_f5_ok(f5: u5, rs2: u5) -> bool {
    if f5 == 0x02 { rs2 == 0 }
    else { f5 == 0x03 || f5 == 0x01 || f5 == 0x00 || f5 == 0x04 || f5 == 0x0C || f5 == 0x08
           || f5 == 0x10 || f5 == 0x14 || f5 == 0x18 || f5 == 0x1C }
}

// ZICSR 不列入 uses_rs1：暫存器型的相依等待在 Decode 的 ZICSR 分支精確處理
// （立即型的 rs1 欄是 zimm，不是暫存器，不能等待）
comb fn uses_rs1(c: u5) -> bool {
    c == C_JALR || c == C_BRANCH || c == C_LOAD || c == C_STORE
        || c == C_OPIMM || c == C_OP || c == C_OPIMM32 || c == C_OP32 || c == C_M || c == C_M32
        || c == C_SFENCE || c == C_AMO
}
comb fn uses_rs2(c: u5) -> bool {
    c == C_BRANCH || c == C_STORE || c == C_OP || c == C_OP32 || c == C_M || c == C_M32
        || c == C_SFENCE || c == C_AMO
}
comb fn writes_rd(c: u5) -> bool {
    c == C_LUI || c == C_AUIPC || c == C_JAL || c == C_JALR || c == C_LOAD
        || c == C_OPIMM || c == C_OP || c == C_OPIMM32 || c == C_OP32 || c == C_M || c == C_M32
        || c == C_AMO
}

// ════════════════════════════════════════════════════════════════════════════
//  ALU（64 位元）與 M 擴充
// ════════════════════════════════════════════════════════════════════════════

/// 64 位元左移。精確整數上限 120 位元，2^64 << 63 會爆，
/// 所以分兩次移位、中間 wrap 回 u64。
comb fn sll64(a: u64, sh: u6) -> u64 {
    ((a << (sh >> 1)).wrap::<u64>() << (sh - (sh >> 1))).wrap::<u64>()
}

/// 64×64 → 128 位元乘積的 (hi, lo)。
/// 積最大近 2^128，超過精確整數上限，以 32 位元學校乘法分塊：
///   a·b = al·bl + (ah·bl + al·bh)·2^32 + ah·bh·2^64
comb fn mul_hi_lo(a: u64, b: u64) -> (u64, u64) {
    let ah = a[63:32];
    let al = a[31:0];
    let bh = b[63:32];
    let bl = b[31:0];
    let p0 = al * bl;                        // < 2^64
    let p1 = ah * bl;                        // < 2^64
    let p2 = al * bh;                        // < 2^64
    let p3 = ah * bh;                        // < 2^64
    let sum_mid = p1 + p2;                   // < 2^65（精確值）
    let mid = sum_mid.wrap::<u64>();
    let s_lo = p0 + ((mid & 0xFFFFFFFF) << 32);   // < 2^65（精確值）
    let lo = s_lo.wrap::<u64>();
    let hi = (sum_mid >> 32) + p3 + (s_lo >> 64); // < 2^64（可證）
    (hi, lo)
}

/// M 擴充（64 位元）。除以零與 overflow 的特殊值依 spec 手動處理
/// （模擬器的 `/` `%` 對零除會直接報錯，且語意恰好是 trunc toward zero，與 RISC-V 一致）。
/// MULH/MULHSU 由無號積修正：a_s·b_s = a_u·b_u − a_u·2^64·sb − b_u·2^64·sa（mod 2^128）。
comb fn alu_m(f3: u3, a: u64, b: u64) -> u64 {
    let (hi, lo) = mul_hi_lo(a, b);
    let sa = a[63:63];
    let sb = b[63:63];
    match f3 {
        0 => lo,                                             // MUL
        1 => (hi - (if sb == 1 { a } else { 0 })
                 - (if sa == 1 { b } else { 0 })).wrap::<u64>(), // MULH（有號×有號）
        2 => (hi - (if sa == 1 { b } else { 0 })).wrap::<u64>(), // MULHSU（有號×無號）
        3 => hi,                                             // MULHU
        4 => if b == 0 { 0xFFFFFFFFFFFFFFFF }
             else { (a.wrap::<i64>() / b.wrap::<i64>()).wrap::<u64>() },   // DIV
        5 => if b == 0 { 0xFFFFFFFFFFFFFFFF } else { a / b },             // DIVU
        6 => if b == 0 { a } else { (a.wrap::<i64>() % b.wrap::<i64>()).wrap::<u64>() }, // REM
        _ => if b == 0 { a } else { a % b },                              // REMU
    }
}

/// M 擴充 W 型：對 32 位元值運算，結果符號擴展到 64 位元。
/// 32 位元乘法積 < 2^64，不會碰到 120 位元上限，可直接算。
comb fn alu_m_w(f3: u3, a: u64, b: u64) -> u64 {
    let a32 = a.wrap::<i32>();
    let b32 = b.wrap::<i32>();
    let au32 = a.wrap::<u32>();
    let bu32 = b.wrap::<u32>();
    match f3 {
        0 => (au32 * bu32).wrap::<i32>().wrap::<u64>(),       // MULW
        4 => if b32 == 0 { 0xFFFFFFFFFFFFFFFF }
             else { (a32 / b32).wrap::<i32>().wrap::<u64>() },             // DIVW
        5 => if bu32 == 0 { 0xFFFFFFFFFFFFFFFF }
             else { (au32 / bu32).wrap::<i32>().wrap::<u64>() },           // DIVUW
        6 => if b32 == 0 { a32.wrap::<u64>() }
             else { (a32 % b32).wrap::<i32>().wrap::<u64>() },             // REMW
        _ => if bu32 == 0 { a32.wrap::<u64>() }
             else { (au32 % bu32).wrap::<i32>().wrap::<u64>() },           // REMUW
    }
}

/// 整數 ALU（64 位元）：f3 選運算，alt 區分 ADD/SUB 與 SRL/SRA
comb fn alu(f3: u3, alt: bit, a: u64, b: u64) -> u64 {
    let sh = b[5:0];
    match f3 {
        0 => if alt == 1 { (a - b).wrap::<u64>() } else { (a + b).wrap::<u64>() },
        1 => sll64(a, sh),
        2 => (a.wrap::<i64>() < b.wrap::<i64>()) as u64,
        3 => (a < b) as u64,
        4 => a ^ b,
        5 => if alt == 1 { (a.wrap::<i64>() >> sh).wrap::<u64>() } else { a >> sh },
        6 => a | b,
        _ => a & b,
    }
}

/// W 型整數 ALU：先在 32 位元內運算，結果符號擴展
comb fn alu_w(f3: u3, alt: bit, a: u64, b: u64) -> u64 {
    let sh = b[4:0];
    let a32 = a.wrap::<i32>();
    match f3 {
        0 => if alt == 1 { (a32 - b.wrap::<i32>()).wrap::<i32>().wrap::<u64>() }
             else { (a32 + b.wrap::<i32>()).wrap::<i32>().wrap::<u64>() },
        1 => (a.wrap::<u32>() << sh).wrap::<i32>().wrap::<u64>(),       // SLLW
        5 => if alt == 1 { (a32 >> sh).wrap::<i32>().wrap::<u64>() }    // SRAW
             else { (a.wrap::<u32>() >> sh).wrap::<i32>().wrap::<u64>() },   // SRLW（結果仍要 sext32）
        _ => 0,
    }
}

/// W 型立即 ALU（ADDIW/SLLIW/SRLIW/SRAIW）；imm 已符號擴展成 64 位元
comb fn alu_w_imm(f3: u3, alt: bit, a: u64, imm: u64) -> u64 {
    let sh = imm[4:0];
    let a32 = a.wrap::<i32>();
    match f3 {
        0 => (a32 + imm.wrap::<i32>()).wrap::<i32>().wrap::<u64>(),     // ADDIW
        1 => (a.wrap::<u32>() << sh).wrap::<i32>().wrap::<u64>(),       // SLLIW
        5 => if alt == 1 { (a32 >> sh).wrap::<i32>().wrap::<u64>() }    // SRAIW
             else { (a.wrap::<u32>() >> sh).wrap::<i32>().wrap::<u64>() },   // SRLIW（結果仍要 sext32）
        _ => 0,
    }
}

/// AMO 的運算（old = 記憶體舊值、src = rs2；.W 時 old 已符號擴展，只比較/寫回低 32 位）
comb fn amo_alu(f5: u5, w: bool, old: u64, src: u64) -> u64 {
    let a = if w { old.wrap::<i32>().wrap::<i64>() } else { old.wrap::<i64>() };
    let b = if w { src.wrap::<i32>().wrap::<i64>() } else { src.wrap::<i64>() };
    let au = if w { old.wrap::<u32>().wrap::<u64>() } else { old };
    let bu = if w { src.wrap::<u32>().wrap::<u64>() } else { src };
    match f5 {
        0x01 => src,                                     // AMOSWAP
        0x00 => (old + src).wrap::<u64>(),               // AMOADD
        0x04 => old ^ src,                               // AMOXOR
        0x0C => old & src,                               // AMOAND
        0x08 => old | src,                               // AMOOR
        0x10 => if a < b { old } else { src },           // AMOMIN
        0x14 => if a > b { old } else { src },           // AMOMAX
        0x18 => if au < bu { old } else { src },         // AMOMINU
        _ => if au > bu { old } else { src },            // AMOMAXU
    }
}

comb fn branch_taken(f3: u3, a: u64, b: u64) -> bool {
    match f3 {
        0 => a == b,
        1 => a != b,
        4 => a.wrap::<i64>() < b.wrap::<i64>(),
        5 => a.wrap::<i64>() >= b.wrap::<i64>(),
        6 => a < b,
        _ => a >= b,
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  CSR、特權模式與 Sv39
// ════════════════════════════════════════════════════════════════════════════

/// 存在的可讀寫 CSR（0x7C0 為 simexit 自訂 CSR）
comb fn csr_rw(a: u12) -> bool {
    a == 0x001 || a == 0x002 || a == 0x003                                   // fflags frm fcsr
        || a == 0x100 || a == 0x104 || a == 0x105 || a == 0x106              // sstatus sie stvec scounteren
        || a == 0x140 || a == 0x141 || a == 0x142 || a == 0x143 || a == 0x144 // sscratch sepc scause stval sip
        || a == 0x180                                                         // satp
        || a == 0x300 || a == 0x301 || a == 0x302 || a == 0x303 || a == 0x304 || a == 0x305
        || a == 0x306 || a == 0x340 || a == 0x341 || a == 0x342 || a == 0x343 || a == 0x344
        || a == 0xB00 || a == 0xB02                                           // mcycle minstret
        || a == SIMEXIT_CSR || a == MTIMECMP_CSR
}
/// 唯讀 CSR（寫入 → 非法指令；本實作全部讀為 0）
comb fn csr_ro(a: u12) -> bool {
    a == 0xF14 || a == 0xF13 || a == 0xF12 || a == 0xF11
        || a == 0xC00 || a == 0xC01 || a == 0xC02                             // cycle time instret
}
/// 各 CSR 寫入時的合法欄位遮罩（WARL/WPRI）。sstatus/sie/sip/satp 另有專門處理。
comb fn csr_wmask(a: u12) -> u64 {
    if a == 0x300 { MSTATUS_W }
    else if a == 0x301 { 0 }                              // misa：唯讀值（寫入忽略）
    else if a == 0x302 { 0xB3FF }                         // medeleg：bit 11（M 模式 ecall）恆 0
    else if a == 0x303 { 0x222 }                          // mideleg：S 層中斷位元
    else if a == 0x304 { 0xAAA }                          // mie
    else if a == 0x344 { 0x222 }                          // mip：SSIP/STIP/SEIP 可寫；MTIP 由 mtimecmp 算出
    else if a == 0x341 || a == 0x141 { 0xFFFFFFFFFFFFFFFC }   // mepc/sepc：IALIGN=32 → [1:0] = 0
    else { 0xFFFFFFFFFFFFFFFF }
}
comb fn clr(v: u64, m: u64) -> u64 { v & ~m }

/// 位址未對齊：f3[1:0] = log2(存取寬度)（載入的 4/5/6 = LBU/LHU/LWU 同樣適用）
comb fn misaligned(f3: u3, a: u64) -> bool {
    (a & ((1 << f3[1:0]) - 1)) != 0
}

/// TLB 標籤：V[63] G[62] 層級[61:60] ASID[59:44] VPN[26:0]；層級 L 的項目忽略 VPN 低 9L 位
comb fn tlb_match(t: u64, vpn: u64) -> bool {
    let sh = 9 * t[61:60];
    (t[26:0] >> sh) == (vpn >> sh)
}

/// 葉 PTE 的權限檢查（acc：1 = 取指、2 = 載入、3 = 儲存；dpriv 為有效特權模式）
comb fn pte_denied(p: u64, acc: u2, dpriv: u2, sum: bit, mxr: bit) -> bool {
    let r = (p & PTE_R) != 0;
    let w = (p & PTE_W) != 0;
    let x = (p & PTE_X) != 0;
    let u = (p & PTE_U) != 0;
    let perm = if acc == 1 { x } else if acc == 2 { r || (mxr == 1 && x) } else { w };
    // U 模式只能碰 U 頁；S 模式碰 U 頁：取指一律不行，載存要 SUM = 1
    let mode_ok = if dpriv == 0 { u } else { !u || (acc != 1 && sum == 1) };
    !perm || !mode_ok
}

// ════════════════════════════════════════════════════════════════════════════
//  管線
// ════════════════════════════════════════════════════════════════════════════

/// Fetch 送出這類指令後停等 redirect（Decode 也只在這些「等待點」接受中斷）：
/// • 分支/跳躍（含 JAL：目標未對齊要在 Decode 陷阱）；
/// • 可能在 Decode 觸發例外的指令——例外與否要等 Decode 檢查完才知道，不停等的話 Fetch 順序
///   取入的下一條會在陷阱後洩漏進管線（甚至觸發第二個陷阱覆蓋 mepc/mtval）；浮點指令也算
///   （FS = Off、無效捨入模式）。沒有陷阱時 Execute 送回下一個 pc。
/// 所有迴圈都含分支或跳躍，所以中斷一定會在有限步內被接受。
comb fn fetch_waits(c: u5) -> bool {
    c == C_BRANCH || c == C_JAL || c == C_JALR || c == C_ECALL || c == C_EBREAK || c == C_MRET
        || c == C_SRET || c == C_SFENCE || c == C_ILLEGAL || c == C_ZICSR || c == C_LOAD
        || c == C_STORE || c == C_AMO || c == C_FLOAD || c == C_FSTORE || c == C_FP || c == C_FMA
}

/// 中斷優先序：MEI 11 > MSI 3 > MTI 7 > SEI 9 > SSI 1 > STI 5；回傳 (有, 編號)
comb fn pick_irq(v: u64) -> (bool, u64) {
    if v[11:11] == 1 { (true, 11) } else if v[3:3] == 1 { (true, 3) } else if v[7:7] == 1 { (true, 7) }
    else if v[9:9] == 1 { (true, 9) } else if v[1:1] == 1 { (true, 1) } else if v[5:5] == 1 { (true, 5) }
    else { (false, 0) }
}

/// 取指：持有指令記憶體（實體位址 0 起）、pc 與一筆 ITLB（pc 頁 → 實體頁）。
/// ITLB 未命中時送 F_XLATE 請 Decode 翻譯，經 redirect 收回應：
///   bit 0 = 1 → 實體頁位址 | 1；bit 0 = 0 → 取指頁錯誤，值是 trap 目標（必 4 對齊）。
/// 翻譯情境（特權模式、satp、頁表）只會被停等型指令改變，所以除了分支/JALR 之外，
/// 收到任何 redirect 都讓 ITLB 失效。
task Fetch(prog: [u32; IWORDS], to_dec: out chan<(u2, u64, u32)>, redirect: in chan<u64>) {
    carry pc: u64 = 0;
    carry itv: bool = false;
    carry ivp: u64 = 0;
    carry ipp: u64 = 0;
    serial for _ in 0..FETCH_LIMIT {
        let mut ok = itv && (pc >> 12) == ivp;
        if !ok {
            to_dec.send((F_XLATE, pc, 0));
            let r = redirect.recv();
            if (r & 1) == 1 {
                itv = true;
                ivp = pc >> 12;
                ipp = r >> 12;
                ok = true;
            } else {
                pc = clr(r, 3);                      // 取指頁錯誤（或中斷）→ trap 目標（下一輪再翻譯）
                itv = false;
            }
        }
        if ok {
            let word = ((ipp << 12) | (pc & 0xFFF)) >> 2;
            if (pc & 3) != 0 || word >= IWORDS {
                // 超出指令 ROM（或未對齊——跳躍目標在 Decode 已檢查，理論上不會發生）
                to_dec.send((F_IFAULT, pc, 0));
                pc = clr(redirect.recv(), 3);
                itv = false;
            } else {
                let inst = prog[word];
                to_dec.send((F_INST, pc, inst));
                let c = classify(inst);
                // simexit（存取自訂 CSR 0x7C0）：停止取指，由 Decode 收尾傾印
                if c == C_ZICSR && inst[31:20] == SIMEXIT_CSR {
                    return;
                }
                if fetch_waits(c) {
                    let r = redirect.recv();
                    if (r & R_CTX) != 0 { itv = false; }
                    pc = clr(r, 3);
                } else {
                    pc = (pc + 4).wrap::<u64>();
                }
            }
        }
    }
    to_dec.send((F_WDOG, 0, 0));             // 看門狗到期（Decode 以 cause 2 收尾）
}

/// 解碼 + 暫存器檔 + 記分板 + CSR 檔 + 特權模式 + MMU（TLBN 項統一 TLB + 頁表走訪）
task Decode<const TLBN: u32, const WBD: u32>(ifq: in chan<(u2, u64, u32)>,
            to_ex: out chan<(u64, u32, u5, u64, u64, u64)>,
            wbq: in chan<(u6, u64, u5)>,
            dump: out chan<u64>) {
    // 暫存器檔：0..31 = x0..x31、32..63 = f0..f31（記分板與寫回 token 共用同一個索引空間）
    let mut rf: [u64; 64] = [0; 64];
    let mut pend: [u4; 64] = [0; 64];
    let mut inflight: u5 = 0;
    let mut retired: u64 = 0;
    let mut stalls: u64 = 0;
    let mut csr: mem<u64, 4096> = [0; 4096];
    let mut priv: u2 = 3;                    // 0 = U, 1 = S, 3 = M
    let mut tlb_tag: mem<u64, TLBN> = [0; TLBN];
    let mut tlb_pte: mem<u64, TLBN> = [0; TLBN];
    let mut tlb_next: u32 = 0;               // 輪替取代
    csr[0x300] = MSTATUS_XL;
    csr[0x301] = MISA;
    csr[0x7C1] = 0xFFFFFFFFFFFFFFFF;         // mtimecmp：重置為永不觸發

    for (tag, pc, inst) in ifq {
        let c = if tag == F_XLATE { C_XLATE } else { classify(inst) };

        // 收尾：看門狗（cause 2）或 simexit（cause 0；模擬器掛鉤，任何特權模式皆可）
        if tag == F_WDOG || (tag == F_INST && c == C_ZICSR && inst[31:20] == SIMEXIT_CSR) {
            for _ in 0..MAX_INFLIGHT {
                if inflight != 0 {
                    let (r, v, fl) = wbq.recv();
                    rf[r] = if r == 0 { 0 } else { v };   // x0 恆 0（FP→整數寫 x0 只為帶回旗標）
                    csr[1] = csr[1] | fl;                // fflags 累積
                    pend[r] = pend[r] - 1;
                    inflight = inflight - 1;
                }
            }
            dump.send(if tag == F_WDOG { 2 } else { 0 });
            dump.send(if tag == F_WDOG { WDOG_PC } else { pc });
            dump.send(retired);
            dump.send(stalls);
            for i in 0..32 { dump.send(rf[i]); }
            dump.send(priv);
            for i in 32..64 { dump.send(rf[i]); }
            return;
        }

        let rd = inst[11:7];
        let rs1 = inst[19:15];
        let rs2 = inst[24:20];
        let fd = fp_decode(inst);
        let is_fp = c == C_FLOAD || c == C_FSTORE || c == C_FP || c == C_FMA;
        // 來源/目的暫存器的索引（f 暫存器 = 32 + 編號）
        let s1: u6 = if (c == C_FP && !fd.1) || c == C_FMA { 32 + rs1 } else { rs1 };
        let s2: u6 = if c == C_FP || c == C_FMA || c == C_FSTORE { 32 + rs2 } else { rs2 };
        let s3: u6 = 32 + inst[31:27];
        let dst: u6 = if c == C_FLOAD || c == C_FMA || (c == C_FP && !fd.3) { 32 + rd } else { rd };
        let u1 = uses_rs1(c) || is_fp;
        let u2 = uses_rs2(c) || (c == C_FP && fd.2) || c == C_FMA || c == C_FSTORE;
        let u3 = c == C_FMA;

        // 資料相依：等到來源暫存器沒有未完成的寫入
        for _ in 0..MAX_INFLIGHT {
            if (u1 && pend[s1] != 0) || (u2 && pend[s2] != 0) || (u3 && pend[s3] != 0) {
                let (r, v, fl) = wbq.recv();
                rf[r] = if r == 0 { 0 } else { v };   // x0 恆 0（FP→整數寫 x0 只為帶回旗標）
                csr[1] = csr[1] | fl;                // fflags 累積
                pend[r] = pend[r] - 1;
                inflight = inflight - 1;
                stalls += 1;
            }
        }
        // ZICSR 暫存器型的相依等待（立即型的 rs1 欄是 zimm，不是暫存器）
        if c == C_ZICSR && inst[14:14] == 0 {
            for _ in 0..MAX_INFLIGHT {
                if pend[rs1] != 0 {
                    let (r, v, fl) = wbq.recv();
                    rf[r] = if r == 0 { 0 } else { v };   // x0 恆 0（FP→整數寫 x0 只為帶回旗標）
                    csr[1] = csr[1] | fl;                // fflags 累積
                    pend[r] = pend[r] - 1;
                    inflight = inflight - 1;
                    stalls += 1;
                }
            }
        }

        let ms = csr[0x300];
        let fn3 = inst[14:12];
        let src = if inst[14:14] == 1 { inst[19:15].wrap::<u64>() } else { rf[rs1] };   // ZICSR 運算元
        let writes = fn3 == 1 || fn3 == 5
                  || ((fn3 == 2 || fn3 == 3 || fn3 == 6 || fn3 == 7) && src != 0);

        // ── 例外偵測（全部集中在 Decode）──
        // acc：需要翻譯的存取（0 = 無、1 = 取指、2 = 載入、3 = 儲存），va 為虛擬位址
        let mut exc: (bool, u64, u64) = (false, 0, 0);   // (觸發, cause, tval)
        let mut acc: u2 = 0;
        let mut va: u64 = 0;
        // ── 中斷：只在等待點接受（見 fetch_waits）。mtime = 已退休指令數，完全可重現。
        // M 層：未委派且（模式 < M 或 MIE）；S 層（mideleg）：模式 < S，或模式 = S 且 SIE。
        let waitpoint = tag == F_XLATE || tag == F_IFAULT || (tag == F_INST && fetch_waits(c));
        let mip = (csr[0x344] & 0x222) | (if retired >= csr[0x7C1] { 0x80 } else { 0 });
        let ready = mip & csr[0x304];
        let m_irq = clr(ready, csr[0x303]);
        let s_irq = ready & csr[0x303];
        let irq = pick_irq((if priv < 3 || ms[3:3] == 1 { m_irq } else { 0 })
                         | (if priv < 1 || (priv == 1 && ms[1:1] == 1) { s_irq } else { 0 }));
        if waitpoint && irq.0 {
            exc = (true, (1 << 63) | irq.1, 0);
        } else if tag == F_IFAULT {
            exc = (true, if (pc & 3) != 0 { 0 } else { 1 }, pc);   // 取指未對齊 / 存取錯誤
        } else if c == C_XLATE {
            acc = 1;
            va = pc;
        } else if c == C_JAL || c == C_JALR || (c == C_BRANCH && branch_taken(fn3, rf[rs1], rf[rs2])) {
            // 跳躍目標未對齊（IALIGN = 32）：例外報在跳躍指令上，tval = 目標
            let t = if c == C_JAL { (pc + imm_j(inst)).wrap::<u64>() }
                    else if c == C_JALR { clr((rf[rs1] + imm_i(inst)).wrap::<u64>(), 1) }
                    else { (pc + imm_b(inst)).wrap::<u64>() };
            if (t & 3) != 0 { exc = (true, 0, t); }
        } else if c == C_ILLEGAL {
            exc = (true, 2, inst.wrap::<u64>());
        } else if c == C_ECALL {
            exc = (true, 8 + priv, 0);               // 8 = U、9 = S、11 = M
        } else if c == C_EBREAK {
            exc = (true, 3, 0);
        } else if c == C_LOAD {
            va = (rf[rs1] + imm_i(inst)).wrap::<u64>();
            if misaligned(fn3, va) { exc = (true, 4, va); } else { acc = 2; }
        } else if c == C_STORE {
            va = (rf[rs1] + imm_s(inst)).wrap::<u64>();
            if misaligned(fn3, va) { exc = (true, 6, va); } else { acc = 3; }
        } else if is_fp && ms[14:13] == 0 {
            exc = (true, 2, inst.wrap::<u64>());     // mstatus.FS = Off：浮點指令非法
        } else if (c == C_FP && fd.4) || c == C_FMA {
            let rmv = if fn3 == 7 { csr[2] } else { fn3 };
            if rmv >= 5 { exc = (true, 2, inst.wrap::<u64>()); }   // 保留的捨入模式（含 frm 無效時的 DYN）
        } else if c == C_FLOAD {
            va = (rf[rs1] + imm_i(inst)).wrap::<u64>();
            if misaligned(fn3, va) { exc = (true, 4, va); } else { acc = 2; }
        } else if c == C_FSTORE {
            va = (rf[rs1] + imm_s(inst)).wrap::<u64>();
            if misaligned(fn3, va) { exc = (true, 6, va); } else { acc = 3; }
        } else if c == C_AMO {
            // LR 是載入、SC/AMO 是儲存（權限檢查要 W；W 隱含 R）；必須自然對齊，不能碰 MMIO
            va = rf[rs1];
            let lr = inst[31:27] == 0x02;
            if misaligned(fn3, va) { exc = (true, if lr { 4 } else { 6 }, va); }
            else { acc = if lr { 2 } else { 3 }; }
        } else if c == C_ZICSR {
            // 未定義 CSR、特權不足（csr[9:8] > 模式）、寫唯讀 CSR、TVM 下 S 模式碰 satp → 非法指令
            let a = inst[31:20];
            if (!csr_rw(a) && !csr_ro(a)) || a[9:8] > priv || (writes && csr_ro(a))
                    || (a == 0x180 && priv == 1 && ms[20:20] == 1)
                    || (a <= 3 && ms[14:13] == 0)            // fflags/frm/fcsr 在 FS = Off 時非法
                    // 使用者計數器：S/U 要 mcounteren 對應位元，U 還要 scounteren
                    || (a >= 0xC00 && a <= 0xC02 && priv < 3 && ((csr[0x306] >> (a - 0xC00)) & 1) == 0)
                    || (a >= 0xC00 && a <= 0xC02 && priv == 0 && ((csr[0x106] >> (a - 0xC00)) & 1) == 0) {
                exc = (true, 2, inst.wrap::<u64>());
            }
        } else if c == C_MRET {
            if priv != 3 { exc = (true, 2, inst.wrap::<u64>()); }
        } else if c == C_SRET {
            if priv == 0 || (priv == 1 && ms[22:22] == 1) { exc = (true, 2, inst.wrap::<u64>()); }   // TSR
        } else if c == C_SFENCE {
            if priv == 0 || (priv == 1 && ms[20:20] == 1) { exc = (true, 2, inst.wrap::<u64>()); }   // TVM
        }

        // ── Sv39 翻譯 ──
        // 載存的有效模式：M 模式且 MPRV = 1 時用 MPP（取指不受 MPRV 影響）
        let mut pa: u64 = va;
        if acc != 0 {
            let dpriv = if acc != 1 && priv == 3 && ms[17:17] == 1 { ms[12:11] } else { priv };
            let satp = csr[0x180];
            if dpriv != 3 && satp[63:60] == 8 {
                let pf: u64 = if acc == 1 { 12 } else if acc == 2 { 13 } else { 15 };
                let af: u64 = if acc == 1 { 1 } else if acc == 2 { 5 } else { 7 };
                let asid = satp[59:44];
                let vpn = va[38:12];
                let mut fault: u64 = 0;
                let mut p: u64 = 0;              // 葉 PTE
                let mut lvl: u2 = 0;             // 葉所在層級（0 = 4KB、1 = 2MB、2 = 1GB）
                let mut hit = false;
                if (va >> 38) != 0 && (va >> 38) != 0x3FFFFFF {
                    fault = pf;                  // 非正規位址：bits 63:39 必須等於 bit 38
                } else {
                    for k in 0..TLBN {
                        let t = tlb_tag[k];
                        if !hit && t[63:63] == 1 && (t[62:62] == 1 || t[59:44] == asid) && tlb_match(t, vpn) {
                            hit = true;
                            p = tlb_pte[k];
                            lvl = t[61:60];
                        }
                    }
                    // 命中但要寫而 D = 0：當作未命中，由走訪設 D（TLB 永遠不需要寫回 PTE）
                    if hit && acc == 3 && (p & PTE_D) == 0 { hit = false; }
                }
                if fault == 0 && !hit {
                    // 頁表走訪：先排空記分板，之後下一個 wb token 必是走訪讀取的結果。
                    // 走訪讀寫以 in-band token 經 Execute → Mem，自動排在先前所有 store 之後。
                    for _ in 0..MAX_INFLIGHT {
                        if inflight != 0 {
                            let (r, v, fl) = wbq.recv();
                            rf[r] = if r == 0 { 0 } else { v };   // x0 恆 0（FP→整數寫 x0 只為帶回旗標）
                            csr[1] = csr[1] | fl;                // fflags 累積
                            pend[r] = pend[r] - 1;
                            inflight = inflight - 1;
                        }
                    }
                    let mut base = satp[43:0] << 12;
                    let mut i: u2 = 2;
                    let mut done = false;
                    let mut g: u64 = 0;
                    let mut paddr: u64 = 0;
                    for _ in 0..3 {
                        if !done {
                            let pte_pa = base + ((vpn >> (9 * i)) & 0x1FF) * 8;
                            if (pte_pa >> 3) >= DWORDS {
                                fault = af;      // 頁表項不在實體記憶體內 → 存取錯誤
                                done = true;
                            } else {
                                to_ex.send((pc, PTW_LD, C_PTW_R, pte_pa, 0, 0));
                                let (_, e, _) = wbq.recv();
                                g = g | (e & PTE_G);
                                if (e & PTE_V) == 0 || ((e & PTE_R) == 0 && (e & PTE_W) != 0) || e[63:54] != 0 {
                                    fault = pf;  // 無效、W 無 R 的保留編碼、保留位元非 0
                                    done = true;
                                } else if (e & (PTE_R | PTE_X)) != 0 {
                                    p = e;       // 葉
                                    paddr = pte_pa;
                                    lvl = i;
                                    done = true;
                                } else if i == 0 {
                                    fault = pf;  // 最底層仍是指標
                                    done = true;
                                } else {
                                    base = e[53:10] << 12;
                                    i = i - 1;
                                }
                            }
                        }
                    }
                    if fault == 0 && lvl > 0 && (p[53:10] & ((1 << (9 * lvl)) - 1)) != 0 {
                        fault = pf;              // 巨頁的 PPN 未對齊
                    }
                    if fault == 0 && !pte_denied(p, acc, dpriv, ms[18:18], ms[19:19]) {
                        // 硬體設 A（儲存另設 D），只在存取被允許時才做；然後填入 TLB
                        let np = p | PTE_A | (if acc == 3 { PTE_D } else { 0 });
                        if np != p {
                            to_ex.send((pc, PTW_SD, C_PTW_W, paddr, np, 0));
                        }
                        p = np;
                        tlb_tag[tlb_next] = (1 << 63) | (if g != 0 { 1 << 62 } else { 0 })
                                            | (lvl.wrap::<u64>() << 60) | (asid << 44) | vpn;
                        tlb_pte[tlb_next] = p;
                        tlb_next = if tlb_next + 1 == TLBN { 0 } else { tlb_next + 1 };
                    }
                }
                if fault == 0 && pte_denied(p, acc, dpriv, ms[18:18], ms[19:19]) {
                    fault = pf;
                }
                if fault != 0 {
                    exc = (true, fault, va);
                } else {
                    let sh = 12 + 9 * lvl;
                    pa = ((p[53:10] << 12) >> sh << sh) | (va & ((1 << sh) - 1));
                }
            }
            // 實體位址範圍（取指的範圍由 Fetch 處理：越界取 0 → 非法指令）
            if !exc.0 {
                if acc == 2 && (pa >> 3) >= DWORDS {
                    exc = (true, 5, va);
                } else if acc == 3 && (pa != MMIO_OUT || c == C_AMO) && (pa >> 3) >= DWORDS {
                    exc = (true, 7, va);
                }
            }
        }

        if exc.0 {
            // 進入 trap：U/S 模式的例外（中斷）若 medeleg（mideleg）對應位元 = 1 → 委派到 S，否則到 M。
            // tvec 的向量模式只影響中斷（BASE + 4 × 編號）；同步例外一律跳 BASE。
            let cause = exc.1;
            let intr = (cause >> 63) == 1;
            let code = cause & 0x3F;
            let mut target: u64 = 0;
            if priv != 3 && (((if intr { csr[0x303] } else { csr[0x302] }) >> code) & 1) == 1 {
                csr[0x141] = pc;
                csr[0x142] = cause;
                csr[0x143] = exc.2;
                // SPIE = SIE、SIE = 0、SPP = 原模式
                csr[0x300] = clr(ms, 0x122) | ((ms & 2) << 4) | (priv.wrap::<u64>() << 8);
                priv = 1;
                let tv = csr[0x105];
                target = clr(tv, 3) + (if intr && (tv & 3) == 1 { 4 * code } else { 0 });
            } else {
                csr[0x341] = pc;
                csr[0x342] = cause;
                csr[0x343] = exc.2;
                // MPIE = MIE、MIE = 0、MPP = 原模式
                csr[0x300] = clr(ms, 0x1888) | ((ms & 8) << 4) | (priv.wrap::<u64>() << 11);
                priv = 3;
                let tv = csr[0x305];
                target = clr(tv, 3) + (if intr && (tv & 3) == 1 { 4 * code } else { 0 });
            }
            to_ex.send((pc, inst, C_TRAP, target | R_CTX, 0, 0));
        } else if c == C_XLATE {
            to_ex.send((pc, 0, C_XLATE, ((pa >> 12) << 12) | 1, 0, 0));
        } else if c == C_MRET {
            // MIE = MPIE、MPIE = 1、MPP = U；回到 MPP 模式（非 M 時清 MPRV）；pc = mepc
            let mpp = ms[12:11];
            csr[0x300] = clr(ms, 0x1888 | (if mpp != 3 { 0x20000 } else { 0 })) | ((ms >> 4) & 8) | 0x80;
            priv = mpp;
            retired += 1;
            to_ex.send((pc, inst, C_TRAP, csr[0x341] | R_CTX, 0, 0));
        } else if c == C_SRET {
            // SIE = SPIE、SPIE = 1、SPP = U、MPRV = 0；回到 SPP 模式；pc = sepc
            let spp = ms[8:8];
            csr[0x300] = clr(ms, 0x20122) | ((ms >> 4) & 2) | 0x20;
            priv = spp;
            retired += 1;
            to_ex.send((pc, inst, C_TRAP, csr[0x141] | R_CTX, 0, 0));
        } else if c == C_SFENCE {
            // rs1 ≠ x0：只清該虛擬位址（含全域項）；rs2 ≠ x0：只清該 ASID 的非全域項
            let fvpn = rf[rs1][38:12];
            let fasid = rf[rs2][15:0];
            for k in 0..TLBN {
                let t = tlb_tag[k];
                if (rs1 == 0 || tlb_match(t, fvpn)) && (rs2 == 0 || (t[62:62] == 0 && t[59:44] == fasid)) {
                    tlb_tag[k] = 0;
                }
            }
            retired += 1;
            to_ex.send((pc, inst, C_TRAP, (pc + 4).wrap::<u64>() | R_CTX, 0, 0));
        } else if c == C_ZICSR {
            let a = inst[31:20];
            // fflags/fcsr：先排空，讓在途浮點運算的旗標都累積進來（程式順序）
            if a == 1 || a == 3 {
                for _ in 0..MAX_INFLIGHT {
                    if inflight != 0 {
                        let (r, v, fl) = wbq.recv();
                        rf[r] = if r == 0 { 0 } else { v };
                        csr[1] = csr[1] | fl;
                        pend[r] = pend[r] - 1;
                        inflight = inflight - 1;
                    }
                }
            }
            let sd = if ms[14:13] == 3 { 1 << 63 } else { 0 };
            // 計數器：cycle = instret = 已退休指令數（行為模擬沒有週期）+ 寫入造成的偏移；
            // time = mtime = 已退休指令數。視窗 CSR：sstatus = mstatus 的 S 欄位、
            // sie = mie & mideleg、sip = mip & mideleg；fcsr = frm << 5 | fflags
            let old = if a == 0xB00 || a == 0xC00 { (retired + csr[0xB00]).wrap::<u64>() }
                      else if a == 0xB02 || a == 0xC02 { (retired + csr[0xB02]).wrap::<u64>() }
                      else if a == 0xC01 { retired }
                      else if csr_ro(a) { 0 }
                      else if a == 0x300 { ms | sd }
                      else if a == 0x100 { (ms | sd) & SSTATUS_R }
                      else if a == 0x003 { (csr[2] << 5) | csr[1] }
                      else if a == 0x344 { mip }
                      else if a == 0x104 { csr[0x304] & csr[0x303] }
                      else if a == 0x144 { mip & csr[0x303] }
                      else { csr[a] };
            if writes {
                let newv = if fn3 == 1 || fn3 == 5 { src }
                           else if fn3 == 2 || fn3 == 6 { old | src }
                           else { old & ~src };
                if a == 0x100 {
                    csr[0x300] = clr(ms, SSTATUS_W) | (newv & SSTATUS_W);
                } else if a == 0x104 {
                    let m = csr[0x303];
                    csr[0x304] = clr(csr[0x304], m) | (newv & m);
                } else if a == 0x180 {
                    if newv[63:60] == 0 || newv[63:60] == 8 { csr[0x180] = newv; }   // 不支援的 MODE：整筆寫入無效
                } else if a <= 3 {
                    if a != 2 { csr[1] = newv & 0x1F; }
                    if a == 2 { csr[2] = newv & 7; }
                    if a == 3 { csr[2] = (newv >> 5) & 7; }
                    csr[0x300] = csr[0x300] | FS_DIRTY;
                } else if a == 0x144 {
                    let m = csr[0x303] & 2;          // sip：只有已委派的 SSIP 可寫
                    csr[0x344] = clr(csr[0x344], m) | (newv & m);
                } else if a == 0xB00 || a == 0xB02 {
                    // 寫入取代這條指令本身的遞增：下一條指令讀到的就是寫入值
                    csr[a] = (newv - retired - 1).wrap::<u64>();
                } else {
                    // mstatus.MPP 是 WARL：寫入保留值 2 時保持原值
                    let v = if a == 0x300 && newv[12:11] == 2 { clr(newv, 0x1800) | (old & 0x1800) } else { newv };
                    csr[a] = clr(old, csr_wmask(a)) | (v & csr_wmask(a));
                }
            }
            if rd != 0 {
                // 在途寫回數上限 = 寫回 FIFO 深度：滿了就先收最舊的一個，Mem 永遠不會卡在送寫回
                if inflight >= WBD {
                    let (r, v, fl) = wbq.recv();
                    rf[r] = if r == 0 { 0 } else { v };
                    csr[1] = csr[1] | fl;
                    pend[r] = pend[r] - 1;
                    inflight = inflight - 1;
                    stalls += 1;
                }
                pend[rd] = pend[rd] + 1;
                inflight = inflight + 1;
            }
            retired += 1;
            to_ex.send((pc, inst, C_CSR, old, 0, 0));    // 舊值經管線寫回 rd
        } else {
            // 浮點指令一律計數（目的可能是 x0：寫回 token 仍要帶回 fflags）
            if (writes_rd(c) && rd != 0) || (is_fp && c != C_FSTORE) {
                // 在途寫回數上限 = 寫回 FIFO 深度：滿了就先收最舊的一個，Mem 永遠不會卡在送寫回
                if inflight >= WBD {
                    let (r, v, fl) = wbq.recv();
                    rf[r] = if r == 0 { 0 } else { v };
                    csr[1] = csr[1] | fl;
                    pend[r] = pend[r] - 1;
                    inflight = inflight - 1;
                    stalls += 1;
                }
                pend[dst] = pend[dst] + 1;
                inflight = inflight + 1;
            }
            if is_fp {
                csr[0x300] = csr[0x300] | FS_DIRTY;   // 保守地標記 FS = Dirty
            }
            // 用 rm 的指令：把動態捨入模式（7）換成 frm，Execute 只看實際模式
            let inst2 = if ((c == C_FP && fd.4) || c == C_FMA) && fn3 == 7 {
                            (inst & 0xFFFF8FFF) | (csr[2].wrap::<u32>() << 12)
                        } else { inst };
            retired += 1;
            // 載存：a = 翻譯後的實體位址（Execute 不再自己加立即值）
            to_ex.send((pc, inst2, c, if acc != 0 { pa } else { rf[s1] }, rf[s2], rf[s3]));
        }
    }
}

/// 執行：ALU、分支判斷、位址計算。分支與 JALR 把下一個 pc 送回 Fetch。
task Execute(from_dec: in chan<(u64, u32, u5, u64, u64, u64)>,
             to_mem: out chan<(u3, u6, u64, u64, u64, u3, u5)>,
             redirect: out chan<u64>) {
    for (pc, inst, c, a, b, c3) in from_dec {
        let rd = inst[11:7];
        // 目的索引：f 暫存器 = 32 + rd（FLW/FLD、FMA、寫 f 的 OP-FP）
        let dst: u6 = if c == C_FLOAD || c == C_FMA || (c == C_FP && !fp_decode(inst).3) { 32 + rd } else { rd };
        let mut fl: u5 = 0;
        let f3 = inst[14:12];
        let alt = inst[30:30];
        let link = (pc + 4).wrap::<u64>();

        // kind：0 = 無寫回，1 = 寫回 val，2 = 載入（寫回 Mem 讀到的值），3 = 儲存，
        //       4 = LR/SC/AMO（val = funct5；rd = x0 時 Mem 照做記憶體操作但不寫回）
        // redir = (要送 redirect, 目標)。所有停等型指令都必須剛好送一個 redirect：
        //   分支/JALR = 目標或 pc+4；trap = 目標；
        //   載入/儲存/ZICSR（C_CSR）= pc+4（走到這裡代表 Decode 檢查後沒有觸發例外）
        let mut redir: (bool, u64) = (false, 0);
        let (kind, val, addr, sdata) = match c {
            C_LUI => (1, imm_u(inst).wrap::<i32>().wrap::<u64>(), 0, 0),
            C_AUIPC => (1, (pc + imm_u(inst).wrap::<i32>()).wrap::<u64>(), 0, 0),
            C_JAL => {
                redir = (true, (pc + imm_j(inst)).wrap::<u64>());
                (1, link, 0, 0)
            }
            C_JALR => {
                redir = (true, ((a + imm_i(inst)).wrap::<u64>()) & 0xFFFFFFFFFFFFFFFE);
                (1, link, 0, 0)
            }
            C_BRANCH => {
                let taken = branch_taken(f3, a, b);
                redir = (true, if taken { (pc + imm_b(inst)).wrap::<u64>() } else { link });
                (0, 0, 0, 0)
            }
            C_LOAD => {
                redir = (true, link);
                (2, 0, a, 0)                 // a = Decode 翻譯好的實體位址
            }
            C_STORE => {
                redir = (true, link);
                (3, 0, a, b)
            }
            C_AMO => {
                redir = (true, link);
                (4, inst[31:27].wrap::<u64>(), a, b)   // val = funct5
            }
            C_FLOAD => {
                redir = (true, link);
                (2, 0, a, 0)
            }
            C_FSTORE => {
                redir = (true, link);
                (3, 0, a, b)
            }
            C_FP => {
                redir = (true, link);
                let r = fp_op(inst, a, b);
                fl = r.1;
                (1, r.0, 0, 0)
            }
            C_FMA => {
                redir = (true, link);
                let r = fma_op(inst, a, b, c3);
                fl = r.1;
                (1, r.0, 0, 0)
            }
            C_PTW_R => (2, 0, a, 0),         // 走訪讀取（inst = PTW_LD：rd = 1、f3 = 3）；結果經 wb 回 Decode
            C_PTW_W => (3, 0, a, b),         // A/D 回寫（inst = PTW_SD）
            C_XLATE => {
                redir = (true, a);           // a = 實體頁 | 1（取指翻譯回應）
                (0, 0, 0, 0)
            }
            C_OPIMM => (1, alu(f3, if f3 == 5 { alt } else { 0 }, a, imm_i(inst).wrap::<u64>()), 0, 0),
            C_OP => (1, alu(f3, alt, a, b), 0, 0),
            C_OPIMM32 => (1, alu_w_imm(f3, alt, a, imm_i(inst).wrap::<u64>()), 0, 0),
            C_OP32 => (1, alu_w(f3, alt, a, b), 0, 0),
            C_M => (1, alu_m(f3, a, b), 0, 0),
            C_M32 => (1, alu_m_w(f3, a, b), 0, 0),
            C_TRAP => {
                redir = (true, a);           // a = trap 目標 pc（mtvec 向量或 mepc）
                (0, 0, 0, 0)
            }
            C_CSR => {
                redir = (true, link | R_CTX);  // CSR 可能改了 satp 等翻譯情境
                (1, a, 0, 0)                 // a = CSR 舊值 → 寫回 rd
            }
            _ => (0, 0, 0, 0),
        };
        if redir.0 {
            redirect.send(redir.1);
        }

        // 目的暫存器是 x0 的整數指令不寫回（Decode 的記分板也不會為它計數）；
        // 浮點指令例外：寫 x0 的 feq 等仍要把 fflags 帶回 Decode
        let k = if (kind == 1 || kind == 2) && dst == 0 && c != C_FP { 0 } else { kind };
        // FLW 以 f3 = 7 告訴 Mem 做 NaN-boxing
        let mf3 = if c == C_FLOAD && f3 == 2 { 7 } else { f3 };
        to_mem.send((k, dst, val, addr, sdata, mf3, fl));
    }
}

/// 記憶體階段：持有資料記憶體（小端、位元組定址，每字 8 位元組）。
///   對 MMIO_OUT 的 SW 不寫記憶體，而是送一個 token（低 32 位）到 io 通道。
task Mem(from_ex: in chan<(u3, u6, u64, u64, u64, u3, u5)>,
         wb: out chan<(u6, u64, u5)>,
         io: out chan<u32>,
         data: [u64; DWORDS]) {
    let mut dmem: mem<u64, DWORDS> = data;
    let mut resv: u64 = 0;                   // LR/SC 保留位址
    let mut resv_v = false;

    for (kind, rd, val, addr, sdata, f3, fl) in from_ex {
        let word = addr >> 3;
        let sh = addr[2:0] * 8;

        if kind == 1 {
            wb.send((rd, val, fl));
        } else if kind == 2 {
            let bad = (f3 == 3 && addr[2:0] != 0)
                   || ((f3 == 2 || f3 == 6) && addr[1:0] != 0)
                   || ((f3 == 1 || f3 == 5) && addr[0:0] != 0);
            assert !bad, "misaligned load";
            assert word < DWORDS, "load address out of range";
            let w = dmem[word];
            let part = w >> sh;
            let v = match f3 {
                0 => part[7:0].wrap::<i8>().wrap::<u64>(),      // LB
                1 => part[15:0].wrap::<i16>().wrap::<u64>(),    // LH
                2 => part[31:0].wrap::<i32>().wrap::<u64>(),    // LW
                3 => w,                                         // LD
                4 => part[7:0],                                 // LBU
                5 => part[15:0],                                // LHU
                6 => part[31:0],                                // LWU
                _ => 0xFFFFFFFF00000000 | part[31:0],           // FLW（NaN-boxing）
            };
            wb.send((rd, v, 0));
        } else if kind == 3 {
            if addr == MMIO_OUT {
                io.send(sdata[31:0]);
            } else {
                let bad = (f3 == 3 && addr[2:0] != 0)
                       || (f3 == 2 && addr[1:0] != 0)
                       || (f3 == 1 && addr[0:0] != 0);
                assert !bad, "misaligned store";
                assert word < DWORDS, "store address out of range";
                let m = match f3 { 0 => 0xFF, 1 => 0xFFFF, 2 => 0xFFFFFFFF, _ => 0xFFFFFFFFFFFFFFFF };
                let w = dmem[word];
                dmem[word] = (w & ~(m << sh)) | ((sdata & m) << sh);
            }
        } else if kind == 4 {
            // LR/SC/AMO（Decode 已檢查對齊與範圍）。保留集 = 一個位址；
            // LR 設定、任何 SC（成功或失敗）清除。單 hart 下一般 store 不清除保留（規格允許）。
            assert word < DWORDS, "atomic address out of range";
            let m = if f3 == 2 { 0xFFFFFFFF } else { 0xFFFFFFFFFFFFFFFF };
            let w = dmem[word];
            let old = if f3 == 2 { (w >> sh)[31:0].wrap::<i32>().wrap::<u64>() } else { w };
            let f5 = val[4:0];
            let mut result: u64 = old;
            if f5 == 0x02 {
                resv = addr;
                resv_v = true;
            } else if f5 == 0x03 {
                let ok = resv_v && resv == addr;
                resv_v = false;
                if ok { dmem[word] = (w & ~(m << sh)) | ((sdata & m) << sh); }
                result = if ok { 0 } else { 1 };
            } else {
                let nv = amo_alu(f5, f3 == 2, old, sdata);
                dmem[word] = (w & ~(m << sh)) | ((nv & m) << sh);
            }
            if rd != 0 {
                wb.send((rd, result, 0));
            }
        }
    }
}

/// 頂層：把四個階段用 FIFO 接起來。
/// WBD = 寫回 FIFO 深度 = Decode 允許的在途寫回數上限（滿了 Decode 先收一個），
/// 所以任何 WBD ≥ 1 都不會死結，只影響停等次數（rv32i 沒有這個上限：寫回太多會死結）。
/// TLBN = TLB 項數：只影響走訪次數，正確 fence 的程式輸出與它無關（確定性測試的對象）。
task Core<const WBD: u32, const TLBN: u32>(prog: [u32; IWORDS], data: [u64; DWORDS],
                          io: out chan<u32>, dump: out chan<u64>) {
    assert WBD >= 1 && WBD <= 8, "WBD must be 1..8 (4-bit scoreboard counters)";
    let (f2d_tx, f2d_rx) = chan::<(u2, u64, u32)>(depth = 1);
    let (d2e_tx, d2e_rx) = chan::<(u64, u32, u5, u64, u64, u64)>(depth = 1);
    let (e2m_tx, e2m_rx) = chan::<(u3, u6, u64, u64, u64, u3, u5)>(depth = 1);
    let (wb_tx, wb_rx)   = chan::<(u6, u64, u5)>(depth = WBD);
    let (rd_tx, rd_rx)   = chan::<u64>(depth = 1);

    spawn Fetch(prog, f2d_tx, rd_rx);
    spawn Decode::<TLBN, WBD>(f2d_rx, d2e_tx, wb_rx, dump);
    spawn Execute(d2e_rx, e2m_tx, rd_tx);
    spawn Mem(e2m_rx, wb_tx, io, data);
}

// ════════════════════════════════════════════════════════════════════════════
//  測試用 harness：載入程式、跑到陷阱、回傳 (傾印, MMIO 輸出)
// ════════════════════════════════════════════════════════════════════════════
fn image(p: _) -> [u32; IWORDS] { concat(p, [0; IWORDS - len(p)]) }      // 不足的部分補 0（= 非法指令）
fn dimage(d: _) -> [u64; DWORDS] { concat(d, [0; DWORDS - len(d)]) }

/// 32 位元元素版資料映像：把 u32 陣列兩兩打包進 64 位元字
/// （小端：偶數索引在字的低 32 位），給「以 4 位元組為步距定址」的 32 位元視角程式用。
fn dimage32(d: _) -> [u64; DWORDS] {
    let mut out: [u64; DWORDS] = [0; DWORDS];
    for k in 0..len(d) {
        let w = out[k >> 1];
        let v = d[k].wrap::<u64>();
        out[k >> 1] = if (k & 1) == 0 { (w & 0xFFFFFFFF00000000) | v }
                      else { (w & 0xFFFFFFFF) | (v << 32) };
    }
    out
}

/// 收集傾印與 MMIO 輸出
fn collect(io_rx: _, dump_rx: _) {
    let mut regs: [u64; DUMP_LEN] = [0; DUMP_LEN];
    for i in 0..DUMP_LEN { regs[i] = dump_rx.recv(); }
    let mut out = [];
    for v in io_rx { out = concat(out, [v]); }       // Mem 結束時關閉 io
    (regs, out)
}

// io 深度：MMIO 寫入次數 ≤ 已發射指令數 ≤ FETCH_LIMIT，
// 且測試端會先把 dump 收完才收 io，所以 io 必須裝得下全部輸出，否則管線未到陷阱就卡死。
fn run_img(p: _, img: [u64; DWORDS]) {
    let (io_tx, io_rx) = chan::<u32>(depth = FETCH_LIMIT);
    let (dump_tx, dump_rx) = chan::<u64>(depth = DUMP_LEN);
    spawn Core::<WB_DEPTH, TLB_N>(image(p), img, io_tx, dump_tx);
    collect(io_rx, dump_rx)
}
/// 同上，但 TLB 只有 1 項、寫回深度 2（確定性測試：輸出必須與預設設定完全相同）
fn run_img_small(p: _, img: [u64; DWORDS]) {
    let (io_tx, io_rx) = chan::<u32>(depth = FETCH_LIMIT);
    let (dump_tx, dump_rx) = chan::<u64>(depth = DUMP_LEN);
    spawn Core::<2, 1>(image(p), img, io_tx, dump_tx);
    collect(io_rx, dump_rx)
}
fn run_with(p: _, d: _) { run_img(p, dimage(d)) }
fn run(p: _) { run_with(p, []) }

/// 布局：[0] jal → main；handler 緊接在後（mtvec = 4）；
/// main 開頭自動加 mtvec 設定（li(5,4) + csrw mtvec, x5）。
fn with_handler(handler: _, main: _) -> [u32; IWORDS] {
    let setup = concat(li(5, 4), [csrrw(0, 0x305, 5)]);
    let m = concat(setup, main);
    concat(concat([jal(0, 4 * (1 + len(handler)))], handler),
           concat(m, [0; IWORDS - 1 - len(handler) - len(m)]))
}

/// 記錄型 handler：mepc → x1、mcause → x2、mtval → x3，然後結束模擬
fn record_handler() -> [u32; 4] {
    [csrr(1, 0x341), csrr(2, 0x342), csrr(3, 0x343), simexit()]
}

/// 系統級程式布局（指令 ROM 實體位址）：
///   0x000 jal → boot；0x004 M handler（mtvec）；0x080 S handler（stvec）；
///   0x100 boot（M 模式：設 mtvec/stvec/medeleg/satp/mstatus/mepc 後 mret）；0x200 payload
const MST_S: u32 = 0x800;               // mstatus.MPP = S
const MST_M: u32 = 0x1800;              // mstatus.MPP = M
const SUM: u32 = 0x40000;
const MXR: u32 = 0x80000;
const MPRV: u32 = 0x20000;
const TVM: u32 = 0x100000;
const TSR: u32 = 0x400000;
const SV39: u32 = 0x80000000;           // satp 高 32 位：MODE = 8
const ROOT: u32 = 1;                    // satp.PPN：根頁表在 PA 0x1000
const RWXAD: u64 = 0xCF;                // V R W X A D
const RWAD: u64 = 0xC7;                 // V R W A D

fn pad(p: _, n: u32) { concat(p, [0; n - len(p)]) }

fn sys(mh: _, sh: _, boot: _, payload: _) {
    concat(concat([jal(0, 0x100)], pad(mh, 31)),
           concat(pad(sh, 32), concat(pad(boot, 64), payload)))
}

/// boot：mtvec = 4、stvec = 0x80、medeleg、satp = (hi:lo)、mstatus = mst、mepc = entry → mret
fn boot(mst: u32, satp_hi: u32, satp_lo: u32, deleg: u32, entry: u32) {
    concat(li(5, 4), concat([csrrw(0, 0x305, 5)],
    concat(li(5, 0x80), concat([csrrw(0, 0x105, 5)],
    concat(li(5, deleg), concat([csrrw(0, 0x302, 5)],
    concat(li64(5, satp_hi, satp_lo), concat([csrrw(0, 0x180, 5), sfence_vma(0, 0)],
    concat(li(5, mst), concat([csrrw(0, 0x300, 5)],
    concat(li(5, entry), [csrrw(0, 0x341, 5), mret()])))))))))))
}
fn boot_bare(mst: u32, deleg: u32) { boot(mst, 0, 0, deleg, 0x200) }
fn boot_vm(mst: u32, deleg: u32) { boot(mst, SV39, ROOT, deleg, 0x200) }

/// 記錄型 handler：epc → x1、cause → x2、tval → x3、status → x4，然後結束模擬
fn mrec() -> [u32; 5] { [csrr(1, 0x341), csrr(2, 0x342), csrr(3, 0x343), csrr(4, 0x300), simexit()] }
fn srec() -> [u32; 5] { [csrr(1, 0x141), csrr(2, 0x142), csrr(3, 0x143), csrr(4, 0x100), simexit()] }

// ── 頁表映像 ──
fn pte(pa: u64, flags: u64) -> u64 { ((pa >> 12) << 10) | flags }
fn poke(img: [u64; DWORDS], addr: u64, v: u64) -> [u64; DWORDS] {
    let mut o = img;
    o[addr >> 3] = v;
    o
}
/// 三層頁表骨架：root[0] → L1、L1[0] → L0、L0[0] = 頁 0 的葉（旗標 f0）
fn pt(f0: u64) -> [u64; DWORDS] {
    let m0: [u64; DWORDS] = [0; DWORDS];
    poke(poke(poke(m0, 0x1000, pte(0x2000, PTE_V)), 0x2000, pte(0x3000, PTE_V)), 0x3000, pte(0, f0))
}
fn l0(img: [u64; DWORDS], vpn0: u64, pa: u64, f: u64) -> [u64; DWORDS] { poke(img, 0x3000 + 8 * vpn0, pte(pa, f)) }
fn l1(img: [u64; DWORDS], vpn1: u64, pa: u64, f: u64) -> [u64; DWORDS] { poke(img, 0x2000 + 8 * vpn1, pte(pa, f)) }
fn l2(img: [u64; DWORDS], vpn2: u64, pa: u64, f: u64) -> [u64; DWORDS] { poke(img, 0x1000 + 8 * vpn2, pte(pa, f)) }

fn cause(r: _) -> u64 { r.0[D_CAUSE] }
fn trap_pc(r: _) -> u64 { r.0[D_PC] }
fn retired(r: _) -> u64 { r.0[D_RETIRED] }
fn stalls(r: _) -> u64 { r.0[D_STALLS] }
fn reg(r: _, n: u32) -> u64 { r.0[D_X + n] }
fn mode(r: _) -> u64 { r.0[D_PRIV] }
fn freg(r: _, n: u32) -> u64 { r.0[D_F + n] }
fn output(r: _) { r.1 }
