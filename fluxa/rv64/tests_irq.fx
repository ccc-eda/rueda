// M6：計數器 CSR、以退休指令數驅動的 timer 中斷、中斷委派與優先序
//
// mtime = 已退休指令數（行為模擬沒有時鐘；完全可重現），mtimecmp 是自訂 M 模式 CSR 0x7C1。
// 中斷只在「等待點」（分支/跳躍、可能陷阱的指令、取指翻譯）接受。

const IRQ: u64 = 0x8000000000000000;

test counters_basic {
    let r = run(concat([
        csrr(10, 0xC02),                    // instret：之前退休 0 條
        nop(), nop(),
        csrr(11, 0xC02),                    // 3
        csrr(12, 0xC00),                    // cycle = 4
        csrr(13, 0xC01)],                   // time = 5
        concat(li(5, 100), [
        csrrw(0, 0xB02, 5),                 // 寫 minstret 取代這條指令的遞增
        csrr(14, 0xB02),                    // 100
        csrr(15, 0xC02),                    // 101
        simexit()])));
    assert reg(r, 10) == 0;
    assert reg(r, 11) == 3;
    assert reg(r, 12) == 4;
    assert reg(r, 13) == 5;
    assert reg(r, 14) == 100;
    assert reg(r, 15) == 101;
}

test counter_permissions {
    // U：mcounteren = 0 → 非法
    let r1 = run(sys(mrec(), [], boot_bare(0, 0), [csrr(7, 0xC01)]));
    assert reg(r1, 2) == 2;
    // U：mcounteren = 7、scounteren = TM → time 可讀、cycle 不行
    let pre = [csrrwi(0, 0x306, 7), csrrwi(0, 0x106, 2)];
    let r2 = run(sys(mrec(), [], concat(pre, boot_bare(0, 0)), [csrr(7, 0xC01), csrr(8, 0xC00)]));
    assert reg(r2, 2) == 2;
    assert reg(r2, 3) == csrr(8, 0xC00);
    // S：只看 mcounteren
    let r3 = run(sys(mrec(), [], concat([csrrwi(0, 0x306, 7)], boot_bare(MST_S, 0)), [csrr(7, 0xC00), simexit()]));
    assert cause(r3) == 0;
    // 使用者計數器是唯讀的
    let r4 = run(with_handler(record_handler(), [csrrw(0, 0xC00, 0)]));
    assert reg(r4, 2) == 2;
}

/// M 模式 timer 中斷：mtimecmp = 50，`j .` 迴圈在等待點（jal）接受中斷
test timer_interrupt_m_mode {
    let r = run(with_handler(record_handler(), concat(li(5, 50), concat([csrrw(0, 0x7C1, 5)],
        concat(li(5, 0x80), [csrrw(0, 0x304, 5), csrrsi(0, 0x300, 8),
        addi(9, 0, 0),
        addi(9, 9, 1),                      // 64: loop
        jal(0, -4)])))));                   // 68
    assert reg(r, 2) == IRQ | 7;
    assert reg(r, 1) == 68;                 // mepc = 尚未執行的 jal
    assert reg(r, 3) == 0;
    assert reg(r, 9) == 20;                 // jal 前退休數 = 11 + 2k ≥ 50 → 第 20 圈
}

/// M 模式且 MIE = 0：中斷擱置（mip.MTIP = 1）但不接受
test interrupt_masked_in_m_mode {
    let r = run([csrrwi(0, 0x7C1, 0), addi(5, 0, 0x80), csrrw(0, 0x304, 5),
                 addi(9, 0, 30), addi(9, 9, -1), bne(9, 0, -4), csrr(10, 0x344), simexit()]);
    assert cause(r) == 0;
    assert reg(r, 9) == 0;
    assert reg(r, 10) == 0x80;
}

/// U 模式下 M 層中斷不看 MIE：mret 進 U 後的第一個等待點（取指翻譯）就接受
test timer_interrupt_from_user {
    let pre = [csrrwi(0, 0x7C1, 0), addi(5, 0, 0x80), csrrw(0, 0x304, 5)];
    let r = run(sys(mrec(), [], concat(pre, boot_bare(0, 0)), [nop(), simexit()]));
    assert reg(r, 2) == IRQ | 7;
    assert reg(r, 1) == 0x200;
    assert (reg(r, 4) >> 11) & 3 == 0;      // MPP = U
    assert mode(r) == 3;
}

