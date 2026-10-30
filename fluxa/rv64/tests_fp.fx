// M5：F/D 的管線層級測試（運算正確性由 tests_fpu.fx 的對照向量負責）

/// mstatus.FS 重置為 Off：先設成 Initial 才能用浮點
fn fp_on() { concat(li(5, 0x2000), [csrrs(0, 0x300, 5)]) }
fn prog(p: _) { concat(fp_on(), p) }
/// 64 位元常數 → f 暫存器（借用 x5、x6）
fn fli(fd: u5, hi: u32, lo: u32) { concat(li64(5, hi, lo), [fmv_d_x(fd, 5)]) }
fn fli_s(fd: u5, v: u32) { concat(li(5, v), [fmv_w_x(fd, 5)]) }

test fs_off_makes_fp_illegal {
    let r1 = run(with_handler(record_handler(), [fadd_d(1, 2, 3)]));
    assert reg(r1, 2) == 2;
    assert reg(r1, 3) == fadd_d(1, 2, 3);
    let r2 = run(with_handler(record_handler(), [flw(1, 0, 0)]));
    assert reg(r2, 2) == 2;
    let r3 = run(with_handler(record_handler(), [csrr(7, 0x001)]));
    assert reg(r3, 2) == 2;
}

test fp_load_store_and_boxing {
    let r = run_with(prog([
        fld(1, 0, 0),                       // f1 = [0]
        flw(2, 0, 8),                       // f2 = box([8] 低 32 位)
        fsd(1, 0, 16),
        fsw(2, 0, 24),                      // 只寫低 32 位
        ld(10, 0, 16),
        ld(11, 0, 24),
        fmv_x_w(12, 2),                     // 符號擴展原始低 32 位
        simexit()]), [0x400921FB54442D18, 0xAAAAAAAABF800000, 0, 0x1111111111111111]);
    assert freg(r, 1) == 0x400921FB54442D18;
    assert freg(r, 2) == 0xFFFFFFFFBF800000;
    assert reg(r, 10) == 0x400921FB54442D18;
    assert reg(r, 11) == 0x11111111BF800000;
    assert reg(r, 12) == 0xFFFFFFFFBF800000;
}

/// 單精度運算元沒有正確 NaN-boxing → 視為 canonical NaN
test improperly_boxed_single_is_nan {
    let r = run(prog(concat(fli(1, 0x3FF00000, 0), concat(fli_s(2, 0x3F800000), [
        fadd_s(3, 1, 2),                    // f1 是雙精度 1.0：上半部不是全 1
        fsgnj_s(4, 1, 1),
        simexit()]))));
    assert freg(r, 3) == 0xFFFFFFFF7FC00000;
    assert freg(r, 4) == 0xFFFFFFFF7FC00000;
}

test fp_double_arith_through_pipeline {
    // f1 = 1.0、f2 = 2.0；f3 = f1 + f2；f4 = f3 × f3 + f1（fmadd 立即用到前一條的結果）；f5 = √f4
    let r = run(prog(concat(fli(1, 0x3FF00000, 0), concat(fli(2, 0x40000000, 0), [
        fadd_d(3, 1, 2),
        fmadd_d(4, 3, 3, 1),                // 10.0
        fsqrt_d(5, 4),
        fadd_d(7, 1, 1),                    // 2.0
        fmadd_d(8, 1, 1, 7),                // rs3 = 剛算出的 f7 → 1×1 + 2 = 3.0
        fdiv_d(6, 4, 2),                    // 5.0
        flt_d(10, 6, 4),                    // 5 < 10 → 1
        fclass_d(11, 5),                    // 正規正數 → bit 6
        fcvt_w_d(12, 5, 1),                 // trunc(√10) = 3
        simexit()]))));
    assert freg(r, 3) == 0x4008000000000000;
    assert freg(r, 4) == 0x4024000000000000;
    assert freg(r, 5) == 0x40094C583ADA5B53;   // √10（RNE）
    assert freg(r, 6) == 0x4014000000000000;
    assert freg(r, 8) == 0x4008000000000000;
    assert reg(r, 10) == 1;
    assert reg(r, 11) == 0x40;
    assert reg(r, 12) == 3;
}

