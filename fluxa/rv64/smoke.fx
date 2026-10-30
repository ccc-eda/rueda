// 煙霧測試：1+2+...+10 迴圈（64 位元資料路徑的基本檢查）
// 註：ecall 現在是真陷阱（M2），結束模擬用 simexit()（存取自訂 CSR 0x7C0）

test smoke_sum {
    let r = run([
        addi(1, 0, 0), addi(2, 0, 1), addi(3, 0, 11),
        add(1, 1, 2),                       // loop:
        addi(2, 2, 1),
        bne(2, 3, -8),
        simexit()]);
    assert cause(r) == 0;
    assert trap_pc(r) == 24;
    assert reg(r, 1) == 55;
    assert retired(r) == 33;
}
