// 手寫測試：每個程式的預期值都不依賴處理器實作（手算，或用 Fluxa 的精確算術算）

test alu_register_ops {
    // a = -10 (0xFFFFFFFFFFFFFFF6), b = 3
    let r = run(concat(concat(li(1, 0xFFFFFFF6), [addi(2, 0, 3)]), [
        add(3, 1, 2), sub(4, 1, 2), sll(5, 1, 2), slt(6, 1, 2), sltu(7, 1, 2),
        xor(8, 1, 2), srl(9, 1, 2), sra(10, 1, 2), or(11, 1, 2), and(12, 1, 2),
        simexit()]));
    assert cause(r) == 0;
    assert reg(r, 3) == 0xFFFFFFFFFFFFFFF9;   // -7
    assert reg(r, 4) == 0xFFFFFFFFFFFFFFF3;   // -13
    assert reg(r, 5) == 0xFFFFFFFFFFFFFFB0;   // -80
    assert reg(r, 6) == 1;                    // -10 < 3（有號）
    assert reg(r, 7) == 0;                    // 0xFFFF...F6 <u 3 為假
    assert reg(r, 8) == 0xFFFFFFFFFFFFFFF5;
    assert reg(r, 9) == 0x1FFFFFFFFFFFFFFE;   // 邏輯右移
    assert reg(r, 10) == 0xFFFFFFFFFFFFFFFE;  // floor(-10 / 8) = -2
    assert reg(r, 11) == 0xFFFFFFFFFFFFFFF7;
    assert reg(r, 12) == 2;
}

test alu_immediate_ops {
    let r = run([
        addi(1, 0, -1),                     // x1 = 0xFFFFFFFFFFFFFFFF
        addi(2, 1, 1),                      // 0（回繞）
        slti(3, 1, 0),                      // -1 < 0 → 1
        sltiu(4, 1, 1),                     // 0xFFFF...FF <u 1 → 0
        sltiu(5, 0, -1),                    // 0 <u 0xFFFF...FF（立即值符號擴展）→ 1
        xori(6, 1, -1),                     // ~(-1) = 0
        ori(7, 0, 0x7FF),
        andi(8, 1, 0x0F0),
        slli(9, 1, 63),                     // 0x8000000000000000
        srli(10, 9, 63),                    // 1
        srai(11, 9, 63),                    // -1
        srai(12, 1, 4),                     // -1
        simexit()]);
    assert reg(r, 2) == 0;
    assert reg(r, 3) == 1;
    assert reg(r, 4) == 0;
    assert reg(r, 5) == 1;
    assert reg(r, 6) == 0;
    assert reg(r, 7) == 0x7FF;
    assert reg(r, 8) == 0xF0;
    assert reg(r, 9) == 0x8000000000000000;
    assert reg(r, 10) == 1;
    assert reg(r, 11) == 0xFFFFFFFFFFFFFFFF;
    assert reg(r, 12) == 0xFFFFFFFFFFFFFFFF;
}

/// RV64 移位量 6 位元；W 型移位量 5 位元且只看低 32 位
test shift_shamt_mask {
    let r = run([
        addi(1, 0, 1),
        addi(2, 0, 33),
        sll(3, 1, 2),                       // sh = 33 → 2^33
        sllw(4, 1, 2),                      // sh = 33 & 31 = 1 → 2
        srli(5, 1, 33),                     // 立即移位允許到 63
        srai(6, 1, 33),
        slli(7, 1, 40),                     // 2^40
        simexit()]);
    assert reg(r, 3) == 0x200000000;
    assert reg(r, 4) == 2;
    assert reg(r, 5) == 0;
    assert reg(r, 6) == 0;
    assert reg(r, 7) == 0x10000000000;
}

