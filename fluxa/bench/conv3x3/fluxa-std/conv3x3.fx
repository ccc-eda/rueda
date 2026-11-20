task Conv3x3(x: in chan<u8>, y: out chan<u8>) {
    // Two row buffers retain only the two rows needed by the sliding window.
    let mut previous: [u8; 8] @reg = [0; 8];
    let mut previous2: [u8; 8] @reg = [0; 8];
    let mut window: [[u8; 3]; 3] = [[0; 3]; 3];
    let mut row: u3 = 0;
    let mut col: u3 = 0;

    for pixel in x {
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

        // The lower-right corner is at (row, col), so this window is centred
        // at (row - 1, col - 1).
        if row >= 2 && col >= 2 {
            let sum: u16 =
                window[0][0] + 2 * window[0][1] + window[0][2] +
                2 * (window[1][0] + 2 * window[1][1] + window[1][2]) +
                window[2][0] + 2 * window[2][1] + window[2][2];
            y.send((sum >> 4).to::<u8>(round = trunc, ovf = sat));
        }

        previous2[col] = previous[col];
        previous[col] = pixel;

        if col == 7 {
            col = 0;
            row = (row + 1).sat::<u3>();
        } else {
            col = (col + 1).sat::<u3>();
        }
    }
    y.close();
}

test conv3x3_vector {
    let (tx, rx) = chan::<u8>(depth = 48);
    let (ty, ry) = chan::<u8>(depth = 24);
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
