// +% 需要宣告了型別的變數：串流迴圈的 v 的型別是推導出來的，要寫 (v + 1).wrap::<u8>()。
// 行為模擬與合成用同一條規則
task Inc(x: in chan<u8>, y: out chan<u8>) { for v in x { y.send(v +% 1); } }
test wrap_on_inferred {
    let (tx, rx) = chan::<u8>(depth = 2);
    let (ty, ry) = chan::<u8>(depth = 2);
    spawn Inc(rx, ty);
    tx.send(255);
    tx.close();
    let v = ry.recv();
}
