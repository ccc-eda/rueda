// std::SystolicMatmul 的其他參數：N = 3、16 位元輸入、40 位元累加（期望值由 Python 算出）
task M3(a: in chan<i16>, b: in chan<i16>, c: out chan<i40>) { spawn std::SystolicMatmul::<3, 16, 40>(a, b, c); }
test m3 {
    let (ta, ra) = chan::<i16>(depth = 2); let (tb, rb) = chan::<i16>(depth = 2); let (tc, rc) = chan::<i40>(depth = 16);
    spawn M3(ra, rb, tc);
    let av = [30000, -2, 3, 4, -32768, 6, 7, 8, 32767]; let bv = [-32768, 1, 2, 3, 32767, 5, 6, 7, -8]; let cv = [-983039988, -35513, 59966, -229340, -1073709010, -163880, -32750, 491512, -262082];
    for k in 0..9 { ta.send(av[k]); tb.send(bv[k]); }
    ta.close(); tb.close();
    for v in cv { assert rc.recv() == v; }
}
