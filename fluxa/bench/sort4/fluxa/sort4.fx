task Sort4(x: in chan<u8>, y: out chan<u8>) {
    let mut a: u8 = 0;
    let mut b: u8 = 0;
    let mut c: u8 = 0;
    let mut d: u8 = 0;
    let mut count: u8 = 0;

    for value in x {
        if count == 0 {
            a = value;
        } else if count == 1 {
            b = value;
        } else if count == 2 {
            c = value;
        } else {
            d = value;

            // Four-input sorting network: three layers of compare-and-swap.
            let mut p0 = a;
            let mut p1 = b;
            let mut p2 = c;
            let mut p3 = d;

            if p0 > p1 {
                let tmp = p0;
                p0 = p1;
                p1 = tmp;
            }
            if p2 > p3 {
                let tmp = p2;
                p2 = p3;
                p3 = tmp;
            }
            if p0 > p2 {
                let tmp = p0;
                p0 = p2;
                p2 = tmp;
            }
            if p1 > p3 {
                let tmp = p1;
                p1 = p3;
                p3 = tmp;
            }
            if p1 > p2 {
                let tmp = p1;
                p1 = p2;
                p2 = tmp;
            }

            y.send(p0);
            y.send(p1);
            y.send(p2);
            y.send(p3);
        }

        if count == 3 {
            count = 0;
        } else {
            count = (count + 1).sat::<u8>();
        }
    }

    y.close();
}

test sort4_vectors {
    let (tx, rx) = chan::<u8>(depth = 16);
    let (ty, ry) = chan::<u8>(depth = 16);
    spawn Sort4(rx, ty);

    let inputs = [9, 3, 7, 1, 0, 255, 128, 128, 5, 5, 5, 4];
    for value in inputs {
        tx.send(value);
    }
    tx.close();

    let expected = [1, 3, 7, 9, 0, 128, 128, 255, 4, 5, 5, 5];
    for value in expected {
        assert ry.recv() == value;
    }
}
