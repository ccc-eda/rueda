// M3：S/U 模式 + Sv39 MMU 的系統級測試
//
// 程式布局（指令 ROM 實體位址）：
//   0x000  jal → boot
//   0x004  M 模式 handler（mtvec = 0x004）
//   0x080  S 模式 handler（stvec = 0x080）
//   0x100  boot（M 模式）：設 mtvec/stvec/medeleg/satp/mstatus/mepc，mret 進目標模式
//   0x200  payload
// 資料 RAM（實體位址，與指令 ROM 是分開的位址空間）：
//   0x0000 資料頁   0x1000 根頁表（L2）   0x2000 L1 頁表   0x3000 L0 頁表
// 預設映射：VA 0x0000 頁 → PA 0（取指時是 ROM 第 0 頁、載存時是 RAM 資料頁）

// sys / boot / mrec / srec 與 mstatus 常數在 rv64.fx 的 harness 區（各測試檔共用）

// 頁表映像 helper（pte / poke / pt / l0 / l1 / l2）在 rv64.fx 的 harness 區

// ════════════════════════════════════════════════════════════════════════════
//  特權模式與委派
// ════════════════════════════════════════════════════════════════════════════

test s_mode_ecall_traps_to_m {
    let r = run(sys(mrec(), [], boot_bare(MST_S, 0), [ecall()]));
    assert mode(r) == 3;
    assert reg(r, 2) == 9;                  // S 模式 ecall
    assert reg(r, 1) == 0x200;
    assert (reg(r, 4) >> 11) & 3 == 1;      // MPP = S
}

test u_mode_ecall_delegated_to_s {
    let r = run(sys(mrec(), srec(), boot_bare(0, 0x100), [ecall()]));
    assert mode(r) == 1;
    assert reg(r, 2) == 8;                  // U 模式 ecall
    assert reg(r, 1) == 0x200;
    assert reg(r, 4) & 0x100 == 0;          // SPP = U
}

/// U ecall 委派到 S；S handler 再 ecall（cause 9 未委派）→ M
test nested_traps_u_to_s_to_m {
    let sh = [csrr(2, 0x142), ecall()];
    let mh = [csrr(3, 0x342), csrr(4, 0x300), simexit()];
    let r = run(sys(mh, sh, boot_bare(0, 0x100), [ecall()]));
    assert reg(r, 2) == 8;
    assert reg(r, 3) == 9;
    assert (reg(r, 4) >> 11) & 3 == 1;      // 從 S 進入 M
    assert mode(r) == 3;
}

/// sret 回到 U：handler 把 sepc 推進 4
test sret_returns_to_u {
    let sh = [csrr(1, 0x141), addi(1, 1, 4), csrrw(0, 0x141, 1), sret()];
    let r = run(sys(mrec(), sh, boot_bare(0, 0x100), [ecall(), addi(9, 0, 7), simexit()]));
    assert reg(r, 9) == 7;
    assert mode(r) == 0;
}

test medeleg_warl {
    // medeleg bit 11（M 模式 ecall）不可委派
    let r = run(concat(li(5, 0xFFFF), [csrrw(0, 0x302, 5), csrr(7, 0x302), csrr(8, 0x301), simexit()]));
    assert reg(r, 7) == 0xB3FF;
    assert reg(r, 8) == 0x8000000000141129;  // misa：RV64 I M A F D S U
}

test csr_privilege_levels {
    // U 讀 sstatus、S 讀 mstatus → 非法指令
    let r1 = run(sys(mrec(), [], boot_bare(0, 0), [csrr(7, 0x100)]));
    assert reg(r1, 2) == 2;
    assert reg(r1, 3) == csrr(7, 0x100);
    let r2 = run(sys(mrec(), [], boot_bare(MST_S, 0), [csrr(7, 0x300)]));
    assert reg(r2, 2) == 2;
    // S 讀 sstatus 合法：只看得到 S 欄位（SUM）與 UXL
    let r3 = run(sys(mrec(), [], boot_bare(MST_S | SUM, 0), [csrr(7, 0x100), simexit()]));
    assert reg(r3, 7) == 0x200040000;
    assert mode(r3) == 1;
}

/// sstatus 是 mstatus 的視窗：透過 sstatus 寫不到 M 欄位
test sstatus_is_a_view {
    let r = run(sys(mrec(), [], boot_bare(MST_S, 0), [csrrsi(0, 0x100, 0xA), ecall()]));
    assert reg(r, 4) & 0xA == 2;            // SIE 設起來了；MIE（bit 3）沒有
}