/// 委派的軟體中斷：S 模式 SIE = 0 時擱置；設 SIE 的 CSR 寫入讓 ITLB 失效，
/// 下一條的取指翻譯就是等待點 → 立即進 S handler（下一條還沒執行）
test delegated_software_interrupt {
    let pre = [csrrwi(0, 0x303, 2), csrrwi(0, 0x304, 2), csrrsi(0, 0x344, 2)];
    let r = run(sys(mrec(), srec(), concat(pre, boot_bare(MST_S, 0)), [
        addi(9, 0, 1),
        csrrsi(0, 0x100, 2),                // sstatus.SIE = 1
        addi(9, 0, 2),                      // 0x208：沒有執行
        jal(0, 0)]));
    assert mode(r) == 1;
    assert reg(r, 2) == IRQ | 1;
    assert reg(r, 1) == 0x208;
    assert reg(r, 9) == 1;
}

/// 同時擱置 SSI 與 STI（都委派）：SSI 優先
test interrupt_priority {
    let pre = concat(li(5, 0x22), [csrrw(0, 0x303, 5), csrrw(0, 0x304, 5), csrrw(0, 0x344, 5)]);
    let r = run(sys(mrec(), srec(), concat(pre, boot_bare(0, 0)), [nop(), simexit()]));
    assert reg(r, 2) == IRQ | 1;
    assert mode(r) == 1;
}

/// mtvec 向量模式：中斷跳 BASE + 4 × 編號（timer = 7 → BASE + 28）
test interrupt_vectored {
    let prog = concat([jal(0, 128)],                    // 0x00 → main @ 0x80
        concat([nop(); 15],                             // idx 1..15
        concat([addi(9, 0, 1)],                         // idx 16 = 0x40（BASE）
        concat([nop(); 6],                              // idx 17..22
        concat([csrr(2, 0x342), simexit()],             // idx 23 = 0x5C（BASE + 28）
        concat([nop(); 7],                              // idx 25..31
        concat(li(5, 0x41), [csrrw(0, 0x305, 5), csrrwi(0, 0x7C1, 0), addi(5, 0, 0x80),
                             csrrw(0, 0x304, 5), csrrsi(0, 0x300, 8), jal(0, 0)])))))));
    let r = run(prog);
    assert reg(r, 2) == IRQ | 7;
    assert reg(r, 9) == 0;
}

/// mip.MTIP 由 mtimecmp 算出、不可寫；STIP/SSIP 可由 M 寫；sip 只看得到委派的位元
test mip_writability {
    let r = run(concat(li(5, 0xA2), [csrrw(0, 0x344, 5), csrr(10, 0x344),
                                     addi(6, 0, 0x20), csrrw(0, 0x303, 6), csrr(11, 0x144), simexit()]));
    assert reg(r, 10) == 0x22;
    assert reg(r, 11) == 0x20;
}

/// 迷你作業系統的 timer 轉送：U 程式跑到 mtime = 40 → M 處理 MTI、設 mip.STIP、關掉 timer、mret
/// → 回到 U 的第一個等待點接受委派給 S 的 STI → S handler
fn timer_forwarding() {
    let mh = [addi(5, 0, 0x20), csrrs(0, 0x344, 5), addi(5, 0, -1), csrrw(0, 0x7C1, 5), mret()];
    let pre = concat(li(5, 0x20), concat([csrrw(0, 0x303, 5)],
              concat(li(5, 0xA0), concat([csrrw(0, 0x304, 5)],
              concat(li(5, 40), [csrrw(0, 0x7C1, 5)])))));
    sys(mh, srec(), concat(pre, boot_bare(0, 0)), [addi(9, 9, 1), jal(0, -4)])
}

test timer_forwarded_to_supervisor {
    let r = run(timer_forwarding());
    assert reg(r, 2) == IRQ | 5;
    assert mode(r) == 1;
    assert reg(r, 4) & 0x100 == 0;          // SPP = U
    assert reg(r, 9) > 0;
}

test interrupt_determinism {
    let p = timer_forwarding();
    let a = run_img(image(p), dimage([]));
    let b = run_img_small(image(p), dimage([]));
    assert retired(a) == retired(b);
    assert trap_pc(a) == trap_pc(b);
    for i in 0..32 { assert reg(a, i) == reg(b, i), "register differs between configurations"; }
}