/// lui/auipc 在 RV64 是 32 位元結果符號擴展到 64 位元
test lui_auipc_sext {
    let r = run([
        lui(1, 0x80000),                    // 0x80000000 sext
        auipc(2, 0x80000),                  // 此指令在 pc = 4
        simexit()]);
    assert reg(r, 1) == 0xFFFFFFFF80000000;
    assert reg(r, 2) == 0xFFFFFFFF80000004;
}

test w_arith {
    let r = run(concat(li(1, 0x7FFFFFFF), concat([
        addi(2, 0, 1),
        addw(3, 1, 2)],                     // 0x80000000（32 位元回繞）→ 符號擴展
        concat(li(4, 0x80000000), [         // x4 = 0xFFFFFFFF80000000
        addiw(5, 4, -1),                    // 0x7FFFFFFF（正）
        subw(6, 4, 2),                      // 0x7FFFFFFF
        simexit()]))));
    assert reg(r, 3) == 0xFFFFFFFF80000000;
    assert reg(r, 5) == 0x7FFFFFFF;
    assert reg(r, 6) == 0x7FFFFFFF;
}

/// W 型與非 W 型移位的對照：W 型結果符號擴展
test w_shifts_sext {
    let r = run(concat(li(4, 0x80000000), [
        addi(1, 0, 1),
        slliw(2, 1, 31),                    // 0x80000000 → sext
        slli(3, 1, 31),                     // 非 W 型：不擴展
        srliw(5, 4, 16),                    // 0x8000（正）
        sraiw(6, 4, 16),                    // 0xFFFF8000 → sext
        simexit()]));
    assert reg(r, 2) == 0xFFFFFFFF80000000;
    assert reg(r, 3) == 0x80000000;
    assert reg(r, 5) == 0x8000;
    assert reg(r, 6) == 0xFFFFFFFFFFFF8000;
}

/// W 型暫存器移位的 shamt 取 b[4:0]；非 W 型取 b[5:0]
test w_shifts_shamt_mask {
    let r = run(concat(li(1, 0x80000000), [   // x1 = 0xFFFFFFFF80000000
        addi(2, 0, 33),
        sraw(3, 1, 2),                      // sh = 1：0x80000000 >>a 1 → sext
        srlw(4, 1, 2),                      // 0x40000000
        sra(5, 1, 2),                       // sh = 33：-2^31 >>a 33 = -1
        srl(6, 1, 2),                       // 0xFFFFFFFF80000000 >>u 33 = 0x7FFFFFFF
        simexit()]));
    assert reg(r, 3) == 0xFFFFFFFFC0000000;
    assert reg(r, 4) == 0x40000000;
    assert reg(r, 5) == 0xFFFFFFFFFFFFFFFF;
    assert reg(r, 6) == 0x7FFFFFFF;
}

/// M 擴充高位結果：mul_hi_lo 以 32 位元分塊繞過 120 位元精確整數上限
test mul_high {
    let r = run(concat(li(1, 0x80000000), [
        slli(1, 1, 32),                     // x1 = 0x8000000000000000（-2^63）
        addi(2, 0, 2),
        mul(3, 1, 2),                       // -2^64 mod 2^64 = 0
        mulhu(4, 1, 2),                     // 2^64 的高半 = 1
        mulh(5, 1, 2),                      // -2^64 >> 64 = -1
        mulhsu(6, 1, 2),                    // 有號 -2^63 × 無號 2 = -2^64 → 高半 -1
        addi(7, 0, -1),
        mul(8, 7, 7),                       // (-1)^2 mod 2^64 = 1
        mulh(9, 7, 7),                      // 0
        mulhsu(10, 7, 2),                   // -1 × 2 = -2 → 高半 -1
        simexit()]));
    assert reg(r, 3) == 0;
    assert reg(r, 4) == 1;
    assert reg(r, 5) == 0xFFFFFFFFFFFFFFFF;
    assert reg(r, 6) == 0xFFFFFFFFFFFFFFFF;
    assert reg(r, 8) == 1;
    assert reg(r, 9) == 0;
    assert reg(r, 10) == 0xFFFFFFFFFFFFFFFF;
}

