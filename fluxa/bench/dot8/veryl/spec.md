# dot8: dot products of 8 pairs

Input streams `a` and `b` of `i8`, output stream `y` of `i32`. Read the pairs (a, b) eight
at a time; after every 8 pairs output the sum of the 8 products. When `a` closes (b closes at
the same time), close `y`.
Throughput goal: one pair per clock.

Task / module name: `Dot8`.

Test vectors (16 pairs → 2 outputs):
  a = 1, 2, 3, 4, 5, 6, 7, 8,   127, 127, 127, 127, -128, -128, -128, -128
  b = 8, 7, 6, 5, 4, 3, 2, 1,   127, 127, 127, 127, -128, -128, -128, -128
  y = 120, 130052
