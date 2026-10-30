// 乘加管線：累加器位寬由 carry 宣告決定，溢位會在模擬時被抓到
task Mac(a: in chan<i16>, b: in chan<i16>, out: out chan<i48>, len: u16) {
    carry acc: i48 = 0;
    pipeline(ii = 1) for _ in 0..len {
        acc += a.recv() * b.recv();
    }
    out.send(acc);
}

test mac_dot_product {
    let (ta, ra) = chan::<i16>(depth = 4);
    let (tb, rb) = chan::<i16>(depth = 4);
    let (to, ro) = chan::<i48>(depth = 1);
    spawn Mac(ra, rb, to, 8);

    let xs = [3, -4, 5, 32767, -32768, 7, 0, 100];
    let ys = [2, 9, -1, 32767, -32768, 7, 5, -100];
    let mut expect: i48 = 0;
    for i in 0..8 {
        ta.send(xs[i]);
        tb.send(ys[i]);
        expect += xs[i] * ys[i];
    }
    assert ro.recv() == expect;
}