/// frm（動態捨入）與 fflags 累積、fcsr 視窗
test frm_and_fflags {
    let r = run(prog(concat(fli_s(1, 0x3F800000), concat(fli_s(2, 0x40400000), [
        fdiv_s(3, 1, 2),                    // 1/3，RNE → …AB，NX
        csrr(10, 0x001),                    // fflags = NX
        csrrwi(0, 0x002, 1),                // frm = RTZ
        fdiv_s(4, 1, 2),                    // RTZ → …AA
        fmv_w_x(5, 0),                      // f5 = +0
        fdiv_s(6, 1, 5),                    // 1/0 → +∞，DZ
        csrr(11, 0x003),                    // fcsr = frm<<5 | fflags = 0x20 | 9
        csrrw(12, 0x001, 0),                // 讀出後清除
        csrr(13, 0x001),
        simexit()]))));
    assert freg(r, 3) == 0xFFFFFFFF3EAAAAAB;
    assert freg(r, 4) == 0xFFFFFFFF3EAAAAAA;
    assert freg(r, 6) == 0xFFFFFFFF7F800000;
    assert reg(r, 10) == 1;
    assert reg(r, 11) == 0x29;
    assert reg(r, 12) == 9;
    assert reg(r, 13) == 0;
}

/// rd = x0 的比較指令仍把 fflags 帶回（sNaN → NV）；x0 保持 0
test fp_compare_to_x0_sets_flags {
    let r = run(prog(concat(fli_s(1, 0x7F800001), [feq_s(0, 1, 1), csrr(10, 0x001), simexit()])));
    assert reg(r, 10) == 16;
    assert reg(r, 0) == 0;
}

test reserved_rounding_mode_is_illegal {
    let bad = fpop(0x00, 1, 1, 2, 3, 5);            // 指令中的 rm = 5
    let r1 = run(with_handler(record_handler(), concat(fp_on(), [bad])));
    assert reg(r1, 2) == 2;
    assert reg(r1, 3) == bad;
    // frm = 5 時動態捨入非法；不用 rm 的指令（fsgnj）照常
    let r2 = run(with_handler(record_handler(), concat(fp_on(), [csrrwi(0, 0x002, 5), fsgnj_s(1, 2, 2), fadd_d(1, 2, 3)])));
    assert reg(r2, 2) == 2;
    assert reg(r2, 3) == fadd_d(1, 2, 3);
    // 保留的 fmt（H = 2）與 fmv 的錯誤 f3
    let r3 = run(with_handler(record_handler(), concat(fp_on(), [fpop(0x00, 2, 1, 2, 3, 0)])));
    assert reg(r3, 2) == 2;
}

/// 浮點運算把 FS 設成 Dirty，SD（bit 63）跟著讀出 1；sstatus 也看得到
test fs_dirty_and_sd {
    let r = run(prog([csrr(10, 0x300), fmv_d_x(1, 0), csrr(11, 0x300), csrr(12, 0x100), simexit()]));
    assert (reg(r, 10) >> 13) & 3 == 1;     // Initial
    assert (reg(r, 11) >> 13) & 3 == 3;     // Dirty
    assert reg(r, 11) >> 63 == 1;
    assert reg(r, 12) >> 63 == 1;
}

/// 浮點載存的例外與 Sv39：未對齊 FLD → 4、超出 RAM 的 FSW → 7
test fp_load_store_faults {
    let r1 = run(with_handler(record_handler(), concat(fp_on(), [addi(6, 0, 4), fld(1, 6, 0)])));
    assert reg(r1, 2) == 4;
    let r2 = run(with_handler(record_handler(), concat(fp_on(), concat(li(6, 0x10000), [fsw(1, 6, 0)]))));
    assert reg(r2, 2) == 7;
}

/// 浮點迴圈：Σ k（k = 1..10）以雙精度累加（fcvt.d.l + fadd.d）、結果轉單精度；
/// 預設設定與 TLB 1 / WBD 2 的結果完全相同
test fp_loop_determinism {
    let p = prog([
        fmv_d_x(1, 0),                      // f1 = 0.0
        addi(7, 0, 1), addi(8, 0, 11),
        fcvt_d_l(2, 7),                     // loop:
        fadd_d(1, 1, 2),
        addi(7, 7, 1),
        bne(7, 8, -12),
        fcvt_s_d(3, 1),
        fcvt_d_s(4, 3),
        fmv_x_d(10, 1),
        fsd(1, 0, 0), sw(10, 0, 8),
        simexit()]);
    let a = run_img(image(p), dimage([]));
    let b = run_img_small(image(p), dimage([]));
    assert freg(a, 1) == 0x404B800000000000;   // 55.0
    assert freg(a, 3) == 0xFFFFFFFF425C0000;   // 55.0f
    assert freg(a, 4) == 0x404B800000000000;
    for i in 0..32 {
        assert reg(a, i) == reg(b, i);
        assert freg(a, i) == freg(b, i);
    }
    assert retired(a) == retired(b);
}