test privileged_instruction_checks {
    let modes = [0, MST_S, 0, MST_S | TSR, 0, MST_S | TVM, MST_S | TVM];
    let insts = [mret(), mret(), sret(), sret(), sfence_vma(0, 0), sfence_vma(0, 0), csrr(7, 0x180)];
    for k in 0..7 {
        let r = run(sys(mrec(), [], boot_bare(modes[k], 0), [insts[k]]));
        assert reg(r, 2) == 2, "privileged instruction should be illegal";
        assert reg(r, 3) == insts[k];
    }
    // 沒有 TVM/TSR 時 S 模式可以用 satp、sfence.vma、sret（sret 回到 SPP = U）
    let r = run(sys(mrec(), [], boot(MST_S, 0, 1, 0, 0x200),
        concat([csrr(7, 0x180), sfence_vma(0, 0), wfi()],
        concat(li(5, 0x21C), [csrrw(0, 0x141, 5), sret(), simexit()]))));
    assert reg(r, 7) == 1;                  // boot 寫入的 satp（bare，PPN = 1）
    assert mode(r) == 0;
}

// ════════════════════════════════════════════════════════════════════════════
//  Sv39 翻譯
// ════════════════════════════════════════════════════════════════════════════

/// VA 0x7000 是 PA 0 的別名：經別名讀寫，經恆等映射讀回
test sv39_alias_load_store {
    let img = poke(l0(pt(RWXAD), 7, 0, RWAD), 8, 0x1234);
    let r = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x7000), [
        ld(11, 10, 8),
        addi(12, 0, 99),
        sd(12, 10, 16),
        ld(13, 0, 16),
        simexit()])), img);
    assert cause(r) == 0;
    assert mode(r) == 1;
    assert reg(r, 11) == 0x1234;
    assert reg(r, 13) == 99;
}

/// 硬體設 A/D：載入設 A、儲存設 D（L0 頁表經 VA 0x3000 映射成可讀）
test sv39_hardware_ad_bits {
    let img = l0(l0(pt(RWXAD), 1, 0, PTE_V | PTE_R | PTE_W), 3, 0x3000, PTE_V | PTE_R | PTE_A);
    let r = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x3000), concat(li(15, 0x1000), [
        ld(11, 10, 8),                      // L0[1]：還沒碰過
        ld(12, 15, 0),                      // 載入 → 設 A
        ld(13, 10, 8),
        sd(0, 15, 32),                      // 儲存 → 設 D（TLB 命中但 D = 0 → 重走）
        ld(14, 10, 8),
        simexit()]))), img);
    assert reg(r, 11) & 0xFF == 0x07;
    assert reg(r, 13) & 0xFF == 0x47;
    assert reg(r, 14) & 0xFF == 0xC7;
}

test sv39_page_faults {
    let base = l0(l0(l0(pt(RWXAD), 2, 0, PTE_V | PTE_R | PTE_A), 7, 0, RWAD), 4, 0, PTE_V | PTE_W | PTE_A | PTE_D);
    // 載入未映射頁
    let r1 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x5000), [ld(11, 10, 0)])), base);
    assert reg(r1, 2) == 13;
    assert reg(r1, 3) == 0x5000;
    assert reg(r1, 1) == 0x208;
    // 寫唯讀頁
    let r2 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x2000), [sd(0, 10, 8)])), base);
    assert reg(r2, 2) == 15;
    assert reg(r2, 3) == 0x2008;
    // 從不可執行頁取指：mepc = mtval = 目標 VA
    let r3 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x7200), [jalr(0, 10, 0)])), base);
    assert reg(r3, 2) == 12;
    assert reg(r3, 1) == 0x7200;
    assert reg(r3, 3) == 0x7200;
    // 中間層指標缺（L1[1] = 0）
    let r4 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x200000), [ld(11, 10, 0)])), base);
    assert reg(r4, 2) == 13;
    // W 無 R 是保留編碼
    let r5 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x4000), [ld(11, 10, 0)])), base);
    assert reg(r5, 2) == 13;
    // 保留位元 [63:54] ≠ 0
    let bad = poke(base, 0x3000 + 8 * 7, pte(0, RWAD) | (1 << 60));
    let r6 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x7000), [ld(11, 10, 0)])), bad);
    assert reg(r6, 2) == 13;
    // 非正規位址：bit 38 = 1 但 bits 63:39 = 0
    let r7 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li64(10, 0x40, 0), [ld(12, 10, 0)])), base);
    assert reg(r7, 2) == 13;
    assert reg(r7, 3) == 0x4000000000;
}

