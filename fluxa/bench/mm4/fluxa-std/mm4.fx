// 4x4 output-stationary systolic matrix multiplier.
//
// The standard-library generator instantiates sixteen processing elements,
// routes A east, routes B south, and returns C in row-major order.
task Mm4(a: in chan<i8>, b: in chan<i8>, c: out chan<i32>) {
    spawn std::SystolicMatmul::<4, 8, 32>(a, b, c);
}

test mm4_vectors {
    let (ta, ra) = chan::<i8>(depth = 2);
    let (tb, rb) = chan::<i8>(depth = 2);
    let (tc, rc) = chan::<i32>(depth = 2);
    spawn Mm4(ra, rb, tc);

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
