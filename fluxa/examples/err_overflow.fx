// carry 累加器太窄：i24 裝不下 8 個 i16*i16 的和 → 模擬期抓到（對應規格 E0301）
task Mac(a: in chan<i16>, out: out chan<i24>, len: u16) {
    carry acc: i24 = 0;
    for _ in 0..len { let x = a.recv(); acc += x * x; }
    out.send(acc);
}
test overflow_is_caught {
    let (ta, ra) = chan::<i16>(depth = 8);
    let (to, ro) = chan::<i24>(depth = 1);
    spawn Mac(ra, to, 8);
    for _ in 0..8 { ta.send(32767); }
    let r = ro.recv();
}
