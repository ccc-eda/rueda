# tpu4: a tiny TPU, simulation only

A 4 × 4 int8 matrix unit driven by an instruction stream, in the style of the first TPU.

Streams (all inputs except `y`):
- `ins` of `(u8, u8, u8, u8)`: instructions (op, p, q, n), see below;
- `w` of `i8`: weights, consumed by LOADW;
- `x` of `i8`: activations, consumed by LOADX;
- `y` of `i8`: output, produced by STORE.

State: a unified buffer `ub` of 16 rows × 4 `i8`; two weight slots W[0], W[1] of 4 × 4 `i8`.

Instructions (op p q n):
- 1 LOADW  p 0 0  : read 16 values from `w` into W[p], row-major.
- 2 LOADX  p 0 n  : read n rows (4 values each) from `x` into ub[p .. p+n-1].
- 3 MATMUL p q n  : for i in 0..n: ub[q+i][j] = clamp((Σ_k ub[p+i][k] · Wsel[k][j]) >> 2,
                    -128, 127), with arithmetic shift; the high bit of the op byte selects
                    the slot and ReLU: op = 3 + 16·s + 32·relu (s = slot 0/1, relu 1 = max(0, ·)).
- 4 STORE  p 0 n  : send ub[p .. p+n-1] row by row on `y` (4 values a row).
- 0 HALT           : close `y` and stop.

Structure (required): the matrix multiply runs on a 4 × 4 grid of processing-element tasks
(spawned, connected by channels, weight-stationary or output-stationary as you like); a
controller task decodes `ins`, owns `ub` and W, and feeds the grid.

Task name: `Tpu4` with parameters (ins, w, x, y).

Test: a two-layer MLP on 4 input rows.
  ins = (1,0,0,0), (1,1,0,0), (2,0,0,4), (35,0,4,4) [MATMUL slot 0, ReLU, ub0..3 → ub4..7],
        (19,4,8,4) [MATMUL slot 1, no ReLU, ub4..7 → ub8..11], (4,8,0,4), (0,0,0,0)
  w   = W0 = 1, 2, -1, 0, 3, -2, 1, 1, -1, 1, 2, -3, 2, 0, 1, 1
        W1 = 2, -1, 0, 1, 1, 1, -2, 0, 0, 3, 1, -1, -2, 1, 1, 2
  x   = 10, -20, 30, -40, 5, 6, 7, 8, -1, -2, -3, -4, 100, -100, 50, -50
  y   = 5, 5, -11, 0, 4, 2, 0, 0, 0, 0, 0, 0, 28, 28, -56, 0
  (hidden layer after ReLU, for debugging: 0, 22, 0, 0, 8, 1, 5, 0, 0, 0, 0, 0, 0, 112, 0, 0)

Only `reda sim` has to pass (no synthesis).
