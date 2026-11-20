# fir4: a 4-tap FIR filter

Input stream `x` of `i8`, output stream `y` of `i8`. For every input sample x[n], output

    y[n] = (3*x[n] + 5*x[n-1] + 5*x[n-2] + 3*x[n-3]) >> 4      (arithmetic shift)

with x[-1] = x[-2] = x[-3] = 0. One output per input. When `x` closes, close `y`.
Throughput goal: one sample per clock.

Task / module name: `Fir4`.

Test vectors:
  x = 16, 0, 0, 0, 0, -16, 32, 7, -128, 127, 100, -100
  y = 3, 5, 5, 3, 0, -3, 1, 6, -15, -8, 19, 28
