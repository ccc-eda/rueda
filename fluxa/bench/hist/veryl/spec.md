# hist: a 16-bin histogram

Input stream `x` of `u4`, output stream `y` of `u16`. Count how many times each value 0..15
occurs until `x` closes; then output the 16 counts in order (count of 0 first) and close `y`.
Keep the counts in a memory (Fluxa: `mem<u16, 16>`; Veryl: an array written in always_ff).

Task / module name: `Hist`.

Test vector (103 values):
  x = 3,1,4,1,5,9,2,6,5,3,5,8,9,7,9,3,2,3,8,4,6,2,6,4,3,3,8,3,2,7,9,5,0,2,8,8,4,1,9,7,1,6,9,3,9,9,3,7,5,1,0,5,8,2,0,9,7,4,9,4,4,5,9,2,3,0,7,8,1,6,4,0,6,2,8,6,2,0,8,9,9,8,6,2,8,0,3,4,8,2,5,3,4,2,1,1,7,0,6,7,15,15,14
  y = 8, 8, 12, 12, 10, 8, 9, 8, 12, 13, 0, 0, 0, 0, 1, 2
