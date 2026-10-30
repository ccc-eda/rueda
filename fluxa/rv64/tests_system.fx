// v0.3 驗收：迷你作業系統——一個程式同時用到 M/S/U、Sv39、委派、timer 中斷轉送、
// 系統呼叫、F/D、A 擴充、MMIO，並在不同的寫回深度 / TLB 大小下輸出完全相同。
//
// 記憶體映射（Sv39，根頁表 PA 0x1000）：
//   VA 0x00000 頁 → PA 0（S 的 handler 程式；非 U）
//   VA 0x10000 頁 → ROM PA 0（U 程式，入口 0x10200 = ROM 0x200）
//   VA 0x20000 頁 → RAM PA 0（U 資料，[0] = 100）
//   VA 0x10000000 的 2MB 頁 → MMIO（只有 S 能寫）
// 暫存器約定：x28 = M 專用、x29..x31 = S 專用（沒有存回，U 程式不碰）；x30 = 時鐘中斷次數。

/// M：MTI → mtimecmp = mtime + 80、設 STIP 轉送給 S；S 的 ecall（SBI 風格）→ 清 STIP
fn os_m_handler() {
    [csrr(28, 0x342),
     blt(28, 0, 28),                        // 中斷 → timer
     addi(28, 0, 0x20), csrrc(0, 0x344, 28),
     csrr(28, 0x341), addi(28, 28, 4), csrrw(0, 0x341, 28), mret(),
     csrr(28, 0xC01), addi(28, 28, 80), csrrw(0, 0x7C1, 28),     // timer:
     addi(28, 0, 0x20), csrrs(0, 0x344, 28), mret()]
}

/// S（核心）：STI → tick++、請 M 清 STIP；U 的 ecall：a7 = 1 → 輸出 a0、其餘 → 結束
fn os_s_handler() {
    [csrr(31, 0x142),
     blt(31, 0, 40),                        // 中斷 → irq
     addi(29, 0, 1),
     beq(17, 29, 8),
     simexit(),                             // exit
     lui(29, 0x10000), sw(10, 29, 0),       // putint
     csrr(29, 0x141), addi(29, 29, 4), csrrw(0, 0x141, 29), sret(),
     addi(30, 30, 1), ecall(), sret()]      // irq:
}

fn os_user() {
    [fmv_d_x(1, 0),
     addi(5, 0, 1), addi(6, 0, 21),
     fcvt_d_l(2, 5), fmadd_d(1, 2, 2, 1), addi(5, 5, 1), bne(5, 6, -12),   // Σ k²（k = 1..20）
     fcvt_w_d(10, 1, 1),
     addi(17, 0, 1), ecall(),                                              // 印 2870
     lui(7, 0x20), addi(8, 0, 5), amoadd_d(9, 8, 7), ld(10, 7, 0), ecall(),  // 印 105
     addi(11, 0, 10),
     lr_d(12, 7), addi(12, 12, 1), sc_d(13, 12, 7), bne(13, 0, -12),     // LR/SC 遞增 10 次
     addi(11, 11, -1), bne(11, 0, -20),
     ld(10, 7, 0), ecall(),                                               // 印 115
     addi(11, 0, 400), addi(11, 11, -1), bne(11, 0, -4),                  // 忙碌迴圈：讓時鐘中斷發生
     addi(17, 0, 2), ecall()]                                             // exit
}

fn os_image() -> [u64; DWORDS] {
    let u = PTE_V | PTE_U | PTE_A;
    poke(l1(l0(l0(pt(RWXAD), 0x10, 0, u | PTE_R | PTE_X), 0x20, 0, u | PTE_R | PTE_W | PTE_D),
            0x80, 0x10000000, RWAD), 0, 100)
}

fn os_program() {
    // mideleg = STI、mie = MTIE | STIE、mtimecmp = 300；mstatus：FS = Initial、MPP = U
    let pre = concat(li(5, 0x20), concat([csrrw(0, 0x303, 5)],
              concat(li(5, 0xA0), concat([csrrw(0, 0x304, 5)],
              concat(li(5, 300), [csrrw(0, 0x7C1, 5)])))));
    sys(os_m_handler(), os_s_handler(), concat(pre, boot(0x2000, SV39, ROOT, 0x100, 0x10200)), os_user())
}

test mini_os {
    let r = run_img(os_program(), os_image());
    assert cause(r) == 0;
    assert output(r) == [2870, 105, 115];
    assert mode(r) == 1;                    // 在 S 核心裡結束
    assert reg(r, 17) == 2;
    assert reg(r, 30) >= 3, "timer interrupts should have preempted the user program";
    assert freg(r, 1) == 0x40A66C0000000000;   // 2870.0
}

test mini_os_determinism {
    let a = run_img(os_program(), os_image());
    let b = run_img_small(os_program(), os_image());
    assert output(a) == output(b);
    assert retired(a) == retired(b);
    assert trap_pc(a) == trap_pc(b);
    for i in 0..32 {
        assert reg(a, i) == reg(b, i), "register differs between configurations";
        assert freg(a, i) == freg(b, i), "f register differs between configurations";
    }
}
