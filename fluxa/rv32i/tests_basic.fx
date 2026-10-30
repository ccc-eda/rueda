// 手寫測試：每個程式的預期值都不依賴處理器實作（手算，或用 Fluxa 的精確算術算）

test sum_1_to_10 {
    let r = run([
        addi(1, 0, 0), addi(2, 0, 1), addi(3, 0, 11),
        add(1, 1, 2),                       // loop:
        addi(2, 2, 1),
        bne(2, 3, -8),
        ecall()]);
    assert cause(r) == 0;
    assert trap_pc(r) == 24;
    assert reg(r, 1) == 55;
    assert retired(r) == 33;                // 3 + 10 次迴圈 × 3
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
        ecall()]);
    assert output(r) == [0, 1, 1, 2, 3, 5, 8, 13, 21, 34];
}

test alu_register_ops {
    // a = -10, b = 3
    let r = run(concat(concat(li(1, 0xFFFFFFF6), li(2, 3)), [
        add(3, 1, 2), sub(4, 1, 2), sll(5, 1, 2), slt(6, 1, 2), sltu(7, 1, 2),
        xor(8, 1, 2), srl(9, 1, 2), sra(10, 1, 2), or(11, 1, 2), and(12, 1, 2),
        ecall()]));
    assert cause(r) == 0;
    assert reg(r, 3) == 0xFFFFFFF9;         // -7
    assert reg(r, 4) == 0xFFFFFFF3;         // -13
    assert reg(r, 5) == 0xFFFFFFB0;         // -80
    assert reg(r, 6) == 1;                  // -10 < 3（有號）
    assert reg(r, 7) == 0;                  // 0xFFFFFFF6 < 3（無號）為假
    assert reg(r, 8) == 0xFFFFFFF5;
    assert reg(r, 9) == 0x1FFFFFFE;         // 邏輯右移
    assert reg(r, 10) == 0xFFFFFFFE;        // 算術右移：floor(-10 / 8) = -2
    assert reg(r, 11) == 0xFFFFFFF7;
    assert reg(r, 12) == 2;
}

test alu_immediate_ops {
    let r = run([
        addi(1, 0, -1),                     // x1 = 0xFFFFFFFF
        addi(2, 1, 1),                      // 0（回繞）
        slti(3, 1, 0),                      // -1 < 0 → 1
        sltiu(4, 1, 1),                     // 0xFFFFFFFF <u 1 → 0
        sltiu(5, 0, -1),                    // 0 <u 0xFFFFFFFF（立即值符號擴展）→ 1
        xori(6, 1, -1),                     // ~(-1) = 0
        ori(7, 0, 0x7FF),
        andi(8, 1, 0x0F0),
        slli(9, 1, 31),
        srli(10, 1, 31),
        srai(11, 9, 31),                    // 0x80000000 >>a 31 = -1
        srai(12, 1, 4),
        ecall()]);
    assert reg(r, 2) == 0;
    assert reg(r, 3) == 1;
    assert reg(r, 4) == 0;
    assert reg(r, 5) == 1;
    assert reg(r, 6) == 0;
    assert reg(r, 7) == 0x7FF;
    assert reg(r, 8) == 0x0F0;
    assert reg(r, 9) == 0x80000000;
    assert reg(r, 10) == 1;
    assert reg(r, 11) == 0xFFFFFFFF;
    assert reg(r, 12) == 0xFFFFFFFF;
}

test lui_auipc_li {
    let r = run(concat([
        lui(1, 0x12345),
        auipc(2, 0xABCDE),                  // 此指令在 pc = 4
        auipc(3, 0)],
        concat(li(4, 0xDEADBEEF), concat(li(5, 0x00000800), concat(li(6, 0xFFFFF800), [ecall()])))));
    assert reg(r, 1) == 0x12345000;
    assert reg(r, 2) == 0xABCDE004;
    assert reg(r, 3) == 8;
    assert reg(r, 4) == 0xDEADBEEF;
    assert reg(r, 5) == 0x00000800;         // li 的 addi 部分為負，需要 lui 補償
    assert reg(r, 6) == 0xFFFFF800;
}

