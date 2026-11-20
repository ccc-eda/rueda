task Hist(x: in chan<u4>, y: out chan<u16>) {
    let mut counts: mem<u16, 16> = [0; 16];
    for v in x {
        counts[v] += 1;
    }
    for i in 0..16 {
        y.send(counts[i]);
    }
}

test hist_vectors {
    let (tx, rx) = chan::<u4>(depth = 8);
    let (ty, ry) = chan::<u16>(depth = 8);
    spawn Hist(rx, ty);
    let xs = [3,1,4,1,5,9,2,6,5,3,5,8,9,7,9,3,2,3,8,4,6,2,6,4,3,3,8,3,2,7,9,5,0,2,8,8,4,1,9,7,1,6,9,3,9,9,3,7,5,1,0,5,8,2,0,9,7,4,9,4,4,5,9,2,3,0,7,8,1,6,4,0,6,2,8,6,2,0,8,9,9,8,6,2,8,0,3,4,8,2,5,3,4,2,1,1,7,0,6,7,15,15,14];
    for k in 0..103 { tx.send(xs[k]); }
    tx.close();
    let want = [8, 8, 12, 12, 10, 8, 9, 8, 12, 13, 0, 0, 0, 0, 1, 2];
    for i in 0..16 { assert ry.recv() == want[i]; }
}
