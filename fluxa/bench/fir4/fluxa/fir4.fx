task Fir4(x: in chan<i8>, y: out chan<i8>) {
    let mut x1: i8 @reg = 0;
    let mut x2: i8 @reg = 0;
    let mut x3: i8 @reg = 0;

    pipeline(ii = 1) for s in x {
        let acc: i16 = 3 * s + 5 * x1 + 5 * x2 + 3 * x3;
        let out: i8 = (acc >> 4).to::<i8>(round = nearest, ovf = sat);
        x3 = x2;
        x2 = x1;
        x1 = s;
        y.send(out);
    }
}

test fir4_vectors {
    let (tx, rx) = chan::<i8>(depth = 16);
    let (ty, ry) = chan::<i8>(depth = 16);
    spawn Fir4(rx, ty);

    let inputs = [16, 0, 0, 0, 0, -16, 32, 7, -128, 127, 100, -100];
    let expected = [3, 5, 5, 3, 0, -3, 1, 6, -15, -8, 19, 28];
    for k in 0..12 {
        tx.send(inputs[k]);
    }
    tx.close();
    for k in 0..12 {
        assert ry.recv() == expected[k];
    }
}