/// 頁錯誤委派到 S（medeleg bit 13）
test sv39_page_fault_delegated {
    let r = run_img(sys(mrec(), srec(), boot_vm(MST_S, 0x2000), concat(li(10, 0x5000), [ld(11, 10, 0)])), pt(RWXAD));
    assert mode(r) == 1;
    assert reg(r, 2) == 13;
    assert reg(r, 3) == 0x5000;
    assert reg(r, 4) & 0x100 == 0x100;      // SPP = S
}

/// 走訪讀到實體記憶體外（satp.PPN 指向 PA 0x100000）→ 取指存取錯誤
test sv39_walk_access_fault {
    let r = run_img(sys(mrec(), [], boot(MST_S, SV39, 0x100, 0, 0x200), [nop()]), pt(RWXAD));
    assert reg(r, 2) == 1;
    assert reg(r, 1) == 0x200;
    assert reg(r, 3) == 0x200;
}

test sv39_user_bit {
    let ucode = RWXAD | PTE_U;
    // U 模式從非 U 頁取指
    let r1 = run_img(sys(mrec(), [], boot_vm(0, 0), [nop()]), pt(RWXAD));
    assert reg(r1, 2) == 12;
    assert reg(r1, 1) == 0x200;
    // U 模式載入非 U 頁
    let r2 = run_img(sys(mrec(), [], boot_vm(0, 0), concat(li(10, 0x7000), [ld(11, 10, 0)])),
                     l0(pt(ucode), 7, 0, RWAD));
    assert reg(r2, 2) == 13;
    // S 模式載入 U 頁：SUM = 0 → 錯誤；SUM = 1 → 可以
    let img = l0(pt(RWXAD), 7, 0, RWAD | PTE_U);
    let prog = concat(li(10, 0x7000), [ld(11, 10, 8), simexit()]);
    let r3 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), prog), img);
    assert reg(r3, 2) == 13;
    let r4 = run_img(sys(mrec(), [], boot_vm(MST_S | SUM, 0), prog), poke(img, 8, 77));
    assert cause(r4) == 0;
    assert reg(r4, 11) == 77;
    // S 模式從 U 頁取指：即使 SUM = 1 也不行
    let r5 = run_img(sys(mrec(), [], boot_vm(MST_S | SUM, 0), [nop()]), pt(ucode));
    assert reg(r5, 2) == 12;
}

/// MXR：僅可執行頁在 MXR = 1 時可讀
test sv39_mxr {
    let img = poke(l0(pt(RWXAD), 7, 0, PTE_V | PTE_X | PTE_A), 8, 55);
    let prog = concat(li(10, 0x7000), [ld(11, 10, 8), simexit()]);
    let r1 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), prog), img);
    assert reg(r1, 2) == 13;
    let r2 = run_img(sys(mrec(), [], boot_vm(MST_S | MXR, 0), prog), img);
    assert reg(r2, 11) == 55;
}

/// 2MB 與 1GB 巨頁、高半部位址、未對齊巨頁
test sv39_superpages {
    let img = poke(l2(l2(l1(l1(pt(RWXAD), 1, 0, RWAD), 2, 0x1000, PTE_V | PTE_R | PTE_A),
                         1, 0, RWAD), 256, 0, RWAD), 8, 0xABC);
    let r = run_img(sys(mrec(), [], boot_vm(MST_S, 0),
        concat(li(10, 0x200000), concat([ld(11, 10, 8)],                 // 2MB 頁：L1[1]
        concat(li(10, 0x40000000), concat([ld(12, 10, 8)],               // 1GB 頁：root[1]
        concat(li64(16, 0xFFFFFFC0, 0), concat([ld(13, 16, 8)],          // 高半部：root[256]
        concat(li(10, 0x400000), [ld(14, 10, 0)])))))))), img);            // L1[2] 的 PPN 未對齊 2MB
    assert reg(r, 11) == 0xABC;
    assert reg(r, 12) == 0xABC;
    assert reg(r, 13) == 0xABC;
    assert reg(r, 2) == 13;                 // 最後一個載入：未對齊巨頁 → 頁錯誤
    assert reg(r, 3) == 0x400000;
}

