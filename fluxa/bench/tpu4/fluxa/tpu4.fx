// A four-by-four int8 systolic TPU.
//
// The sixteen Pe tasks form a 4x4 grid.  Activation values move to the
// right, weight values move down, and every PE performs one multiply for
// each pair it receives.  Thus Pe(i,j) computes dot(ub[p+i], W[k][j]) and
// accumulates the four products of one matrix operation.

task Pe(row_in: in chan<i8>, row_out: out chan<i8>,
        col_in: in chan<i8>, col_out: out chan<i8>,
        result: out chan<i32>) {
    let mut acc: i32 = 0;
    let mut count: u3 = 0;

    // Every matrix operation contributes exactly four products.  The grid
    // can consequently be reused for an arbitrary number of operations.
    for a in row_in {
        row_out.send(a);
        let b = col_in.recv();
        col_out.send(b);

        let product: i32 = a * b;
        acc += product;
        count = (count + 1).sat::<u3>();

        if count == 4 {
            result.send(acc);
            acc = 0;
            count = 0;
        }
    }
}

task Sink(rx: in chan<i8>) {
    for _ in rx { }
}

// Grid owns the links internal to the PE array.  `rows` and `cols` are the
// four incoming row/column streams; `results` contains the sixteen output
// endpoints in row-major order.
task Grid(rows: [chan<i8>; 4], cols: [chan<i8>; 4],
         results: [chan<i32>; 16]) {
    // Horizontal links: row i, between columns j and j+1.
    let h01 = chan::<i8>(depth = 8);
    let h02 = chan::<i8>(depth = 8);
    let h03 = chan::<i8>(depth = 8);
    let h11 = chan::<i8>(depth = 8);
    let h12 = chan::<i8>(depth = 8);
    let h13 = chan::<i8>(depth = 8);
    let h21 = chan::<i8>(depth = 8);
    let h22 = chan::<i8>(depth = 8);
    let h23 = chan::<i8>(depth = 8);
    let h31 = chan::<i8>(depth = 8);
    let h32 = chan::<i8>(depth = 8);
    let h33 = chan::<i8>(depth = 8);

    // Vertical links: column j, between rows i and i+1.
    let v01 = chan::<i8>(depth = 8);
    let v02 = chan::<i8>(depth = 8);
    let v03 = chan::<i8>(depth = 8);
    let v11 = chan::<i8>(depth = 8);
    let v12 = chan::<i8>(depth = 8);
    let v13 = chan::<i8>(depth = 8);
    let v21 = chan::<i8>(depth = 8);
    let v22 = chan::<i8>(depth = 8);
    let v23 = chan::<i8>(depth = 8);
    let v31 = chan::<i8>(depth = 8);
    let v32 = chan::<i8>(depth = 8);
    let v33 = chan::<i8>(depth = 8);

    // Right-edge and bottom-edge links need consumers.
    let sr0 = chan::<i8>(depth = 8);
    let sr1 = chan::<i8>(depth = 8);
    let sr2 = chan::<i8>(depth = 8);
    let sr3 = chan::<i8>(depth = 8);
    let sc0 = chan::<i8>(depth = 8);
    let sc1 = chan::<i8>(depth = 8);
    let sc2 = chan::<i8>(depth = 8);
    let sc3 = chan::<i8>(depth = 8);

    // Row 0.
    spawn Pe(rows[0], h01.0, cols[0], v01.0, results[0]);
    spawn Pe(h01.1, h02.0, cols[1], v11.0, results[1]);
    spawn Pe(h02.1, h03.0, cols[2], v21.0, results[2]);
    spawn Pe(h03.1, sr0.0, cols[3], v31.0, results[3]);

    // Row 1.
    spawn Pe(rows[1], h11.0, v01.1, v02.0, results[4]);
    spawn Pe(h11.1, h12.0, v11.1, v12.0, results[5]);
    spawn Pe(h12.1, h13.0, v21.1, v22.0, results[6]);
    spawn Pe(h13.1, sr1.0, v31.1, v32.0, results[7]);

    // Row 2.
    spawn Pe(rows[2], h21.0, v02.1, v03.0, results[8]);
    spawn Pe(h21.1, h22.0, v12.1, v13.0, results[9]);
    spawn Pe(h22.1, h23.0, v22.1, v23.0, results[10]);
    spawn Pe(h23.1, sr2.0, v32.1, v33.0, results[11]);

    // Row 3.
    spawn Pe(rows[3], h31.0, v03.1, sc0.0, results[12]);
    spawn Pe(h31.1, h32.0, v13.1, sc1.0, results[13]);
    spawn Pe(h32.1, h33.0, v23.1, sc2.0, results[14]);
    spawn Pe(h33.1, sr3.0, v33.1, sc3.0, results[15]);

    spawn Sink(sr0.1);
    spawn Sink(sr1.1);
    spawn Sink(sr2.1);
    spawn Sink(sr3.1);
    spawn Sink(sc0.1);
    spawn Sink(sc1.1);
    spawn Sink(sc2.1);
    spawn Sink(sc3.1);
}