/// 除法捨入方向：trunc toward zero（-100/7 = -14 餘 -2）
test div_rem {
    let r = run([
        addi(1, 0, 100), addi(2, 0, 7),
        div(3, 1, 2),                       // 14
        rem(4, 1, 2),                       // 2
        addi(5, 0, -100),
        div(6, 5, 2),                       // -14（向零捨入，不是 floor 的 -15）
        rem(7, 5, 2),                       // -2（餘數符號隨被除數）
        simexit()]);
    assert reg(r, 3) == 14;
    assert reg(r, 4) == 2;
    assert reg(r, 6) == 0xFFFFFFFFFFFFFFF2;
    assert reg(r, 7) == 0xFFFFFFFFFFFFFFFE;
}

/// 除法特殊值：overflow（-2^63 / -1）與除以零
test div_edge {
    let r = run(concat(li(1, 0x80000000), [
        slli(1, 1, 32),                     // -2^63
        addi(2, 0, -1),
        div(3, 1, 2),                       // overflow → 回 -2^63
        rem(4, 1, 2),                       // 0
        div(5, 1, 0),                       // /0 → -1
        divu(6, 1, 0),                      // /0 → 2^64-1
        rem(7, 1, 0),                       // %0 → 被除數
        remu(8, 1, 0),                      // → 被除數
        addi(14, 0, 2),
        divu(9, 1, 14),                     // 2^63 / 2 = 2^62
        addi(10, 0, -1), addi(11, 0, 3),
        divu(12, 10, 11),                   // 0xFFFF...FF / 3
        remu(13, 10, 11),                   // 0
        simexit()]));
    assert reg(r, 3) == 0x8000000000000000;
    assert reg(r, 4) == 0;
    assert reg(r, 5) == 0xFFFFFFFFFFFFFFFF;
    assert reg(r, 6) == 0xFFFFFFFFFFFFFFFF;
    assert reg(r, 7) == 0x8000000000000000;
    assert reg(r, 8) == 0x8000000000000000;
    assert reg(r, 9) == 0x4000000000000000;
    assert reg(r, 12) == 0x5555555555555555;
    assert reg(r, 13) == 0;
}

test mulw_divw {
    let r = run(concat(li(1, 0xFFFFFFFF), [
        addi(2, 0, 2),
        mulw(3, 1, 2),                      // 0xFFFFFFFF×2 低 32 = 0xFFFFFFFE → sext
        addi(4, 0, 100), addi(5, 0, 7),
        divw(6, 4, 5),                      // 14
        remw(7, 4, 5),                      // 2
        simexit()]));
    assert reg(r, 3) == 0xFFFFFFFFFFFFFFFE;
    assert reg(r, 6) == 14;
    assert reg(r, 7) == 2;
}

test divw_edge {
    let r = run(concat(li(1, 0x80000000), concat([   // x1 = 0xFFFFFFFF80000000（a32 = -2^31）
        addi(2, 0, -1),                     // b32 = -1
        divw(3, 1, 2),                      // overflow → 0x80000000 sext
        remw(4, 1, 2),                      // 0
        divw(5, 1, 0),                      // /0 → -1
        remw(6, 1, 0),                      // sext32(0x80000000)
        addi(7, 0, 3)],
        concat(li(14, 0xFFFFFFFF), [        // x14 = 0xFFFFFFFFFFFFFFFF（a32 = 0xFFFFFFFF）
        divuw(8, 14, 7),                    // 0xFFFFFFFF / 3 = 0x55555555（正）
        remuw(9, 14, 7),                    // 0
        simexit()]))));
    assert reg(r, 3) == 0xFFFFFFFF80000000;
    assert reg(r, 4) == 0;
    assert reg(r, 5) == 0xFFFFFFFFFFFFFFFF;
    assert reg(r, 6) == 0xFFFFFFFF80000000;
    assert reg(r, 8) == 0x55555555;
    assert reg(r, 9) == 0;
}

