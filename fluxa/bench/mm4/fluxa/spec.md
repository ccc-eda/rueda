# mm4: 4×4 matrix multiply on a systolic array

Input streams `a` and `b` of `i8`, output stream `c` of `i32`. Read the 16 elements of A
(row-major) from `a` and the 16 elements of B (row-major) from `b`, compute C = A × B and
output its 16 elements row-major on `c`. Repeat for the next pair of matrices until `a`
closes (b closes at the same time); then close `c`.

Structure (required): a 4×4 grid of processing elements, each its own instance (Fluxa: a
`task` spawned 16 times; Veryl: a module instantiated 16 times), connected by channels /
signals so that A's values flow east along the rows and B's values flow south along the
columns, each PE accumulating one element of C. The top `Mm4` builds and feeds the grid.

Task / module name: `Mm4`.

Test vectors (one pair):
  A = 1, -2, 3, 4,   5, 6, -7, 8,   -9, 10, 11, 12,   13, -14, 15, 127
  B = 2, 0, -1, 3,   1, 4, 2, -5,   -6, 7, 8, 1,     9, -10, 11, -128
  C = 18, -27, 63, -496,   130, -105, 39, -1046,   34, -3, 249, -1602,   1065, -1221, 1476, -16132
