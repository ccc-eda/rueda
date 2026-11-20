// dot8: after every 8 (a, b) pairs, output the sum of the 8 products
task Dot8(a: in chan<i8>, b: in chan<i8>, y: out chan<i32>) {
    for x0 in a {
        let mut acc: i32 = x0 * b.recv();
        for _ in 0..7 {
            acc += a.recv() * b.recv();
        }
        y.send(acc);
    }
}

test dot8_vectors {
    let (ta, ra) = chan::<i8>(depth = 4);
    let (tb, rb) = chan::<i8>(depth = 4);
    let (ty, ry) = chan::<i32>(depth = 4);
    spawn Dot8(ra, rb, ty);

    let av = [1, 2, 3, 4, 5, 6, 7, 8, 127, 127, 127, 127, -128, -128, -128, -128];
    let bv = [8, 7, 6, 5, 4, 3, 2, 1, 127, 127, 127, 127, -128, -128, -128, -128];
    for k in 0..16 {
        ta.send(av[k]);
        tb.send(bv[k]);
    }
    ta.close();
    tb.close();

    assert ry.recv() == 120;
    assert ry.recv() == 130052;
}
