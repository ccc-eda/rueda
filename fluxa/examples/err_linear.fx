// spawn 之後 rx 端點已被消耗，再用就是線性違規
task Sink(rx: in chan<u8>) { for _ in rx { } }
test use_after_move {
    let (tx, rx) = chan::<u8>(depth = 2);
    spawn Sink(rx);
    let v = rx.recv();
}
