// broadcast 顯式扇出：一個生產者、兩個消費者
task Gen(n: u32, out: out chan<u8>) { for i in 0..n { out.send(i); } }
task Double(rx: in chan<u8>, out: out chan<u9>) { for v in rx { out.send(v * 2); } }
task Total(rx: in chan<u8>, out: out chan<u16>) {
    carry s: u16 = 0;
    for v in rx { s += v; }
    out.send(s);
}

test fanout {
    let (tg, rg) = chan::<u8>(depth = 2);
    let outs = broadcast(rg, 2);
    let (td, rd) = chan::<u9>(depth = 2);
    let (tt, rt) = chan::<u16>(depth = 1);
    spawn Gen(10, tg);
    spawn Double(outs[0], td);
    spawn Total(outs[1], tt);
    for i in 0..10 { assert rd.recv() == i * 2; }
    assert rt.recv() == 45;
}
