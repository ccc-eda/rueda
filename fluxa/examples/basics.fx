// 位寬、定點數、位元切片、控制流的基本行為
comb fn parity(x: u8) -> bit { x.xor_reduce() }

fn clamp_add(a: u8, b: u8) -> u8 {
    let s = a + b;                 // 推導為 0..=510，不會悄悄溢位
    s.sat::<u8>()                  // 窄化必須顯式
}

fn classify(x: int<0..=99>) -> u2 {
    match x {
        0 => 0,
        1..10 => 1,
        10..=49 => 2,
        _ => 3,
    }
}

test bits_and_slices {
    assert parity(0b1011) == 1;
    assert parity(0b1001) == 0;
    assert (0xAB)[7:4] == 0xA;
    assert (0xAB)[3:0] == 0xB;
    assert 8'hFF == 255;
    assert clamp_add(200, 100) == 255;
    assert clamp_add(20, 30) == 50;
    assert classify(0) == 0;
    assert classify(5) == 1;
    assert classify(10) == 2;
    assert classify(77) == 3;
}

test fixed_point {
    let a: fixed<4, 4> = 1.5;
    let b: fixed<4, 4> = 2.25;
    let p = a * b;                              // fixed<8,8> 的精確乘積
    assert p == 3.375;
    let s = a + b;
    assert s == 3.75;
    assert s.to::<u4>(round = nearest) == 4;
    assert s.to::<u4>(round = trunc) == 3;
    let q: fixed<2, 2> = (a * b).to::<fixed<2, 2>>(round = even, ovf = sat);
    assert q == 1.75;                           // 3.375 -> 範圍 [-2, 1.75]，飽和
}

test registers_commit_at_iteration_end {
    // @reg 變數：迭代內讀到舊值，寫入於迭代結束提交（硬體移位暫存器語意）
    let mut r: [u8; 3] @reg = [0, 0, 0];
    let mut seen: [u8; 4] = [0; 4];
    for t in 0..4 {
        seen[t] = r[2];
        r[2] = r[1];
        r[1] = r[0];
        r[0] = t + 1;
    }
    assert seen == [0, 0, 0, 1];
}

/// 大移位量：右移超過寬度 = 0（負數 = -1，即 floor 語意）；0 左移任何量 = 0
test wide_shifts {
    let x: u120 = 1 << 119;
    let n: u32 = 200;
    assert (x >> n) == 0;
    assert (x >> 119) == 1;
    let m: i32 = -5;
    assert (m >> n) == -1;
    let z: u8 = 0;
    assert (z << n) == 0;
}
