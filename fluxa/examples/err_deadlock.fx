// 兩個任務都先 recv 再 send，互相等待 → 死結報告
task A(i: in chan<u8>, o: out chan<u8>) { let v = i.recv(); o.send(v); }
test circular_wait {
    let (t1, r1) = chan::<u8>(depth = 1);
    let (t2, r2) = chan::<u8>(depth = 1);
    spawn A(r1, t2);
    spawn A(r2, t1);
    let (tz, rz) = chan::<u8>(depth = 1);
    let v = rz.recv();          // 測試端也在等；沒有任何人會送資料
}
