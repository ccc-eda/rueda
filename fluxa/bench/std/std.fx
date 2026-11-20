// Parameterised streaming matrix multiplication and 3x3 window generators.
//
// SystolicMatmul uses an N by N output-stationary processing-element grid.
// A rows enter from the west and B columns enter from the north.  Each PE
// consumes one A and one B value per product and keeps the N-product sum in
// its own CW-bit accumulator.  Results are shifted east within each row and
// the completed rows are shifted south, which produces row-major output.

task MatrixFeeder<const AW: u32>(
    a: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    b: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    aout: out chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    bout: out chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>
) {
    // A and B are consumed in lockstep.  The broadcaster and the pickers
    // recover the rows and columns without buffering a complete frame here.
    for av in a {
        let bv = b.recv();
        aout.send(av);
        bout.send(bv);
    }
}

task RowPicker<const N: u32, const AW: u32, const ROW: u32>(
    x: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    y: out chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>
) {
    let mut row: [int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>; N] = [0; N];
    let mut pos: u32 = 0;

    for v in x {
        if pos / N == ROW {
            row[pos % N] = v;
        }
        pos = (pos + 1).wrap::<u32>();
        if pos == N * N {
            for k in 0..N {
                y.send(row[k]);
            }
            pos = 0;
        }
    }
    y.close();
}

task ColumnPicker<const N: u32, const AW: u32, const COL: u32>(
    x: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    y: out chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>
) {
    let mut column: [int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>; N] = [0; N];
    let mut pos: u32 = 0;

    for v in x {
        if pos % N == COL {
            column[pos / N] = v;
        }
        pos = (pos + 1).wrap::<u32>();
        if pos == N * N {
            for k in 0..N {
                y.send(column[k]);
            }
            pos = 0;
        }
    }
    y.close();
}

// The first column has no result coming from its west neighbour.  Its own
// result is the first token on the row's result shift register.
task PeFirst<const N: u32, const AW: u32, const CW: u32>(
    ain: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    bin: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    aout: out chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    bout: out chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    result: out chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>
) {
    let mut acc: int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1> = 0;
    let mut count: u32 = 0;

    // Each input contains one frame after another.  Keep the accumulator
    // alive across frames, but start a new sum whenever N products have
    // been consumed.
    for av in ain {
        let bv = bin.recv();
        let product = av * bv;
        acc = (acc + product).wrap::<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>();
        aout.send(av);
        bout.send(bv);
        count = (count + 1).wrap::<u32>();
        if count == N {
            result.send(acc);
            acc = 0;
            count = 0;
        }
    }
}

// COL is also the number of completed results which arrive from the west.
// They are forwarded before this PE's result, preserving row order.
task Pe<const N: u32, const AW: u32, const CW: u32, const COL: u32>(
    ain: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    bin: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    aout: out chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    bout: out chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    previous: in chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>,
    result: out chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>
) {
    let mut acc: int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1> = 0;
    let mut count: u32 = 0;

    // The A/B streams contain one matrix after another.  The result shift
    // register must carry the same frame boundary as the data streams.
    for av in ain {
        let bv = bin.recv();
        let product = av * bv;
        acc = (acc + product).wrap::<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>();
        aout.send(av);
        bout.send(bv);
        count = (count + 1).wrap::<u32>();
        if count == N {
            for _ in 0..COL {
                let old = previous.recv();
                result.send(old);
            }
            result.send(acc);
            acc = 0;
            count = 0;
        }
    }
}

task Drain(x: in chan<i8>) {
    for _value in x {
    }
}

// A row is emitted as N scalar result tokens.  This first vertical stage
// simply forwards row zero.
task RowMergeFirst<const N: u32, const CW: u32>(
    own: in chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>,
    out: out chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>
) {
    // One row is N tokens; the loop repeats that row for every frame.
    for v in own {
        out.send(v);
    }
    out.close();
}

// Intermediate vertical stages carry all preceding rows and then append
// their own row.  The first token of each row is read early so the PE row
// cannot be back-pressured while an earlier row is still travelling south.
task RowMergeMid<const N: u32, const CW: u32, const ROW: u32>(
    previous: in chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>,
    own: in chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>,
    out: out chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>
) {
    for first in own {
        let mut own_row: [int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>; N] = [0; N];
        own_row[0] = first;
        for k in 1..N {
            own_row[k] = own.recv();
        }
        for _ in 0..N * ROW {
            out.send(previous.recv());
        }
        for k in 0..N {
            out.send(own_row[k]);
        }
    }
    out.close();
}

task RowMergeLast<const N: u32, const CW: u32>(
    previous: in chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>,
    own: in chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>,
    c: out chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>
) {
    for first in own {
        let mut own_row: [int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>; N] = [0; N];
        own_row[0] = first;
        for k in 1..N {
            own_row[k] = own.recv();
        }
        for _ in 0..N * (N - 1) {
            c.send(previous.recv());
        }
        for k in 0..N {
            c.send(own_row[k]);
        }
    }
    c.close();
}