/// li64 + 64 位元 load/store 與各寬度符號/零擴展
test li64_and_load_store {
    let r = run(concat(li64(1, 0x11223344, 0x55667788), concat([
        sd(1, 0, 0),
        ld(2, 0, 0),
        lwu(3, 0, 4),                       // 0x11223344
        lwu(4, 0, 0),                       // 0x55667788
        lw(5, 0, 4),                        // 正數，sext 相同
        lhu(6, 0, 6),                       // 0x1122
        lbu(7, 0, 7)],                      // 0x11
        concat(li(9, 0x80000000), [
        sw(9, 0, 8),                        // 只存低 32 位
        lw(10, 0, 8),                       // 0x80000000 → sext → 負
        lwu(11, 0, 8),                      // 0x80000000（零擴展）
        simexit()]))));
    assert reg(r, 1) == 0x1122334455667788;
    assert reg(r, 2) == 0x1122334455667788;
    assert reg(r, 3) == 0x11223344;
    assert reg(r, 4) == 0x55667788;
    assert reg(r, 5) == 0x11223344;
    assert reg(r, 6) == 0x1122;
    assert reg(r, 7) == 0x11;
    assert reg(r, 10) == 0xFFFFFFFF80000000;
    assert reg(r, 11) == 0x80000000;
}

test partial_stores_merge {
    let r = run(concat(li(1, 0xAA), concat(li(2, 0xBEEF), [
        sb(1, 0, 5),                        // 第 1 個字的第 5 個位元組
        sh(2, 0, 10),                       // 第 2 個字的高 16 位元
        ld(3, 0, 0),
        ld(4, 0, 8),
        simexit()])));
    assert reg(r, 3) == 0x0000AA0000000000;
    assert reg(r, 4) == 0x00000000BEEF0000;   // byte10-11 = 第 2 個字的 bits[31:16]
}

test jal_and_jalr {
    let r = run([
        jal(1, 12),                         //  0: 呼叫 → 12，x1 = 4
        addi(5, 0, 7),                      //  4: 返回後
        simexit(),                          //  8
        addi(6, 0, 9),                      // 12: 被呼叫者
        jalr(0, 1, 0)]);                    // 16: 返回
    assert cause(r) == 0;
    assert trap_pc(r) == 8;
    assert reg(r, 1) == 4;
    assert reg(r, 5) == 7;
    assert reg(r, 6) == 9;
    assert retired(r) == 4;
}

test all_six_branches {
    // x1 = -1 (0xFFFF...FF), x2 = 1。每個分支後面接一個 addi：沒跳才會執行。
    // beq 不跳、bne 跳、blt 跳（有號）、bge 不跳、bltu 不跳（無號）、bgeu 跳  → 3 次沒跳
    let r = run([
        addi(1, 0, -1), addi(2, 0, 1), addi(10, 0, 0),
        beq(1, 2, 8),  addi(10, 10, 1),
        bne(1, 2, 8),  addi(10, 10, 1),
        blt(1, 2, 8),  addi(10, 10, 1),
        bge(1, 2, 8),  addi(10, 10, 1),
        bltu(1, 2, 8), addi(10, 10, 1),
        bgeu(1, 2, 8), addi(10, 10, 1),
        simexit()]);
    assert reg(r, 10) == 3;
}

test fibonacci_via_mmio {
    let r = run([
        lui(10, 0x10000),                   // x10 = MMIO_OUT
        addi(1, 0, 0), addi(2, 0, 1), addi(3, 0, 10),
        sw(1, 10, 0),                       // loop: 輸出 a
        add(4, 1, 2),
        addi(1, 2, 0),
        addi(2, 4, 0),
        addi(3, 3, -1),
        bne(3, 0, -20),
        simexit()]);
    assert output(r) == [0, 1, 1, 2, 3, 5, 8, 13, 21, 34];
}