test x0_is_hardwired_zero {
    let r = run([
        addi(0, 0, 5),                      // 寫入被丟棄
        add(1, 0, 0),
        lui(0, 0xFFFFF),
        lw(0, 0, 0),
        jal(0, 8),
        addi(2, 0, 99),                     // 被 jal 跳過
        addi(3, 0, 1),
        ecall()]);
    assert reg(r, 0) == 0;
    assert reg(r, 1) == 0;
    assert reg(r, 2) == 0;
    assert reg(r, 3) == 1;
}

test load_store_byte_lanes {
    let r = run(concat(li(1, 0x8081FF7F), [
        sw(1, 0, 0),
        lw(2, 0, 0),
        lb(3, 0, 0),                        // 0x7F
        lb(4, 0, 1),                        // 0xFF → -1
        lbu(5, 0, 1),                       // 0xFF
        lh(6, 0, 2),                        // 0x8081 → 符號擴展
        lhu(7, 0, 2),
        lb(8, 0, 3),                        // 0x80 → 符號擴展
        ecall()]));
    assert reg(r, 2) == 0x8081FF7F;
    assert reg(r, 3) == 0x0000007F;
    assert reg(r, 4) == 0xFFFFFFFF;
    assert reg(r, 5) == 0x000000FF;
    assert reg(r, 6) == 0xFFFF8081;
    assert reg(r, 7) == 0x00008081;
    assert reg(r, 8) == 0xFFFFFF80;
}

test partial_stores_merge {
    let r = run(concat(li(1, 0xAA), concat(li(2, 0xBEEF), [
        sb(1, 0, 5),                        // 第 1 個字的第 1 個位元組
        sh(2, 0, 10),                       // 第 2 個字的高半字
        lw(3, 0, 4),
        lw(4, 0, 8),
        addi(5, 0, -1),
        sw(5, 0, 12),
        sb(0, 0, 13),                       // 清掉中間一個位元組
        lw(6, 0, 12),
        ecall()])));
    assert reg(r, 3) == 0x0000AA00;
    assert reg(r, 4) == 0xBEEF0000;
    assert reg(r, 6) == 0xFFFF00FF;
}

test all_six_branches {
    // x1 = -1 (0xFFFFFFFF), x2 = 1。每個分支後面接一個 addi：沒跳才會執行。
    // beq 不跳、bne 跳、blt 跳（有號）、bge 不跳、bltu 不跳（無號）、bgeu 跳  → 3 次沒跳
    let r = run([
        addi(1, 0, -1), addi(2, 0, 1), addi(10, 0, 0),
        beq(1, 2, 8),  addi(10, 10, 1),
        bne(1, 2, 8),  addi(10, 10, 1),
        blt(1, 2, 8),  addi(10, 10, 1),
        bge(1, 2, 8),  addi(10, 10, 1),
        bltu(1, 2, 8), addi(10, 10, 1),
        bgeu(1, 2, 8), addi(10, 10, 1),
        ecall()]);
    assert reg(r, 10) == 3;
}

test jal_and_jalr {
    let r = run([
        jal(1, 12),                         //  0: 呼叫 → 12，x1 = 4
        addi(5, 0, 7),                      //  4: 返回後
        ecall(),                            //  8
        addi(6, 0, 9),                      // 12: 被呼叫者
        jalr(0, 1, 0)]);                    // 16: 返回
    assert cause(r) == 0;
    assert trap_pc(r) == 8;
    assert reg(r, 1) == 4;
    assert reg(r, 5) == 7;
    assert reg(r, 6) == 9;
    assert retired(r) == 4;
}

test jalr_clears_lsb {
    let r = run([
        addi(1, 0, 13),                     //  0
        jalr(2, 1, 4),                      //  4: 目標 17 → 清最低位 → 16；x2 = 8
        addi(3, 0, 1),                      //  8: 被跳過
        addi(3, 0, 2),                      // 12: 被跳過
        ecall()]);                          // 16
    assert reg(r, 2) == 8;
    assert reg(r, 3) == 0;
    assert trap_pc(r) == 16;
}

test raw_hazard_chain_stalls {
    // x1 = 1; x1 = x1 + x1（重複 10 次）→ 1024；每條都相依於前一條
    let r = run(concat([addi(1, 0, 1)], concat(
        [add(1, 1, 1), add(1, 1, 1), add(1, 1, 1), add(1, 1, 1), add(1, 1, 1)],
        [add(1, 1, 1), add(1, 1, 1), add(1, 1, 1), add(1, 1, 1), add(1, 1, 1), ecall()])));
    assert reg(r, 1) == 1024;
    assert stalls(r) > 0;
}

