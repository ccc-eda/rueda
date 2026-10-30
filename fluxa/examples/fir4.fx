// 4 階 FIR，8 位元樣本：小到可以放進 iCE40 UP5K（一個時脈一個樣本）
const COEF: [fixed<1, 7>; 4] = [0.125, 0.375, 0.375, 0.125];

task Fir4(x: in chan<i8>, y: out chan<i8>) {
    let mut hist: [i8; 3] @reg = [0; 3];
    pipeline(ii = 1) for s in x {
        let win: [i8; 4] = concat([s], hist);
        let acc = (0..4).unrolled().map(|k| win[k] * COEF[k]).tree_sum();
        hist = win[0..3];
        y.send(acc.to::<i8>(round = nearest, ovf = sat));
    }
}

test fir4_step {
    // 步階輸入：輸出依序是係數的部分和 × 64，最後停在 64
    let (tx, rx) = chan::<i8>(depth = 8);
    let (ty, ry) = chan::<i8>(depth = 8);
    spawn Fir4(rx, ty);
    for _ in 0..6 { tx.send(64); }
    tx.close();
    let want = [8, 32, 56, 64, 64, 64];
    for k in 0..6 { assert ry.recv() == want[k]; }
}