test bubble_sort_in_memory {
    let r = run_with([
        addi(2, 0, 8),                      //  0: n
        addi(3, 2, -1),                     //  4: i = n - 1
        addi(4, 0, 0),                      //  8: outer: j = 0
        slli(5, 4, 2),                      // 12: inner: off = j * 4
        lw(6, 5, 0),                        // 16
        lw(7, 5, 4),                        // 20
        bge(7, 6, 12),                      // 24: 已排序就跳過交換
        sw(7, 5, 0),                        // 28
        sw(6, 5, 4),                        // 32
        addi(4, 4, 1),                      // 36: noswap
        blt(4, 3, -28),                     // 40 → inner
        addi(3, 3, -1),                     // 44
        blt(0, 3, -40),                     // 48 → outer
        lui(10, 0x10000),                   // 52
        addi(4, 0, 0),                      // 56
        slli(5, 4, 2),                      // 60: out:
        lw(6, 5, 0),                        // 64
        sw(6, 10, 0),                       // 68
        addi(4, 4, 1),                      // 72
        blt(4, 2, -16),                     // 76 → out
        simexit()],                           // 80
        dimage32([5, 0xFFFFFFFD, 9, 0, 0xFFFFFFF9, 2, 2, 100]));     // 5, -3, 9, 0, -7, 2, 2, 100
    assert cause(r) == 0;
    assert output(r) == [0xFFFFFFF9, 0xFFFFFFFD, 0, 2, 2, 5, 9, 100];
}

// ════════════════════════════════════════════════════════════════════════════
//  M2：Zicsr 與 M 模式 trap
// ════════════════════════════════════════════════════════════════════════════

// with_handler / record_handler 在 rv64.fx 的 harness 區（各測試檔共用）

// main 的第一條使用者指令位址：setup（li 2 條 + csrw 1 條）= idx 1+hlen+3
// ecall/ebreak/非法指令都在 main 開頭 → pc = 4 * (hlen + 4)

test ecall_trap_enters_handler {
    let r = run(with_handler(record_handler(), [ecall()]));
    assert reg(r, 1) == 32;     // mepc = ecall 的 pc
    assert reg(r, 2) == 11;     // mcause = M 模式 ecall
    assert reg(r, 3) == 0;      // mtval = 0
}

test ebreak_trap {
    let r = run(with_handler(record_handler(), [ebreak()]));
    assert reg(r, 1) == 32;
    assert reg(r, 2) == 3;      // mcause = breakpoint
}

test illegal_instruction_trap_records_mtval {
    let r = run(with_handler(record_handler(), [0xFFFFFFFF]));
    assert reg(r, 1) == 32;
    assert reg(r, 2) == 2;
    assert reg(r, 3) == 0xFFFFFFFF;     // mtval = 指令本身
}

test fetch_out_of_range_trap {
    // main 在 pc 32；jal(0, 1968) → 目標 2000（越過指令 ROM）→ 取指存取錯誤（cause 1）
    let r = run(with_handler(record_handler(), [jal(0, 1968)]));
    assert reg(r, 2) == 1;
    assert reg(r, 1) == 2000;   // mepc = 越界的取指位址
    assert reg(r, 3) == 2000;
}

/// 跳躍目標未對齊（IALIGN = 32）：例外報在跳躍指令上（mepc = 跳躍指令、mtval = 目標）
test misaligned_jump_target_trap {
    let r1 = run(with_handler(record_handler(), [addi(1, 0, 6), jalr(0, 1, 0)]));
    assert reg(r1, 2) == 0;
    assert reg(r1, 1) == 36;    // jalr 的 pc
    assert reg(r1, 3) == 6;
    let r2 = run(with_handler(record_handler(), [jal(0, 2)]));
    assert reg(r2, 2) == 0;
    assert reg(r2, 1) == 32;
    assert reg(r2, 3) == 34;
    // 分支：只有成立時才檢查目標
    let r3 = run(with_handler(record_handler(), [bne(0, 0, 6), beq(0, 0, 6)]));
    assert reg(r3, 2) == 0;
    assert reg(r3, 1) == 36;
    assert reg(r3, 3) == 42;
}

