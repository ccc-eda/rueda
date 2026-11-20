# conv3x3: a 3×3 convolution over an image stream

Input stream `x` of `u8`: an 8 × 6 image (width 8, height 6), row-major. Output stream `y` of
`u8`: for every pixel that has all eight neighbours (rows 1..4, columns 1..6, row-major, 24
outputs), output

    y = (Σ K[i][j] · x[r-1+i][c-1+j]) >> 4,   K = [[1, 2, 1], [2, 4, 2], [1, 2, 1]]

Keep at most two previous rows in memory (line buffers) plus a 3 × 3 window, not the whole
image. After the image, close `y` (x closes after the 48 pixels).

Task / module name: `Conv3x3`.

Test vectors:
  x = 0, 11, 22, 33, 44, 55, 66, 77,   37, 67, 97, 127, 157, 187, 217, 114,
      74, 123, 172, 221, 137, 186, 235, 151,   111, 179, 247, 182, 250, 185, 253, 188,
      148, 235, 189, 20, 230, 61, 15, 225,   185, 35, 8, 237, 87, 60, 33, 6
  y = 67, 97, 118, 132, 153, 158,   123, 163, 179, 178, 194, 201,
      170, 189, 183, 184, 178, 180,   162, 150, 146, 151, 115, 102
