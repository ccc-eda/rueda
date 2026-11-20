# sort4: sort groups of four

Input stream `x` of `u8`, output stream `y` of `u8`. Read the input four values at a time
and output each group of four in ascending order. When `x` closes, close `y`.

Task / module name: `Sort4`.

Test vectors:
  x = 9, 3, 7, 1,   0, 255, 128, 128,   5, 5, 5, 4
  y = 1, 3, 7, 9,   0, 128, 128, 255,   4, 5, 5, 5