/// 程式在虛擬位址執行：U 模式 code 位於 1GB 頁 VA 0x40000000 起（→ ROM PA 0）
test sv39_code_at_virtual_address {
    let img = l2(pt(RWXAD), 1, 0, RWXAD | PTE_U);
    let sh = [csrr(1, 0x141), csrr(2, 0x142), simexit()];
    let r = run_img(sys(mrec(), sh, boot(0, SV39, ROOT, 0x100, 0x40000200), [auipc(9, 0), ecall()]), img);
    assert reg(r, 9) == 0x40000200;
    assert reg(r, 1) == 0x40000204;         // sepc 是虛擬位址
    assert reg(r, 2) == 8;
}

/// U 模式經 2MB 映射寫 MMIO
test sv39_mmio_from_user {
    let img = l1(pt(RWXAD | PTE_U), 0x80, 0x10000000, RWAD | PTE_U);
    let r = run_img(sys(mrec(), [], boot_vm(0, 0), [
        lui(10, 0x10000), addi(11, 0, 42), sw(11, 10, 0), simexit()]), img);
    assert cause(r) == 0;
    assert mode(r) == 0;
    assert output(r) == [42];
}

/// MPRV：M 模式載存以 MPP 的模式翻譯（取指不受影響）
test sv39_mprv {
    let img = poke(l0(pt(RWXAD), 7, 0x1000, RWAD), 0x1800, 31);
    let r = run_img(sys(mrec(), [], boot_vm(MST_M, 0),
        concat(li(5, MPRV | MST_S), concat([csrrw(0, 0x300, 5)],
        concat(li(10, 0x7800), [ld(11, 10, 0), simexit()])))), img);
    assert mode(r) == 3;
    assert reg(r, 11) == 31;
}

/// SFENCE.VMA：改 PTE 後 fence，新映射才保證生效（全部清與指定位址兩種形式）
test sv39_sfence_vma {
    // VA 0x7000 → PA 0（[0x800] = 111），改成 → PA 0x1000（[0x1800] = 222）
    let img = poke(poke(l0(l0(pt(RWXAD), 3, 0x3000, RWAD), 7, 0, RWAD), 0x800, 111), 0x1800, 222);
    let newpte = pte(0x1000, RWAD);
    let body = concat(li(10, 0x7800), concat([ld(11, 10, 0)],
               concat(li(12, 0x3000), concat(li64(13, 0, newpte), [sd(13, 12, 56)]))));
    let r1 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(body, [sfence_vma(0, 0), ld(14, 10, 0), simexit()])), img);
    assert reg(r1, 11) == 111;
    assert reg(r1, 14) == 222;
    let r2 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(body, [sfence_vma(10, 0), ld(14, 10, 0), simexit()])), img);
    assert reg(r2, 14) == 222;
    // ASID 過濾：satp.ASID = 5，sfence.vma x0, x(=5) 清掉該 ASID 的項目
    let r3 = run_img(sys(mrec(), [], boot(MST_S, SV39 | 0x5000, ROOT, 0, 0x200),
        concat(body, concat([addi(15, 0, 5)], [sfence_vma(0, 15), ld(14, 10, 0), simexit()]))), img);
    assert reg(r3, 14) == 222;
}

/// TLB 項目以 ASID 標記：換到從未用過的 ASID 2 後必須重新走訪，看到已修改的 PTE
/// （本模型的性質：只有程式用過的 ASID 才可能留下舊項目）
test sv39_tlb_asid_tag {
    let img = poke(poke(l0(l0(pt(RWXAD), 3, 0x3000, RWAD), 7, 0, RWAD), 0x800, 111), 0x1800, 222);
    let newpte = pte(0x1000, RWAD);
    let r = run_img(sys(mrec(), [], boot(MST_S, SV39 | 0x1000, ROOT, 0, 0x200),
        concat(li(10, 0x7800), concat([ld(11, 10, 0)],                     // ASID 1：快取 → PA 0
        concat(li(12, 0x3000), concat(li64(13, 0, newpte), concat([sd(13, 12, 56)],   // 改 PTE（不 fence）
        concat(li64(5, SV39 | 0x2000, ROOT), [csrrw(0, 0x180, 5), ld(14, 10, 0), simexit()]))))))), img);
    assert reg(r, 11) == 111;
    assert reg(r, 14) == 222;
}

