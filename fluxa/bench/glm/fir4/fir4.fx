// fir4: 4-tap FIR, y[n] = (3*x[n] + 5*x[n-1] + 5*x[n-2] + 3*x[n-3]) >> 4
task Fir4(x: in chan<i8>, y: out chan<i8>) {
    let mut hist: [i8; 3] @reg = [0; 3];
    pipeline(ii = 1) for s in x {
        let win: [i8; 4] = concat([s], hist);
        let acc = 3 * win[0] + 5 * win[1] + 5 * win[2] + 3 * win[3];
        y.send(acc >> 4);
        hist = win[0..3];
    }
}

test fir4_vectors {
    let (tx, rx) = chan::<i8>(depth = 8);
    let (ty, ry) = chan::<i8>(depth = 8);
    spawn Fir4(rx, ty);
    let xs: [i8; 12] = [16, 0, 0, 0, 0, -16, 32, 7, -128, 127, 100, -100];
    let ys: [i8; 12] = [3, 5, 5, 3, 0, -3, 1, 6, -15, -8, 19, 28];
    for k in 0..12 { tx.send(xs[k]); }
    tx.close();
    for k in 0..12 { assert ry.recv() == ys[k]; }
}
