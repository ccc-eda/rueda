// ════════════════════════════════════════════════════════════════════════════
//  Tests of the Fluxa Rv64Soc (soc.fx) on the Fluxa simulator: small programs (written with
//  rv64.fx's assembler) in a small RAM; each tells its results on the UART and ends at the
//  finisher. Load with rv64.fx and fpu.fx (the assembler):
//
//    reda sim fluxa/rv64/rv64.fx fluxa/rv64/fpu.fx fluxa/rv64/linux/soc.fx fluxa/rv64/linux/tests_soc.fx
//
//  Linux on the same SoC: fluxa/rv64/linux/test.sh (fluxa-emu, riscv-emu's commit log).
// ════════════════════════════════════════════════════════════════════════════

const SOC_RAMW: u32 = 4096;               // 32 KB at 0x8000_0000

/// the outside of a test: nothing comes in; mtime counts instructions (a wait jumps to the timer)
task SocEnv(req: in chan<(bool, u64)>, resp: out chan<(bool, u8, u16, u64)>) {
    for (wait, until) in req { resp.send((false, 0, 0, if wait && until != H_MAX { until } else { 0 })); }
}

/// instructions (32-bit words) as the RAM's 64-bit words
fn soc_words(p: _) -> _ {
    let q = concat(p, [0]);
    array((len(p) + 1) / 2, |k| q[2 * k] + (q[2 * k + 1] << 32))
}

/// run program `p` at 0x8000_0000 until it writes the finisher: (the finisher's code, the first
/// `ntx` UART bytes, the first `ngpio` GPIO changes (outputs, enables))
fn soc_run(p: _, ntx: u32, ngpio: u32) {
    let (lt, lr) = chan::<(u32, u64)>(depth = 4);
    let (eqt, eqr) = chan::<(bool, u64)>(depth = 1);
    let (et, er) = chan::<(bool, u8, u16, u64)>(depth = 1);
    let (tt, tr) = chan::<u8>(depth = 256);
    let (gt, gr) = chan::<(u16, u16)>(depth = 64);
    let (ft, fr) = chan::<u32>(depth = 1);
    let (xt, xr) = chan::<(u2, u64, u32, bool, u5, u64)>(depth = 1);
    spawn Soc::<SOC_RAMW, false>(lr, eqt, er, tt, gt, ft, xt);
    spawn SocEnv(eqr, et);
    let w = soc_words(p);
    for k in 0..len(w) { lt.send((k, w[k])); }
    lt.close();
    let code = fr.recv();
    let mut out = [];
    for _ in 0..ntx { out = concat(out, [tr.recv()]); }
    let mut g = [];
    for _ in 0..ngpio { g = concat(g, [gr.recv()]); }
    (code, out, g)
}

/// the low byte of rs to the UART (x30 is the address)
fn putc(rs: u5) -> [u32; 2] { [lui(30, 0x10000), sb(rs, 30, 0)] }
/// the finisher: pass (0x5555), then wait there
fn soc_exit() -> _ { concat(li(31, 0x5555), [lui(30, 0x100), sw(31, 30, 0), jal(0, 0)]) }
/// a program, then the exit
fn prog(p: _) -> _ { concat(p, soc_exit()) }

test uart_and_finisher {
    let r = soc_run(prog(concat(concat(li(5, 72), putc(5)), concat(li(5, 105), concat(putc(5),
        // the line status register: the transmitter always empty, nothing received
        [lui(30, 0x10000), lbu(6, 30, 5), sb(6, 30, 0)])))), 3, 0);
    assert r.0 == 0x5555;
    assert r.1 == [72, 105, 0x60];
}

test alu_and_m {
    let p = concat(li(5, 7), concat(li(6, 6), concat([mul(7, 5, 6)], concat(putc(7),       // 42
            concat(li(8, 100), concat([div(9, 8, 5)], concat(putc(9),                      // 14
            concat([remu(10, 8, 5)], concat(putc(10),                                       // 2
            concat([addiw(11, 0, -3), srli(11, 11, 56)], putc(11)))))))))));                // 0xff
    let r = soc_run(prog(p), 4, 0);
    assert r.1 == [42, 14, 2, 0xFF];
}

