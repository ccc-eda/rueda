// Streaming 3x3 convolution for the fixed 8x6 image in the specification.
//
// prev and prev2 are two eight-pixel line buffers.  At column c they contain
// pixels from the preceding and second-preceding rows.  The six horizontal
// registers and the three current-row registers retain one 3x3 window.

task Conv3x3(x: in chan<u8>, y: out chan<u8>) {
    let mut prev: [u8; 8] @reg = [0; 8];
    let mut prev2: [u8; 8] @reg = [0; 8];

    let mut top2: u8 @reg = 0;
    let mut top1: u8 @reg = 0;
    let mut mid2: u8 @reg = 0;
    let mut mid1: u8 @reg = 0;
    let mut bot2: u8 @reg = 0;
    let mut bot1: u8 @reg = 0;

    let mut row: u3 = 0;
    let mut col: u3 = 0;

    for pixel in x {
        // The line buffers are overwritten column by column.  Read the old
        // values before replacing them with the rows currently arriving.
        let top = prev2[col];
        let mid = prev[col];

        // Horizontal histories restart at the beginning of every row.
        if col == 0 {
            top2 = 0;
            top1 = 0;
            mid2 = 0;
            mid1 = 0;
            bot2 = 0;
            bot1 = 0;
        }

        // Rows 2..5 and columns 2..7 are lower-right corners of the
        // requested output windows (centres in rows 1..4, columns 1..6).
        if row >= 2 && col >= 2 {
            let acc: u16 =
                top2 + 2 * top1 + top +
                2 * (mid2 + 2 * mid1 + mid) +
                bot2 + 2 * bot1 + pixel;
            y.send((acc >> 4).to::<u8>(ovf = sat));
        }

        // Advance the two line buffers and the horizontal window.
        prev2[col] = prev[col];
        prev[col] = pixel;

        top2 = top1;
        top1 = top;
        mid2 = mid1;
        mid1 = mid;
        bot2 = bot1;
        bot1 = pixel;

        if col == 7 {
            col = 0;
            row = (row + 1).sat::<u3>();
        } else {
            col = (col + 1).sat::<u3>();
        }
    }

    y.close();
}

test conv3x3_vectors {
    let (tx, rx) = chan::<u8>(depth = 64);
    let (ty, ry) = chan::<u8>(depth = 32);
    spawn Conv3x3(rx, ty);

    let input = [
        0, 11, 22, 33, 44, 55, 66, 77,
        37, 67, 97, 127, 157, 187, 217, 114,
        74, 123, 172, 221, 137, 186, 235, 151,
        111, 179, 247, 182, 250, 185, 253, 188,
        148, 235, 189, 20, 230, 61, 15, 225,
        185, 35, 8, 237, 87, 60, 33, 6
    ];
    let expected = [
        67, 97, 118, 132, 153, 158,
        123, 163, 179, 178, 194, 201,
        170, 189, 183, 184, 178, 180,
        162, 150, 146, 151, 115, 102
    ];

    for i in 0..48 {
        tx.send(input[i]);
    }
    tx.close();

    for i in 0..24 {
        assert ry.recv() == expected[i];
    }
}
