task Mm4Top(a: in chan<i8>, b: in chan<i8>, c: out chan<i32>) {
    spawn SystolicMatmul::<4, 8, 32>(a, b, c);
}

task Mm2Top(a: in chan<i8>, b: in chan<i8>, c: out chan<i32>) {
    spawn SystolicMatmul::<2, 8, 32>(a, b, c);
}

task Blur(
    windows: in chan<[[u8; 3]; 3]>,
    y: out chan<u8>
) {
    for w in windows {
        let sum: u16 =
            w[0][0] + 2 * w[0][1] + w[0][2] +
            2 * (w[1][0] + 2 * w[1][1] + w[1][2]) +
            w[2][0] + 2 * w[2][1] + w[2][2];
        y.send((sum >> 4).to::<u8>(round = trunc, ovf = sat));
    }
    y.close();
}

task Win8x6Top(x: in chan<u8>, y: out chan<u8>) {
    let (windows_tx, windows_rx) = chan::<[[u8; 3]; 3]>(depth = 2);
    spawn Window3x3::<8, 6, 8>(x, windows_tx);
    spawn Blur(windows_rx, y);
}

test mm4_vector {
    let (ta, ra) = chan::<i8>(depth = 2);
    let (tb, rb) = chan::<i8>(depth = 2);
    let (tc, rc) = chan::<i32>(depth = 2);
    spawn Mm4Top(ra, rb, tc);

    let av = [1, -2, 3, 4, 5, 6, -7, 8, -9, 10, 11, 12, 13, -14, 15, 127];
    let bv = [2, 0, -1, 3, 1, 4, 2, -5, -6, 7, 8, 1, 9, -10, 11, -128];
    for i in 0..16 {
        ta.send(av[i]);
        tb.send(bv[i]);
    }
    ta.close();
    tb.close();

    let expected = [
        18, -27, 63, -496,
        130, -105, 39, -1046,
        34, -3, 249, -1602,
        1065, -1221, 1476, -16132
    ];
    for i in 0..16 {
        assert rc.recv() == expected[i];
    }
}

test mm2_two_pairs {
    let (ta, ra) = chan::<i8>(depth = 2);
    let (tb, rb) = chan::<i8>(depth = 2);
    let (tc, rc) = chan::<i32>(depth = 2);
    spawn Mm2Top(ra, rb, tc);

    let av = [1, 2, 3, 4, -1, 0, 2, -3];
    let bv = [5, 6, 7, 8, 4, -5, -6, 7];
    for i in 0..8 {
        ta.send(av[i]);
        tb.send(bv[i]);
    }
    ta.close();
    tb.close();

    let expected = [19, 22, 43, 50, -4, 5, 26, -31];
    for i in 0..8 {
        assert rc.recv() == expected[i];
    }
}

test win8x6_vector {
    let (tx, rx) = chan::<u8>(depth = 2);
    let (ty, ry) = chan::<u8>(depth = 32);
    spawn Win8x6Top(rx, ty);

    let input = [
        0, 11, 22, 33, 44, 55, 66, 77,
        37, 67, 97, 127, 157, 187, 217, 114,
        74, 123, 172, 221, 137, 186, 235, 151,
        111, 179, 247, 182, 250, 185, 253, 188,
        148, 235, 189, 20, 230, 61, 15, 225,
        185, 35, 8, 237, 87, 60, 33, 6
    ];
    for i in 0..48 {
        tx.send(input[i]);
    }
    tx.close();

    let expected = [
        67, 97, 118, 132, 153, 158,
        123, 163, 179, 178, 194, 201,
        170, 189, 183, 184, 178, 180,
        162, 150, 146, 151, 115, 102
    ];
    for i in 0..24 {
        assert ry.recv() == expected[i];
    }
}