test loads_and_stores {
    // 0x1234_5678_9abc_def0 at 0x8000_4000; bytes and halves back, signed and unsigned
    let p = concat(li64(5, 0x12345678, 0x9ABCDEF0), concat([lui(6, 0x80004), slli(6, 6, 32), srli(6, 6, 32), sd(5, 6, 0),
            lbu(7, 6, 1), lh(8, 6, 6), lb(9, 6, 0), srai(9, 9, 8), ld(10, 6, 0), srli(10, 10, 32)],
            concat(putc(7), concat(putc(8), concat(putc(9), putc(10))))));
    let r = soc_run(prog(p), 4, 0);
    // 0xde, 0x34 (the low byte of 0x1234), 0xff (0xf0 sign-extended, shifted), 0x78
    assert r.1 == [0xDE, 0x34, 0xFF, 0x78];
}

test compressed_instructions {
    // c.li x5, 9 then c.addi x5, 3 in one 32-bit word
    let r = soc_run(prog(concat([0x028D42A5], putc(5))), 1, 0);
    assert r.1 == [12];
}

test ecall_trap_and_mret {
    // [0] jal main; [1..] the handler: the cause to the UART, mepc + 4, mret
    let handler = concat([csrr(5, 0x342)], concat(putc(5), [csrr(6, 0x341), addi(6, 6, 4), csrrw(0, 0x341, 6), mret()]));
    let main = concat([auipc(7, 0), addi(7, 7, -4 * len(handler)), csrrw(0, 0x305, 7)],
        concat([ecall()], concat(li(5, 82), putc(5))));
    let p = concat([jal(0, 4 * (len(handler) + 1))], concat(handler, main));
    let r = soc_run(prog(p), 2, 0);
    // the cause of an ecall from M mode (11), then the instruction after it ran ('R')
    assert r.1 == [11, 82];
}

test illegal_instruction {
    let handler = concat([csrr(5, 0x342)], concat(putc(5), concat([csrr(5, 0x343)], concat(putc(5), soc_exit()))));
    let main = [auipc(7, 0), addi(7, 7, -4 * len(handler)), csrrw(0, 0x305, 7), 0xFFFFFFFF];
    let p = concat([jal(0, 4 * (len(handler) + 1))], concat(handler, main));
    let r = soc_run(p, 2, 0);
    // illegal instruction (2), mtval = the instruction (its low byte)
    assert r.1 == [2, 0xFF];
}

test timer_interrupt_wakes_wfi {
    // mtimecmp (CLINT 0x200_4000) = 300, MTIE, MIE, wfi: the timer's interrupt ends the wait
    let handler = concat([csrr(5, 0x342)], concat(putc(5), soc_exit()));
    let main = concat([auipc(7, 0), addi(7, 7, -4 * len(handler)), csrrw(0, 0x305, 7)],
        concat(li(5, 300), [lui(6, 0x2004), sd(5, 6, 0), addi(5, 0, 0x80), csrrw(0, 0x304, 5), csrrsi(0, 0x300, 8), wfi(), jal(0, 0)]));
    let p = concat([jal(0, 4 * (len(handler) + 1))], concat(handler, main));
    let r = soc_run(p, 1, 0);
    // mcause = interrupt | 7: its low byte
    assert r.1 == [7];
}

test gpio_outputs {
    // output enable 0b11, then the outputs 0b10 (GPIO at 0x1006_0000: output_en at 8, output_val at 12)
    let p = concat(li(6, 0x10060000), [addi(5, 0, 3), sw(5, 6, 8), addi(5, 0, 2), sw(5, 6, 12)]);
    let r = soc_run(prog(p), 0, 2);
    assert r.2 == [(0, 3), (2, 3)];
}
