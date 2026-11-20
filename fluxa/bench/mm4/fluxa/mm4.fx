// One processing element.  A travels east and B travels south; the two
// streams retain their row/column order, so the kth value on each input is a
// pair of A[i][k] and B[k][j].
task Pe(
    ain: in chan<i8>,
    bin: in chan<i8>,
    east: out chan<i8>,
    south: out chan<i8>,
    result: out chan<i32>
) {
    let mut acc: i32 = 0;
    let mut count: u8 = 0;

    for av in ain {
        let bv = bin.recv();
        let product: i32 = (av as i32) * (bv as i32);
        acc = acc + product;

        east.send(av);
        south.send(bv);

        count = count + 1;
        if count == 4 {
            result.send(acc);
            acc = 0;
            count = 0;
        }
    }

    east.close();
    south.close();
    result.close();
}

// A row of B is received row-major and distributed to the four columns.
task BRow(
    row: in chan<i8>,
    c0: out chan<i8>,
    c1: out chan<i8>,
    c2: out chan<i8>,
    c3: out chan<i8>
) {
    for v0 in row {
        let v1 = row.recv();
        let v2 = row.recv();
        let v3 = row.recv();
        c0.send(v0);
        c1.send(v1);
        c2.send(v2);
        c3.send(v3);
    }
    c0.close();
    c1.close();
    c2.close();
    c3.close();
}

// Gather one value from every B row, making a column-major B stream.
task BColumn(
    r0: in chan<i8>,
    r1: in chan<i8>,
    r2: in chan<i8>,
    r3: in chan<i8>,
    column: out chan<i8>
) {
    for v0 in r0 {
        let v1 = r1.recv();
        let v2 = r2.recv();
        let v3 = r3.recv();
        column.send(v0);
        column.send(v1);
        column.send(v2);
        column.send(v3);
    }
    column.close();
}

// Feed the west edge of the array with A rows and the B-row network with
// the row-major B input.
task Feeder(
    a: in chan<i8>,
    b: in chan<i8>,
    a0_tx: out chan<i8>, a1_tx: out chan<i8>,
    a2_tx: out chan<i8>, a3_tx: out chan<i8>,
    br0_tx: out chan<i8>, br1_tx: out chan<i8>,
    br2_tx: out chan<i8>, br3_tx: out chan<i8>
) {
    let mut arow: u8 = 0;
    let mut acol: u8 = 0;
    let mut brow: u8 = 0;
    let mut bcol: u8 = 0;
    for av in a {
        let bv = b.recv();

        if arow == 0 { a0_tx.send(av); }
        else if arow == 1 { a1_tx.send(av); }
        else if arow == 2 { a2_tx.send(av); }
        else { a3_tx.send(av); }

        if brow == 0 { br0_tx.send(bv); }
        else if brow == 1 { br1_tx.send(bv); }
        else if brow == 2 { br2_tx.send(bv); }
        else { br3_tx.send(bv); }

        acol = acol + 1;
        if acol == 4 { acol = 0; arow = arow + 1; }
        bcol = bcol + 1;
        if bcol == 4 { bcol = 0; brow = brow + 1; }
    }

    a0_tx.close(); a1_tx.close(); a2_tx.close(); a3_tx.close();
    br0_tx.close(); br1_tx.close(); br2_tx.close(); br3_tx.close();
}

// Consume values which leave the east and south edges of the array.
task Drain(x: in chan<i8>) {
    for _value in x {
    }
}

// Read the sixteen PE results in row-major order.
task Collect(
    p00: in chan<i32>, p01: in chan<i32>, p02: in chan<i32>, p03: in chan<i32>,
    p10: in chan<i32>, p11: in chan<i32>, p12: in chan<i32>, p13: in chan<i32>,
    p20: in chan<i32>, p21: in chan<i32>, p22: in chan<i32>, p23: in chan<i32>,
    p30: in chan<i32>, p31: in chan<i32>, p32: in chan<i32>, p33: in chan<i32>,
    c: out chan<i32>
) {
    for v00 in p00 {
        let v01 = p01.recv();
        let v02 = p02.recv();
        let v03 = p03.recv();
        let v10 = p10.recv();
        let v11 = p11.recv();
        let v12 = p12.recv();
        let v13 = p13.recv();
        let v20 = p20.recv();
        let v21 = p21.recv();
        let v22 = p22.recv();
        let v23 = p23.recv();
        let v30 = p30.recv();
        let v31 = p31.recv();
        let v32 = p32.recv();
        let v33 = p33.recv();

        c.send(v00); c.send(v01); c.send(v02); c.send(v03);
        c.send(v10); c.send(v11); c.send(v12); c.send(v13);
        c.send(v20); c.send(v21); c.send(v22); c.send(v23);
        c.send(v30); c.send(v31); c.send(v32); c.send(v33);
    }
    c.close();
}