task SystolicMatmul<const N: u32, const AW: u32, const CW: u32>(
    a: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    b: in chan<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>,
    c: out chan<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>
) {
    let (afeed_tx, afeed_rx) = chan::<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>(depth = 2);
    let (bfeed_tx, bfeed_rx) = chan::<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>(depth = 2);
    spawn MatrixFeeder::<AW>(a, b, afeed_tx, bfeed_tx);

    let a_picks = broadcast(afeed_rx, N);
    let b_picks = broadcast(bfeed_rx, N);

    let row_edges = array(N, |_| chan::<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>(depth = 2));
    let col_edges = array(N, |_| chan::<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>(depth = 2));
    for i in 0..N {
        spawn RowPicker::<N, AW, i>(a_picks[i], row_edges[i].0);
    }
    for j in 0..N {
        spawn ColumnPicker::<N, AW, j>(b_picks[j], col_edges[j].0);
    }

    // Horizontal A links, vertical B links, and horizontal result links.
    let a_links = array(N * (N - 1), |_| chan::<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>(depth = 2));
    let b_links = array(N * (N - 1), |_| chan::<int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>>(depth = 2));
    let result_links = array(N * (N - 1), |_| chan::<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>(depth = 2));
    let row_outputs = array(N, |_| chan::<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>(depth = 2));

    // Edge forwarding channels are drained rather than allowing an unused
    // east/south output to back up the grid.
    let a_drains = array(N, |_| chan::<i8>(depth = 2));
    let b_drains = array(N, |_| chan::<i8>(depth = 2));

    for i in 0..N {
        for j in 0..N {
            let ain = if j == 0 { row_edges[i].1 } else { a_links[i * (N - 1) + j - 1].1 };
            let bin = if i == 0 { col_edges[j].1 } else { b_links[(i - 1) * N + j].1 };
            let aout = if j == N - 1 { a_drains[i].0 } else { a_links[i * (N - 1) + j].0 };
            let bout = if i == N - 1 { b_drains[j].0 } else { b_links[i * N + j].0 };
            let result = if j == N - 1 { row_outputs[i].0 } else { result_links[i * (N - 1) + j].0 };

            if j == 0 {
                spawn PeFirst::<N, AW, CW>(ain, bin, aout, bout, result);
            } else {
                let previous = result_links[i * (N - 1) + j - 1].1;
                spawn Pe::<N, AW, CW, j>(ain, bin, aout, bout, previous, result);
            }
        }
    }
    for i in 0..N {
        spawn Drain(a_drains[i].1);
    }
    for j in 0..N {
        spawn Drain(b_drains[j].1);
    }

    // Carry completed rows south.  Only the final stage exposes c.
    let vertical = array(N - 1, |_| chan::<int<-(1 << (CW - 1)) ..= (1 << (CW - 1)) - 1>>(depth = 2));
    spawn RowMergeFirst::<N, CW>(row_outputs[0].1, vertical[0].0);
    for i in 1..N - 1 {
        spawn RowMergeMid::<N, CW, i>(vertical[i - 1].1, row_outputs[i].1, vertical[i].0);
    }
    spawn RowMergeLast::<N, CW>(vertical[N - 2].1, row_outputs[N - 1].1, c);
}

// A two-line sliding window.  The line buffers are register arrays; the
// window itself contains only the currently active 3 by 3 neighbourhood.
task Window3x3<const W: u32, const H: u32, const PW: u32>(
    x: in chan<int<0 ..= (1 << PW) - 1>>,
    w: out chan<[[int<0 ..= (1 << PW) - 1>; 3]; 3]>
) {
    let mut previous: [int<0 ..= (1 << PW) - 1>; W] @reg = [0; W];
    let mut previous2: [int<0 ..= (1 << PW) - 1>; W] @reg = [0; W];
    let mut window: [[int<0 ..= (1 << PW) - 1>; 3]; 3] = [[0; 3]; 3];
    let mut row: u32 = 0;
    let mut col: u32 = 0;

    for pixel in x {
        // Read the old rows before replacing their current-column entries.
        let top = previous2[col];
        let middle = previous[col];

        if col == 0 {
            window[0][0] = 0; window[0][1] = 0; window[0][2] = 0;
            window[1][0] = 0; window[1][1] = 0; window[1][2] = 0;
            window[2][0] = 0; window[2][1] = 0; window[2][2] = 0;
        }

        window[0][0] = window[0][1];
        window[0][1] = window[0][2];
        window[0][2] = top;
        window[1][0] = window[1][1];
        window[1][1] = window[1][2];
        window[1][2] = middle;
        window[2][0] = window[2][1];
        window[2][1] = window[2][2];
        window[2][2] = pixel;

        // The lower-right corner of the window is (row,col), hence the
        // corresponding centre is (row-1,col-1).
        if row >= 2 && col >= 2 {
            w.send(window);
        }

        previous2[col] = previous[col];
        previous[col] = pixel;

        if col == W - 1 {
            col = 0;
            if row == H - 1 {
                row = 0;
            } else {
                row = (row + 1).wrap::<u32>();
            }
        } else {
            col = (col + 1).wrap::<u32>();
        }
    }
    w.close();
}
