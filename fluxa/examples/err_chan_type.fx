// 通道的元素型別要和參數一致：i8 的通道不能接 chan<i16> 的參數。
// 行為模擬在 spawn 時就攔下；合成也拒絕（否則 FIFO 會默默截斷成 8 位元）
// 整個程式只有一處型別不符，錯誤訊息才不會隨排程順序改變。
task Wide(x: in chan<i16>, y: out chan<i16>) { for v in x { y.send(v); } }
task Narrow(x: in chan<i8>, y: out chan<i8>) { for v in x { y.send(v); } }
task Net(x: in chan<i16>, x2: in chan<i16>, y8: out chan<i8>, y16: out chan<i16>) {
    let (t, r) = chan::<i8>(depth = 2);
    spawn Wide(x, t);
    spawn Narrow(r, y8);
    spawn Wide(x2, y16);
}
test narrow_channel {
    let (tx, rx) = chan::<i16>(depth = 2);
    let (tx2, rx2) = chan::<i16>(depth = 2);
    let (t8, r8) = chan::<i8>(depth = 2);
    let (t16, r16) = chan::<i16>(depth = 2);
    spawn Net(rx, rx2, t8, t16);
    tx.send(300);
    tx.close();
    tx2.send(7);
    tx2.close();
    let v8 = r8.recv();
    let v16 = r16.recv();
}
