// 寫回 FIFO 太淺 → 管線死結。Fluxa 模擬器會指出每個階段卡在哪條通道。
// 用法：fluxa rv32i.fx err_wb_depth.fx
test undersized_writeback_fifo {
    let prog = image([addi(1, 0, 1), addi(2, 0, 2), addi(3, 0, 3), addi(4, 0, 4),
                      addi(5, 0, 5), addi(6, 0, 6), addi(7, 0, 7), addi(8, 0, 8), ecall()]);
    let (io_tx, io_rx) = chan::<u32>(depth = 4);
    let (dump_tx, dump_rx) = chan::<u32>(depth = 40);
    spawn Core::<1>(prog, dimage([]), io_tx, dump_tx);
    let first = dump_rx.recv();
}
