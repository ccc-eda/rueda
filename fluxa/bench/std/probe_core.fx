task Core<const N: u32, const AW: u32, const CW: u32>(
    a: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    b: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    c: out chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>
) {
    let mut aa: [int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>; N * N] = [0; N * N];
    let mut bb: [int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>; N * N] = [0; N * N];
    let mut idx: u32 = 0;
    for av in a {
        let bv = b.recv();
        aa[idx] = av;
        bb[idx] = bv;
        idx = (idx + 1).wrap::<u32>();
        if idx == N * N {
            for i in 0..N {
                for j in 0..N {
                    let mut sum: int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1> = 0;
                    serial for k in 0..N {
                        sum = (sum + (aa[i * N + k] as int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>) * (bb[k * N + j] as int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>)).wrap::<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>();
                    }
                    c.send(sum);
                }
            }
            idx = 0;
        }
    }
    c.close();
}

test probe {
    let (ta, ra) = chan::<i8>(depth = 2);
    let (tb, rb) = chan::<i8>(depth = 2);
    let (tc, rc) = chan::<i32>(depth = 2);
    spawn Core::<4, 8, 32>(ra, rb, tc);
    let av = [1, -2, 3, 4, 5, 6, -7, 8, -9, 10, 11, 12, 13, -14, 15, 127];
    let bv = [2, 0, -1, 3, 1, 4, 2, -5, -6, 7, 8, 1, 9, -10, 11, -128];
    for i in 0..16 { ta.send(av[i]); tb.send(bv[i]); }
    ta.close(); tb.close();
    let expected = [18, -27, 63, -496, 130, -105, 39, -1046, 34, -3, 249, -1602, 1065, -1221, 1476, -16132];
    for i in 0..16 { assert rc.recv() == expected[i]; }
}