/// mret：handler 記錄 mcause、把 mepc 推進 4、返回主程式繼續執行
test mret_returns {
    let handler = [csrr(2, 0x342), csrr(1, 0x341), addi(1, 1, 4), csrrw(0, 0x341, 1), mret()];
    let r = run(with_handler(handler, [ecall(), addi(9, 0, 99), simexit()]));
    assert reg(r, 9) == 99;     // mret 回到 ecall 的下一條（mepc + 4）
    assert reg(r, 2) == 11;
}

/// mstatus 的 trap 進出：MIE[3]、MPIE[7]、MPP[12:11]
test mstatus_trap_and_mret {
    let handler = [csrr(1, 0x300), csrr(2, 0x341), addi(2, 2, 4), csrrw(0, 0x341, 2), mret()];
    let main = [addi(4, 0, 8), csrrw(0, 0x300, 4), ecall(), csrr(5, 0x300), simexit()];
    let r = run(with_handler(handler, main));
    // UXL = SXL = 2（唯讀）恆在 bits 35:32
    assert reg(r, 1) == 0xA00001880;   // trap 進入：MPIE = MIE(1)、MPP = 3、MIE = 0
    assert reg(r, 5) == 0xA00000088;   // mret 之後：MIE = MPIE(1)、MPIE = 1、MPP = U（最低支援模式）
}

/// 向量模式：mtvec[1:0] = 1 只影響中斷（BASE + 4×cause）；同步例外一律跳 BASE
test mtvec_vectored_trap {
    let prog = concat([jal(0, 128)],                    // 0x00 → main @ 0x80
        concat([nop(); 15],                             // idx 1..15
        concat([addi(9, 0, 1)],                         // idx 16 = 0x40（BASE）
        concat([nop(); 10],                             // idx 17..26
        concat([csrr(2, 0x342), simexit()],             // idx 27..28 = 0x6C（cause 11 的向量位置）
        concat([nop(); 3],                              // idx 29..31
        concat(li(5, 0x41), [csrrw(0, 0x305, 5), ecall()])))))));   // idx 32..35
    let r = run(prog);
    assert reg(r, 2) == 11;
    assert reg(r, 9) == 1;      // 從 BASE 進入（若跳向量位置，x9 不會被設）
}

/// 載入/儲存的未對齊與越界 → 例外（mcause 4/5/6/7，mtval = 故障位址）
test load_store_faults {
    let r1 = run(with_handler(record_handler(), [addi(6, 0, 2), lw(7, 6, 0)]));
    assert reg(r1, 2) == 4;     // 載入位址未對齊
    assert reg(r1, 3) == 2;     // mtval = 故障位址

    let r2 = run(with_handler(record_handler(), concat(li(6, 0x10000), [lw(7, 6, 0)])));
    assert reg(r2, 2) == 5;     // 載入存取錯誤（超出資料記憶體）
    assert reg(r2, 3) == 0x10000;

    let r3 = run(with_handler(record_handler(), [addi(6, 0, 2), sw(7, 6, 0)]));
    assert reg(r3, 2) == 6;     // 儲存位址未對齊
    assert reg(r3, 3) == 2;

    let r4 = run(with_handler(record_handler(), concat(li(6, 0x10000), [sw(7, 6, 0)])));
    assert reg(r4, 2) == 7;     // 儲存存取錯誤
    assert reg(r4, 3) == 0x10000;
}

