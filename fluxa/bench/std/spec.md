# Two standard-library generators for Fluxa

Write `std.fx` with two parameterised task networks, and `std_test.fx` with top tasks and
tests. They become Fluxa's standard library (`std::SystolicMatmul::<…>`), so they must be
correct, small in hardware and easy to call. Check with

    reda sim std.fx std_test.fx
    reda synth --top Mm4Top std.fx std_test.fx
    reda synth --top Win8x6Top std.fx std_test.fx
    reda fpga --device hx8k --package ct256 --top Mm4Top --bin /tmp/m.bin std.fx std_test.fx   (prints ICESTORM_LC)

## Rules of the current Fluxa implementation (learned the hard way)

- Element types use const generics, not type parameters (type parameters are not
  synthesised). A signed AW-bit value is `int<-(1 << (AW - 1)) ..= (1 << (AW - 1)) - 1>`
  (equal to `i8` when AW = 8); an unsigned PW-bit value is `int<0 ..= (1 << PW) - 1>`.
- A task parameter cannot be an array of channels in synthesis (`outs: [chan<T>; N]` only
  simulates). Every task's interface is single channels and values.
- A task that spawns may only build the network (`chan`, `array(n, |_| chan::<T>(depth = d))`,
  `spawn` in loops). Index local channel arrays as `cs[i].0` (tx) and `cs[i].1` (rx).
- Use depth 2 for internal channels. Do not deepen FIFOs to get past a simulation deadlock:
  find the real cause (usually the order in which a task reads its inputs).
- Wrapping arithmetic: `(x + 1).wrap::<T>()`; `+%` needs a typed variable on the left.

## 1. SystolicMatmul<const N: u32, const AW: u32, const CW: u32>(a, b, c)

- `a`, `b`: `in chan<sAW>` (signed AW bits); `c`: `out chan<sCW>`.
- Reads A (N×N, row-major) from `a` and B (N×N, row-major) from `b` **in lockstep**: one
  value of `a`, then one of `b`, N·N times. Computes C = A × B and sends C row-major on `c`.
  Repeats for the next pair until `a` closes (then `b` closes too); then closes `c`.
- Structure: an N×N grid of PE tasks, output-stationary: A's rows enter from the west,
  skewed (row i delayed by i), and flow east; B's columns enter from the north, skewed, and
  flow south; each PE multiplies and accumulates N products into its own CW-bit register.
- Results leave without channel-array parameters: e.g. each row's results shift east along
  the row to the east edge, and the east edge forwards rows south, so the last task sends
  all N·N values row-major. Edge PEs should not need extra sink tasks (use const generic
  flags such as `const EAST: u32` to leave out a forward), or keep sinks few.
- Must fit an iCE40 HX8K for N = 4, AW = 8, CW = 32 (well under 7,680 LC; smaller is better).

Tests in std_test.fx (`task Mm4Top(a: in chan<i8>, b: in chan<i8>, c: out chan<i32>)` =
`spawn SystolicMatmul::<4, 8, 32>(a, b, c)`; also a `Mm2Top` with N = 2):
- N = 4: A = 1,-2,3,4, 5,6,-7,8, -9,10,11,12, 13,-14,15,127
         B = 2,0,-1,3, 1,4,2,-5, -6,7,8,1, 9,-10,11,-128
         C = 18,-27,63,-496, 130,-105,39,-1046, 34,-3,249,-1602, 1065,-1221,1476,-16132
- N = 2, two pairs in one run: A = 1,2,3,4  B = 5,6,7,8  → C = 19,22,43,50;
         then A = -1,0,2,-3  B = 4,-5,-6,7  → C = -4,5,26,-31.
  Feed a and b interleaved in the test (a[k], b[k], a[k+1], …) with input depth 2.

## 2. Window3x3<const W: u32, const H: u32, const PW: u32>(x, w)

- `x`: `in chan<uPW>` (unsigned PW bits), a W×H image row-major; repeated frames until `x`
  closes. `w`: `out chan<[[uPW; 3]; 3]>`: for every pixel (r, c) with 1 ≤ r ≤ H−2 and
  1 ≤ c ≤ W−2, in row-major order, the 3×3 neighbourhood `w[i][j] = x[r−1+i][c−1+j]`.
  After the last frame, close `w`.
- Keep two line buffers of W pixels (`@reg` arrays) and the 3×3 window; never the whole frame.

Test in std_test.fx: `task Win8x6Top(x: in chan<u8>, y: out chan<u8>)` spawns
`Window3x3::<8, 6, 8>` and a small `Blur` task that applies
`y = (Σ K[i][j]·w[i][j]) >> 4`, K = [[1,2,1],[2,4,2],[1,2,1]]. With
  x = 0,11,22,33,44,55,66,77, 37,67,97,127,157,187,217,114, 74,123,172,221,137,186,235,151,
      111,179,247,182,250,185,253,188, 148,235,189,20,230,61,15,225, 185,35,8,237,87,60,33,6
  y = 67,97,118,132,153,158, 123,163,179,178,194,201, 170,189,183,184,178,180, 162,150,146,151,115,102
