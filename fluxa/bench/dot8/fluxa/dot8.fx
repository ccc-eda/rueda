task Dot8Pairs(a: in chan<i8>, b: in chan<i8>, products: out chan<i32>) {
    pipeline(ii = 1) for av in a {
        let bv = b.recv();
        let product: i32 = av * bv;
        products.send(product);
    }
}

task Dot8Reduce(products: in chan<i32>, y: out chan<i32>) {
    let mut acc: i32 = 0;
    let mut count: u8 = 0;
    for product in products {
        acc = (acc + product).sat::<i32>();
        count = (count + 1).sat::<u8>();
        if count == 8 {
            y.send(acc);
            acc = 0;
            count = 0;
        }
    }
}

task Dot8(a: in chan<i8>, b: in chan<i8>, y: out chan<i32>) {
    let (tp, rp) = chan::<i32>(depth = 16);
    spawn Dot8Pairs(a, b, tp);
    spawn Dot8Reduce(rp, y);
}

test dot8_vectors {
    let (ta, ra) = chan::<i8>(depth = 16);
    let (tb, rb) = chan::<i8>(depth = 16);
    let (ty, ry) = chan::<i32>(depth = 2);
    spawn Dot8(ra, rb, ty);

    let av = [1, 2, 3, 4, 5, 6, 7, 8, 127, 127, 127, 127, -128, -128, -128, -128];
    let bv = [8, 7, 6, 5, 4, 3, 2, 1, 127, 127, 127, 127, -128, -128, -128, -128];
    for i in 0..16 {
        ta.send(av[i]);
        tb.send(bv[i]);
    }
    ta.close();
    tb.close();

    let expected = [120, 130052];
    for i in 0..2 {
        assert ry.recv() == expected[i];
    }
}