test csr_read_write {
    let r = run([
        addi(2, 0, 123),
        csrrw(1, 0x340, 2),     // x1 = 舊值 0；mscratch = 123
        csrrs(3, 0x340, 0),     // x3 = 123；rs1 = x0 → 不寫
        addi(4, 0, 0xFF),
        csrrs(5, 0x340, 4),     // x5 = 123；mscratch |= 0xFF
        csrrc(6, 0x340, 4),     // x6 = 255；mscratch &= ~0xFF
        csrr(7, 0x340),         // x7 = 0
        simexit()]);
    assert reg(r, 1) == 0;
    assert reg(r, 3) == 123;
    assert reg(r, 5) == 123;
    assert reg(r, 6) == 255;
    assert reg(r, 7) == 0;
}

test csr_immediate_forms {
    let r = run([
        csrrwi(1, 0x340, 5),    // x1 = 0；mscratch = 5
        csrrsi(2, 0x340, 2),    // x2 = 5；mscratch = 7
        csrrci(3, 0x340, 1),    // x3 = 7；mscratch = 6
        csrr(4, 0x340),         // x4 = 6；zimm = 0 → 不寫
        simexit()]);
    assert reg(r, 1) == 0;
    assert reg(r, 2) == 5;
    assert reg(r, 3) == 7;
    assert reg(r, 4) == 6;
}

test csr_protection {
    // 寫唯讀 CSR（mhartid）→ 非法指令例外，mtval = 指令本身
    let r = run(with_handler(record_handler(), [csrrw(8, 0xF14, 0)]));
    assert reg(r, 2) == 2;
    assert reg(r, 3) == csrrw(8, 0xF14, 0);
}

test csr_unknown_is_illegal {
    let r = run(with_handler(record_handler(), [csrrw(8, 0x7FF, 0)]));
    assert reg(r, 2) == 2;
}

test csr_ro_read_ok {
    // csrrs with rs1 = x0 是純讀取，對唯讀 CSR 合法
    let r = run([csrrs(9, 0xF14, 0), simexit()]);
    assert reg(r, 9) == 0;      // mhartid = 0
    assert cause(r) == 0;
}

test watchdog_stops_infinite_loop {
    let r = run([jal(0, 0)]);   // 原地跳 → 看門狗 → cause 2 收尾
    assert cause(r) == 2;
}

test reserved_encodings_are_illegal {
    // RV64 特有的保留編碼：SLLI 的 imm[11:6] 非 0、未定義的 f3 組合
    let bad = [
        enc_i(0x400 + 3, 1, 1, 2, OPIMM),   // SLLI 混入 SRAI 的 imm[11:6] = 010000
        enc_i(0, 1, 7, 2, LOAD),            // LOAD f3 = 7 未定義
        enc_s(0, 1, 1, 4, STORE),           // STORE f3 = 4 未定義
        enc_b(8, 1, 1, 2, BRANCH),          // BRANCH f3 = 2 未定義
        enc_i(1, 1, 4, 2, OPIMM32),         // OP-IMM-32 只有 f3 = 0/1/5
        enc_r(0, 1, 1, 2, 2, OP32),         // OP-32 只有 f3 = 0/1/5（f7 = 0）
        enc_r(1, 1, 1, 1, 2, OP32)];        // M32 只用 f3 = 0/4..7
    for k in 0..7 {
        let r = run(with_handler(record_handler(), [bad[k]]));
        assert reg(r, 2) == 2, "reserved encoding accepted";
        assert reg(r, 3) == bad[k], "mtval should be the instruction word";
    }
}

/// 連續 30 個互不相依的寫回（含 CSR 讀取這種停等型指令），寫回深度只有 2：
/// Decode 的在途寫回上限保證不死結（rv32i 的 err_wb_depth.fx 示範的是沒有上限時的死結）
test independent_writes_exceed_wb_depth {
    let mut p = [];
    for k in 1..31 {
        p = concat(p, if k % 3 == 0 { [csrr(k, 0xF14)] } else { [addi(k, 0, k)] });
    }
    let a = run_img_small(image(concat(p, [simexit()])), dimage([]));
    assert cause(a) == 0;
    assert reg(a, 1) == 1;
    assert reg(a, 29) == 29;
    assert reg(a, 30) == 0;
}
