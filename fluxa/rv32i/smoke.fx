test smoke_sum {
    let r = run([addi(1,0,0), addi(2,0,1), addi(3,0,11), add(1,1,2), addi(2,2,1), bne(2,3,-8), ecall()]);
    assert cause(r) == 0;
    assert trap_pc(r) == 24;
    assert reg(r, 1) == 55;
    assert retired(r) == 33;
}