test independent_instructions_do_not_stall {
    let r = run([addi(1, 0, 1), addi(2, 0, 2), addi(3, 0, 3), addi(4, 0, 4), addi(5, 0, 5), ecall()]);
    assert stalls(r) == 0;
    assert reg(r, 5) == 5;
}

test load_use_hazard {
    let r = run_with([
        lw(1, 0, 0),
        addi(2, 1, 1),                      // 立刻使用剛載入的值
        sw(2, 0, 4),
        lw(3, 0, 4),
        add(4, 3, 3),
        ecall()], [41]);
    assert reg(r, 2) == 42;
    assert reg(r, 3) == 42;
    assert reg(r, 4) == 84;
}

test software_multiply {
    // 沒有 M 擴充：用移位加法算 123 * 45
    let r = run([
        addi(1, 0, 123), addi(2, 0, 45), addi(3, 0, 0),
        beq(2, 0, 28),                      // 12: loop → done
        andi(4, 2, 1),
        beq(4, 0, 8),
        add(3, 3, 1),
        slli(1, 1, 1),                      // 28
        srli(2, 2, 1),
        jal(0, -24),                        // 36 → 12
        ecall()]);                          // 40
    assert reg(r, 3) == 5535;
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
        ecall()],                           // 80
        [5, 0xFFFFFFFD, 9, 0, 0xFFFFFFF9, 2, 2, 100]);     // 5, -3, 9, 0, -7, 2, 2, 100
    assert cause(r) == 0;
    assert output(r) == [0xFFFFFFF9, 0xFFFFFFFD, 0, 2, 2, 5, 9, 100];
}

test ebreak_trap {
    let r = run([addi(1, 0, 3), ebreak()]);
    assert cause(r) == 1;
    assert trap_pc(r) == 4;
    assert reg(r, 1) == 3;
}

test illegal_instruction_trap {
    let r = run([addi(1, 0, 3), 0xFFFFFFFF]);
    assert cause(r) == 2;
    assert trap_pc(r) == 4;
    assert reg(r, 1) == 3;
}

test running_off_the_end_traps {
    let r = run([addi(1, 0, 3)]);           // 後面全是 0 = 非法指令
    assert cause(r) == 2;
    assert trap_pc(r) == 4;
}

test reserved_encodings_are_illegal {
    // funct7 非法的 SLLI、f3 = 2 的 BRANCH、f3 = 3 的 LOAD、f3 = 1 的 JALR、SUB 以外的 funct7 = 0x20
    let bad = [
        enc_i(0x400 + 3, 1, 1, 2, 0x13),
        enc_b(8, 1, 1, 2, 0x63),
        enc_i(0, 1, 3, 2, 0x03),
        enc_i(0, 1, 1, 2, 0x67),
        enc_r(0x20, 1, 1, 6, 2, 0x33)];
    for k in 0..5 {
        let r = run([bad[k]]);
        assert cause(r) == 2, "reserved encoding accepted";
    }
}

test branch_out_of_range_traps {
    let r = run([jal(0, 2000)]);             // 跳到指令記憶體之外
    assert cause(r) == 2;
    assert trap_pc(r) == 2000;
}

test misaligned_jump_target_traps {
    let r = run([addi(1, 0, 6), jalr(0, 1, 0)]);   // 目標 6 沒有 4 位元組對齊
    assert cause(r) == 2;
    assert trap_pc(r) == 6;
}

test watchdog_stops_infinite_loop {
    let r = run([jal(0, 0)]);                // 原地跳
    assert cause(r) == 2;
}

test assembler_catches_bad_immediates {
    // 這個測試只確認合法邊界值可以組譯；超界值的錯誤示範在 err_bad_immediate.fx
    assert addi(1, 0, 2047) == 0x7FF00093;
    assert addi(1, 0, -2048) == 0x80000093;
    assert beq(0, 0, -4096) == 0x80000063;
    assert jal(1, 1048574) == 0x7FFFF0EF;
    assert ecall() == 0x00000073;
    assert addi(5, 6, 7) == 0x00730293;
    assert lw(5, 6, 8) == 0x00832283;
    assert sw(5, 6, 8) == 0x00532423;
}
