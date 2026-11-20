task Fir4(x: in chan<i8>, y: out chan<i8>) {
    let mut hist: [i8; 3] @reg = [0; 3];

    pipeline(ii = 1) for s in x {
        let win: [i8; 4] = concat([s], hist);
        let acc: i32 = 3 * win[0].to::<i32>()
            + 5 * win[1].to::<i32>()
            + 5 * win[2].to::<i32>()
            + 3 * win[3].to::<i32>();
        hist = win[0..3];
        y.send((acc >> 4).to::<i8>());
    }
}

test fir4_vectors {
    let (tx, rx) = chan::<i8>(depth = 12);
    let (ty, ry) = chan::<i8>(depth = 12);
    spawn Fir4(rx, ty);

    let xs = [16, 0, 0, 0, 0, -16, 32, 7, -128, 127, 100, -100];
    let ys = [3, 5, 5, 3, 0, -3, 1, 6, -15, -8, 19, 28];

    for k in 0..12 {
        tx.send(xs[k]);
    }
    tx.close();

    for k in 0..12 {
        assert ry.recv() == ys[k];
    }
}
