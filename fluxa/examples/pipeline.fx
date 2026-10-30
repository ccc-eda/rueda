// 一個建出網路的 task：三個階段用通道串起來，中間一個 broadcast 扇出。
// 合成時（reda synth --top Pipeline）它變成一個網表：每個 spawn 一個電路，每條內部通道一個 FIFO。
task Scale(x: in chan<i8>, y: out chan<i10>, k: u2) { for v in x { y.send(v * k); } }
task Offset(x: in chan<i10>, y: out chan<i11>) { for v in x { y.send(v + 3); } }
task Count(x: in chan<i10>, n: out chan<u8>) {
    carry c: u8 = 0;
    for _ in x { c += 1; }
    n.send(c);
}

task Pipeline(x: in chan<i8>, y: out chan<i11>, n: out chan<u8>) {
    let (ta, ra) = chan::<i10>(depth = 2);
    let outs = broadcast(ra, 2);
    spawn Scale(x, ta, 3);
    spawn Offset(outs[0], y);
    spawn Count(outs[1], n);
}

test pipeline {
    let (tx, rx) = chan::<i8>(depth = 4);
    let (ty, ry) = chan::<i11>(depth = 4);
    let (tn, rn) = chan::<u8>(depth = 1);
    spawn Pipeline(rx, ty, tn);
    let xs = [5, -7, 0, 42, -128, 127];
    for v in xs { tx.send(v); }
    tx.close();
    for v in xs { assert ry.recv() == v * 3 + 3; }
    assert rn.recv() == 6;
}