// The controller owns the unified buffer and the two weight slots.  It
// decodes instructions, loads state, drives the PE grid, and stores results.
task Controller(ins: in chan<(u8, u8, u8, u8)>,
                w: in chan<i8>, x: in chan<i8>, y: out chan<i8>,
                rows: [chan<i8>; 4], cols: [chan<i8>; 4],
                results: [chan<i32>; 16]) {
    let mut ub: [[i8; 4]; 16] = [[0; 4]; 16];
    let mut W: [[[i8; 4]; 4]; 2] = [[[0; 4]; 4]; 2];

    for instruction in ins {
        let (op, p, q, n) = instruction;
        let base_op = op & 15;
        let slot: u8 = (op >> 4) & 1;
        let relu: u8 = (op >> 5) & 1;

        if base_op == 1 {
            // LOADW: sixteen row-major weights into the selected slot.
            for k in 0..4 {
                for j in 0..4 {
                    W[p][k][j] = w.recv();
                }
            }
        } else if base_op == 2 {
            // LOADX: n four-byte activation rows.
            for i in 0..n {
                for j in 0..4 {
                    ub[p + i][j] = x.recv();
                }
            }
        } else if base_op == 3 {
            // MATMUL.  Feed all four grid rows on every operation; rows that
            // are outside the requested result range use zero activations.
            // Keeping all rows synchronized is required by the systolic grid.
            for i in 0..4 {
                for k in 0..4 {
                    let a: i8 = if i < n { ub[p + i][k] } else { 0 };
                    rows[i].send(a);
                }
            }
            for j in 0..4 {
                for k in 0..4 {
                    cols[j].send(W[slot][k][j]);
                }
            }

            // Consume every grid result so that unused rows cannot back up
            // the result channels on the next matrix operation.
            for i in 0..4 {
                for j in 0..4 {
                    let raw: i32 = results[i * 4 + j].recv();
                    let shifted: i32 = raw >> 2;
                    let activated: i32 =
                        if relu != 0 && shifted < 0 { 0 } else { shifted };
                    if i < n {
                        ub[q + i][j] = activated.sat::<i8>();
                    }
                }
            }
        } else if base_op == 4 {
            // STORE: emit the selected rows in row-major order.
            for i in 0..n {
                for j in 0..4 {
                    y.send(ub[p + i][j]);
                }
            }
        } else if base_op == 0 {
            // HALT: terminate the output stream.
            y.close();
            return;
        }
    }
}

task Tpu4(ins: in chan<(u8, u8, u8, u8)>,
          w: in chan<i8>, x: in chan<i8>, y: out chan<i8>) {
    let row_links = array(4, |_| chan::<i8>(depth = 8));
    let col_links = array(4, |_| chan::<i8>(depth = 8));
    let result_links = array(16, |_| chan::<i32>(depth = 2));

    let row_rx = row_links.map(|p| p.1);
    let row_tx = row_links.map(|p| p.0);
    let col_rx = col_links.map(|p| p.1);
    let col_tx = col_links.map(|p| p.0);
    let result_tx = result_links.map(|p| p.0);
    let result_rx = result_links.map(|p| p.1);

    spawn Grid(row_rx, col_rx, result_tx);
    spawn Controller(ins, w, x, y, row_tx, col_tx, result_rx);
}

test tpu4_mlp_vectors {
    let (ti, ri) = chan::<(u8, u8, u8, u8)>(depth = 8);
    let (tw, rw) = chan::<i8>(depth = 64);
    let (tx, rx) = chan::<i8>(depth = 32);
    let (ty, ry) = chan::<i8>(depth = 16);

    spawn Tpu4(ri, rw, rx, ty);

    let instructions = [
        (1, 0, 0, 0),
        (1, 1, 0, 0),
        (2, 0, 0, 4),
        (35, 0, 4, 4),
        (19, 4, 8, 4),
        (4, 8, 0, 4),
        (0, 0, 0, 0)
    ];
    for instruction in instructions {
        ti.send(instruction);
    }
    ti.close();

    let weights = [
        1, 2, -1, 0,
        3, -2, 1, 1,
        -1, 1, 2, -3,
        2, 0, 1, 1,
        2, -1, 0, 1,
        1, 1, -2, 0,
        0, 3, 1, -1,
        -2, 1, 1, 2
    ];
    for value in weights {
        tw.send(value);
    }
    tw.close();

    let activations = [
        10, -20, 30, -40,
        5, 6, 7, 8,
        -1, -2, -3, -4,
        100, -100, 50, -50
    ];
    for value in activations {
        tx.send(value);
    }
    tx.close();

    let expected = [
        5, 5, -11, 0,
        4, 2, 0, 0,
        0, 0, 0, 0,
        28, 28, -56, 0
    ];
    for i in 0..16 {
        assert ry.recv() == expected[i], "TPU output mismatch";
    }
}
