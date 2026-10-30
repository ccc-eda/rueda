// M4：A 擴充（LR/SC/AMO，單 hart）

/// 資料：[0] = 0x00000000_FFFFFFF0（低 32 位 = -16）、[8] = 100、[16] = -5（64 位）
fn amo_data() { [0xFFFFFFF0, 100, 0xFFFFFFFFFFFFFFFB] }

test amo_word_ops {
    // 每個 AMO.W 對 [0]（-16）做一次，rs2 = 3；rd 收到的是運算前的舊值（符號擴展）
    let ops = [amoswap_w(9, 2, 0), amoadd_w(9, 2, 0), amoxor_w(9, 2, 0), amoand_w(9, 2, 0),
               amoor_w(9, 2, 0), amomin_w(9, 2, 0), amomax_w(9, 2, 0), amominu_w(9, 2, 0), amomaxu_w(9, 2, 0)];
    // -16 op 3 的結果（低 32 位）
    let want = [3, 0xFFFFFFF3, 0xFFFFFFF3, 0, 0xFFFFFFF3, 0xFFFFFFF0, 3, 3, 0xFFFFFFF0];
    for k in 0..9 {
        let r = run_with([addi(2, 0, 3), ops[k], lw(10, 0, 0), lw(11, 0, 4), simexit()], amo_data());
        assert cause(r) == 0;
        assert reg(r, 9) == 0xFFFFFFFFFFFFFFF0, "AMO.W returns the sign-extended old value";
        assert reg(r, 10) == want[k].wrap::<i32>().wrap::<u64>(), "AMO.W result";
        assert reg(r, 11) == 0, "AMO.W must not touch the upper word";
    }
}

test amo_double_ops {
    // 對 [16]（-5）做 AMO.D，rs2 = 7
    let ops = [amoswap_d(9, 2, 1), amoadd_d(9, 2, 1), amoxor_d(9, 2, 1), amoand_d(9, 2, 1),
               amoor_d(9, 2, 1), amomin_d(9, 2, 1), amomax_d(9, 2, 1), amominu_d(9, 2, 1), amomaxu_d(9, 2, 1)];
    let want = [7, 2, 0xFFFFFFFFFFFFFFFC, 3, 0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFB, 7, 7, 0xFFFFFFFFFFFFFFFB];
    for k in 0..9 {
        let r = run_with([addi(1, 0, 16), addi(2, 0, 7), ops[k], ld(10, 1, 0), simexit()], amo_data());
        assert reg(r, 9) == 0xFFFFFFFFFFFFFFFB;
        assert reg(r, 10) == want[k], "AMO.D result";
    }
}

/// AMOMIN.W 的比較是 32 位有號，AMOMINU.W 是 32 位無號（不看上半部）
test amo_w_compares_low_word {
    let r = run_with(concat([addi(1, 0, 8)], concat(li64(2, 0x7FFFFFFF, 50), [amominu_w(9, 2, 1), lw(10, 1, 0),
                      addi(1, 0, 24), amomin_w(9, 2, 1), lw(11, 1, 0), simexit()])),
                     [0, 100, 0, 0xFFFFFFFFFFFFFFFF]);
    assert reg(r, 10) == 50;                // minu(100, 50)
    assert reg(r, 11) == 0xFFFFFFFFFFFFFFFF; // min(-1, 50) = -1
}

test lr_sc_success_and_failure {
    let r = run_with([
        addi(1, 0, 8),
        lr_d(2, 1),                          // x2 = 100，保留 [8]
        addi(3, 2, 1),
        sc_d(4, 3, 1),                       // 成功：x4 = 0，[8] = 101
        sc_d(5, 3, 1),                       // 保留已被清掉：失敗，x5 = 1
        lr_w(6, 1),
        addi(7, 0, 16),
        sc_w(8, 3, 7),                       // 位址不同：失敗
        sc_w(10, 3, 1),                      // 前一個 SC 已清保留：失敗
        ld(11, 1, 0),
        simexit()], amo_data());
    assert reg(r, 2) == 100;
    assert reg(r, 4) == 0;
    assert reg(r, 5) == 1;
    assert reg(r, 8) == 1;
    assert reg(r, 10) == 1;
    assert reg(r, 11) == 101;
}

/// 經典 LR/SC 原子遞增迴圈：10 次
test lr_sc_increment_loop {
    let r = run_with([
        addi(1, 0, 8), addi(5, 0, 10),
        lr_w(2, 1),                          // retry:
        addi(2, 2, 1),
        sc_w(3, 2, 1),
        bne(3, 0, -12),
        addi(5, 5, -1),
        bne(5, 0, -20),
        lw(9, 1, 0),
        simexit()], amo_data());
    assert reg(r, 9) == 110;
}

test amo_rd_zero_still_updates_memory {
    let r = run_with([addi(2, 0, 5), amoadd_d(0, 2, 0), ld(9, 0, 0), simexit()], amo_data());
    assert reg(r, 9) == 0xFFFFFFF5;
    assert reg(r, 0) == 0;
}

test amo_exceptions {
    // 未對齊：LR → 載入未對齊（4）；SC/AMO → 儲存未對齊（6）
    let r1 = run(with_handler(record_handler(), [addi(6, 0, 4), lr_d(7, 6)]));
    assert reg(r1, 2) == 4;
    assert reg(r1, 3) == 4;
    let r2 = run(with_handler(record_handler(), [addi(6, 0, 2), amoadd_w(7, 0, 6)]));
    assert reg(r2, 2) == 6;
    let r3 = run(with_handler(record_handler(), [addi(6, 0, 4), sc_d(7, 0, 6)]));
    assert reg(r3, 2) == 6;
    // 超出 RAM：LR → 5；AMO → 7；AMO 到 MMIO 也是 7
    let r4 = run(with_handler(record_handler(), concat(li(6, 0x10000), [lr_w(7, 6)])));
    assert reg(r4, 2) == 5;
    let r5 = run(with_handler(record_handler(), concat(li(6, 0x10000), [amoswap_w(7, 0, 6)])));
    assert reg(r5, 2) == 7;
    let r6 = run(with_handler(record_handler(), [lui(6, 0x10000), amoor_w(7, 0, 6)]));
    assert reg(r6, 2) == 7;
    assert reg(r6, 3) == 0x10000000;
    // 保留編碼：LR 的 rs2 ≠ 0、未定義 funct5、f3 ∉ {2, 3}
    let bad = [enc_amo(0x02, 1, 0, 2, 7), enc_amo(0x05, 0, 0, 2, 7), enc_amo(0x00, 0, 0, 0, 7)];
    for k in 0..3 {
        let r = run(with_handler(record_handler(), [bad[k]]));
        assert reg(r, 2) == 2, "reserved AMO encoding accepted";
        assert reg(r, 3) == bad[k];
    }
}
