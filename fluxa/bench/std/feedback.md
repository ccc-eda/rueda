# Round 2: two failures left (Mm4Top passes and all three tops synthesize — keep that)

1. `mm2_two_pairs` fails: `std_test.fx:72: recv() on a closed, empty channel`. The second pair
   of matrices is never computed: SystolicMatmul handles one pair and stops. It must repeat
   (read the next N·N pairs, compute, send) until `a` closes, then close `c`. Every task in
   the network has to loop the same way (feeder, pickers, PEs, mergers), each until its
   input closes, so the whole network closes in order.
2. `win8x6_vector` deadlocks: the *test* sends all 48 pixels into a depth-2 channel before
   receiving anything, while the outputs back up (depth 2). Fix the test, not the generator:
   give the test's output channel depth 32 (or interleave sends and receives). Do not
   deepen the generator's internal channels.

Run `reda sim std.fx std_test.fx` until all three tests pass, and keep the three
`reda synth --top … std.fx std_test.fx` passing.
