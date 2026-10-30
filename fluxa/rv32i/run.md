```
./target/release/fluxa rv32i/rv32i.fx rv32i/smoke.fx --stats
Output

fluxa: rv32i/rv32i.fx rv32i/smoke.fx — 75 task/function(s), 1 test(s)
[PASS] smoke_sum (0.006s)
  channel                      depth     sent     recv max-occ
  test:smoke_sum:dump_tx->dump_rx    40       36       36      32
  Core:f2d_tx->f2d_rx              1       34       34       1
  Core:d2e_tx->d2e_rx              1       33       33       1
  Core:e2m_tx->e2m_rx              1       33       33       1
  Core:wb_tx->wb_rx                8       23       23       3
  Core:rd_tx->rd_rx                1       10       10       1
1 passed, 0 failed

```