/// satp 寫入不支援的 MODE：整筆寫入無效
test satp_warl {
    let r = run(concat(li64(5, 0x90000000, 7), [csrrw(0, 0x180, 5), csrr(7, 0x180),
                                               addi(5, 0, 3), csrrw(0, 0x180, 5), csrr(8, 0x180), simexit()]));
    assert reg(r, 7) == 0;
    assert reg(r, 8) == 3;
}

// ════════════════════════════════════════════════════════════════════════════
//  確定性：TLB 1 項 + 寫回深度 2 與預設設定（TLB 8、WBD 8）的輸出完全相同
// ════════════════════════════════════════════════════════════════════════════

/// U 模式在虛擬位址 0x7000（→ PA 0）上做泡沫排序、經 MMIO 輸出；跨 4 個頁面存取
test sv39_determinism_tlb_and_depth {
    let ucode = RWXAD | PTE_U;
    let img = l1(l0(l0(l0(poke(poke(poke(poke(pt(ucode), 0, 5), 8, 0xFFFFFFFFFFFFFFFD), 16, 9), 24, 1),
                     7, 0, RWAD | PTE_U), 8, 0x1000, PTE_V | PTE_R | PTE_U), 9, 0x2000, PTE_V | PTE_R | PTE_U),
                 0x80, 0x10000000, RWAD | PTE_U);
    let prog = sys(mrec(), [], boot_vm(0, 0), concat(li(20, 0x7000), [
        addi(2, 0, 4),                      // n
        addi(3, 2, -1),                     // i = n - 1
        addi(4, 0, 0),                      // outer: j = 0
        slli(5, 4, 3),                      // inner
        add(5, 5, 20),
        ld(6, 5, 0),
        ld(7, 5, 8),
        bge(7, 6, 12),
        sd(7, 5, 0),
        sd(6, 5, 8),
        addi(4, 4, 1),
        blt(4, 3, -32),
        addi(3, 3, -1),
        blt(0, 3, -44),
        lui(10, 0x10000),
        addi(4, 0, 0),
        slli(5, 4, 3),                      // out:
        add(5, 5, 20),
        ld(6, 5, 0),
        sw(6, 10, 0),
        addi(4, 4, 1),
        blt(4, 2, -20),
        lui(21, 0x8),                       // 讀頁 0x8000、0x9000（多走幾次頁表，壓 TLB）
        ld(21, 21, 0),
        lui(22, 0x9),
        ld(22, 22, 0),
        simexit()]));
    let a = run_img(prog, img);
    let b = run_img_small(prog, img);
    assert cause(a) == 0;
    assert output(a) == [0xFFFFFFFD, 1, 5, 9];
    assert output(a) == output(b);
    assert cause(a) == cause(b);
    assert trap_pc(a) == trap_pc(b);
    assert retired(a) == retired(b);
    assert mode(a) == mode(b);
    for i in 0..32 { assert reg(a, i) == reg(b, i), "register differs between configurations"; }
}

/// AMO 經 Sv39：唯讀頁 → 儲存頁錯誤（15）、未映射 LR → 載入頁錯誤（13）；AMO 設 D
test sv39_amo {
    let img = poke(l0(l0(l0(pt(RWXAD), 2, 0, PTE_V | PTE_R | PTE_A), 3, 0x3000, PTE_V | PTE_R | PTE_A),
                      7, 0, PTE_V | PTE_R | PTE_W | PTE_A), 8, 40);
    let r1 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x2008), [amoadd_d(11, 0, 10)])), img);
    assert reg(r1, 2) == 15;
    let r2 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x5000), [lr_d(11, 10)])), img);
    assert reg(r2, 2) == 13;
    let r3 = run_img(sys(mrec(), [], boot_vm(MST_S, 0), concat(li(10, 0x7008), concat(li(12, 0x3000), [
        addi(13, 0, 2), amoadd_d(11, 13, 10), ld(14, 0, 8), ld(15, 12, 56), simexit()]))), img);
    assert reg(r3, 11) == 40;
    assert reg(r3, 14) == 42;
    assert reg(r3, 15) & 0xFF == 0xC7;     // AMO 是儲存：設 A 與 D
}