task Mm4(a: in chan<i8>, b: in chan<i8>, c: out chan<i32>) {
    // West-edge A inputs (one per row).
    let (a0_tx, a0_rx) = chan::<i8>(depth = 16);
    let (a1_tx, a1_rx) = chan::<i8>(depth = 16);
    let (a2_tx, a2_rx) = chan::<i8>(depth = 16);
    let (a3_tx, a3_rx) = chan::<i8>(depth = 16);

    // The four B rows are transposed into four B columns before entering
    // the north edge of the array.
    let (br0_tx, br0_rx) = chan::<i8>(depth = 16);
    let (br1_tx, br1_rx) = chan::<i8>(depth = 16);
    let (br2_tx, br2_rx) = chan::<i8>(depth = 16);
    let (br3_tx, br3_rx) = chan::<i8>(depth = 16);

    let (b00_tx, b00_rx) = chan::<i8>(depth = 16);
    let (b01_tx, b01_rx) = chan::<i8>(depth = 16);
    let (b02_tx, b02_rx) = chan::<i8>(depth = 16);
    let (b03_tx, b03_rx) = chan::<i8>(depth = 16);
    let (b10_tx, b10_rx) = chan::<i8>(depth = 16);
    let (b11_tx, b11_rx) = chan::<i8>(depth = 16);
    let (b12_tx, b12_rx) = chan::<i8>(depth = 16);
    let (b13_tx, b13_rx) = chan::<i8>(depth = 16);
    let (b20_tx, b20_rx) = chan::<i8>(depth = 16);
    let (b21_tx, b21_rx) = chan::<i8>(depth = 16);
    let (b22_tx, b22_rx) = chan::<i8>(depth = 16);
    let (b23_tx, b23_rx) = chan::<i8>(depth = 16);
    let (b30_tx, b30_rx) = chan::<i8>(depth = 16);
    let (b31_tx, b31_rx) = chan::<i8>(depth = 16);
    let (b32_tx, b32_rx) = chan::<i8>(depth = 16);
    let (b33_tx, b33_rx) = chan::<i8>(depth = 16);

    let (bc0_tx, bc0_rx) = chan::<i8>(depth = 16);
    let (bc1_tx, bc1_rx) = chan::<i8>(depth = 16);
    let (bc2_tx, bc2_rx) = chan::<i8>(depth = 16);
    let (bc3_tx, bc3_rx) = chan::<i8>(depth = 16);

    // A paths: west to east.
    let (e00_tx, e00_rx) = chan::<i8>(depth = 16);
    let (e01_tx, e01_rx) = chan::<i8>(depth = 16);
    let (e02_tx, e02_rx) = chan::<i8>(depth = 16);
    let (e03_tx, e03_rx) = chan::<i8>(depth = 16);
    let (e10_tx, e10_rx) = chan::<i8>(depth = 16);
    let (e11_tx, e11_rx) = chan::<i8>(depth = 16);
    let (e12_tx, e12_rx) = chan::<i8>(depth = 16);
    let (e13_tx, e13_rx) = chan::<i8>(depth = 16);
    let (e20_tx, e20_rx) = chan::<i8>(depth = 16);
    let (e21_tx, e21_rx) = chan::<i8>(depth = 16);
    let (e22_tx, e22_rx) = chan::<i8>(depth = 16);
    let (e23_tx, e23_rx) = chan::<i8>(depth = 16);
    let (e30_tx, e30_rx) = chan::<i8>(depth = 16);
    let (e31_tx, e31_rx) = chan::<i8>(depth = 16);
    let (e32_tx, e32_rx) = chan::<i8>(depth = 16);
    let (e33_tx, e33_rx) = chan::<i8>(depth = 16);

    // B paths: north to south.
    let (s00_tx, s00_rx) = chan::<i8>(depth = 16);
    let (s01_tx, s01_rx) = chan::<i8>(depth = 16);
    let (s02_tx, s02_rx) = chan::<i8>(depth = 16);
    let (s03_tx, s03_rx) = chan::<i8>(depth = 16);
    let (s10_tx, s10_rx) = chan::<i8>(depth = 16);
    let (s11_tx, s11_rx) = chan::<i8>(depth = 16);
    let (s12_tx, s12_rx) = chan::<i8>(depth = 16);
    let (s13_tx, s13_rx) = chan::<i8>(depth = 16);
    let (s20_tx, s20_rx) = chan::<i8>(depth = 16);
    let (s21_tx, s21_rx) = chan::<i8>(depth = 16);
    let (s22_tx, s22_rx) = chan::<i8>(depth = 16);
    let (s23_tx, s23_rx) = chan::<i8>(depth = 16);
    let (s30_tx, s30_rx) = chan::<i8>(depth = 16);
    let (s31_tx, s31_rx) = chan::<i8>(depth = 16);
    let (s32_tx, s32_rx) = chan::<i8>(depth = 16);
    let (s33_tx, s33_rx) = chan::<i8>(depth = 16);

    // One result channel per processing element.
    let (z00_tx, z00_rx) = chan::<i32>(depth = 4);
    let (z01_tx, z01_rx) = chan::<i32>(depth = 4);
    let (z02_tx, z02_rx) = chan::<i32>(depth = 4);
    let (z03_tx, z03_rx) = chan::<i32>(depth = 4);
    let (z10_tx, z10_rx) = chan::<i32>(depth = 4);
    let (z11_tx, z11_rx) = chan::<i32>(depth = 4);
    let (z12_tx, z12_rx) = chan::<i32>(depth = 4);
    let (z13_tx, z13_rx) = chan::<i32>(depth = 4);
    let (z20_tx, z20_rx) = chan::<i32>(depth = 4);
    let (z21_tx, z21_rx) = chan::<i32>(depth = 4);
    let (z22_tx, z22_rx) = chan::<i32>(depth = 4);
    let (z23_tx, z23_rx) = chan::<i32>(depth = 4);
    let (z30_tx, z30_rx) = chan::<i32>(depth = 4);
    let (z31_tx, z31_rx) = chan::<i32>(depth = 4);
    let (z32_tx, z32_rx) = chan::<i32>(depth = 4);
    let (z33_tx, z33_rx) = chan::<i32>(depth = 4);

    // Transpose B and create the four north-edge column streams.
    spawn BRow(br0_rx, b00_tx, b01_tx, b02_tx, b03_tx);
    spawn BRow(br1_rx, b10_tx, b11_tx, b12_tx, b13_tx);
    spawn BRow(br2_rx, b20_tx, b21_tx, b22_tx, b23_tx);
    spawn BRow(br3_rx, b30_tx, b31_tx, b32_tx, b33_tx);
    spawn BColumn(b00_rx, b10_rx, b20_rx, b30_rx, bc0_tx);
    spawn BColumn(b01_rx, b11_rx, b21_rx, b31_rx, bc1_tx);
    spawn BColumn(b02_rx, b12_rx, b22_rx, b32_rx, bc2_tx);
    spawn BColumn(b03_rx, b13_rx, b23_rx, b33_rx, bc3_tx);

    // The 4 by 4 grid of processing elements.
    spawn Pe(a0_rx, bc0_rx, e00_tx, s00_tx, z00_tx);
    spawn Pe(e00_rx, bc1_rx, e01_tx, s01_tx, z01_tx);
    spawn Pe(e01_rx, bc2_rx, e02_tx, s02_tx, z02_tx);
    spawn Pe(e02_rx, bc3_rx, e03_tx, s03_tx, z03_tx);

    spawn Pe(a1_rx, s00_rx, e10_tx, s10_tx, z10_tx);
    spawn Pe(e10_rx, s01_rx, e11_tx, s11_tx, z11_tx);
    spawn Pe(e11_rx, s02_rx, e12_tx, s12_tx, z12_tx);
    spawn Pe(e12_rx, s03_rx, e13_tx, s13_tx, z13_tx);

    spawn Pe(a2_rx, s10_rx, e20_tx, s20_tx, z20_tx);
    spawn Pe(e20_rx, s11_rx, e21_tx, s21_tx, z21_tx);
    spawn Pe(e21_rx, s12_rx, e22_tx, s22_tx, z22_tx);
    spawn Pe(e22_rx, s13_rx, e23_tx, s23_tx, z23_tx);

    spawn Pe(a3_rx, s20_rx, e30_tx, s30_tx, z30_tx);
    spawn Pe(e30_rx, s21_rx, e31_tx, s31_tx, z31_tx);
    spawn Pe(e31_rx, s22_rx, e32_tx, s32_tx, z32_tx);
    spawn Pe(e32_rx, s23_rx, e33_tx, s33_tx, z33_tx);

    // Drain the east and south boundaries.
    spawn Drain(e03_rx); spawn Drain(e13_rx);
    spawn Drain(e23_rx); spawn Drain(e33_rx);
    spawn Drain(s30_rx); spawn Drain(s31_rx);
    spawn Drain(s32_rx); spawn Drain(s33_rx);

    spawn Collect(
        z00_rx, z01_rx, z02_rx, z03_rx,
        z10_rx, z11_rx, z12_rx, z13_rx,
        z20_rx, z21_rx, z22_rx, z23_rx,
        z30_rx, z31_rx, z32_rx, z33_rx, c
    );

    spawn Feeder(
        a, b,
        a0_tx, a1_tx, a2_tx, a3_tx,
        br0_tx, br1_tx, br2_tx, br3_tx
    );
}

test mm4_vectors {
    let (ta, ra) = chan::<i8>(depth = 32);
    let (tb, rb) = chan::<i8>(depth = 32);
    let (tc, rc) = chan::<i32>(depth = 32);
    spawn Mm4(ra, rb, tc);

    let av = [1, -2, 3, 4, 5, 6, -7, 8, -9, 10, 11, 12, 13, -14, 15, 127];
    let bv = [2, 0, -1, 3, 1, 4, 2, -5, -6, 7, 8, 1, 9, -10, 11, -128];
    for i in 0..16 {
        ta.send(av[i]);
        tb.send(bv[i]);
    }
    ta.close();
    tb.close();

    let expected = [
        18, -27, 63, -496,
        130, -105, 39, -1046,
        34, -3, 249, -1602,
        1065, -1221, 1476, -16132
    ];
    for i in 0..16 {
        assert rc.recv() == expected[i];
    }
}
