// 通道的元素型別要和參數一致：i8 的通道不能接 chan<i16> 的參數。
// 行為模擬在 spawn 時就攔下；合成也拒絕（否則 FIFO 會默默截斷成 8 位元）
task Wide(x: in chan<i16>, y: out chan<i16>) { for v in x { y.send(v); } }
task Net(x: in chan<i16>, y: out chan<i16>) {
    let (t, r) = chan::<i8>(depth = 2);
    spawn Wide(x, t);
    spawn Wide(r, y);
}
test narrow_channel {
    let (tx, rx) = chan::<i16>(depth = 2);
    let (ty, ry) = chan::<i16>(depth = 2);
    spawn Net(rx, ty);
    tx.send(300);
    tx.close();
    let v = ry.recv();
}
