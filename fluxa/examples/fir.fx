// 16 階低通 FIR：係數於編譯期由視窗法產生並量化為 fixed<1,15>
const TAPS: u32 = 16;

const COEF: [fixed<1, 15>; TAPS] = comptime {
    design_lowpass(taps = TAPS, cutoff = 0.2, window = hamming)
        .quantize::<fixed<1, 15, round = nearest>>()
};

task Fir(x: in chan<i16>, y: out chan<i16>) {
    let mut hist: [i16; TAPS - 1] @reg = [0; TAPS - 1];

    pipeline(ii = 1) for s in x {
        let win: [i16; TAPS] = concat([s], hist);          // win[0] 是最新樣本
        let acc = (0..TAPS).unrolled()
                           .map(|k| win[k] * COEF[k])      // i16 * fixed<1,15>
                           .tree_sum();                    // 加法樹
        hist = win[0..TAPS - 1];                           // 迭代結束時提交
        y.send(acc.to::<i16>(round = nearest, ovf = sat));
    }
}

test fir_impulse {
    // 輸出 FIFO 必須能容納整段回應，否則測試端先送完輸入會與 Fir 互相等待（真正的 KPN 死結）
    let (tx, rx) = chan::<i16>(depth = 4);
    let (ty, ry) = chan::<i16>(depth = 32);
    spawn Fir(rx, ty);

    tx.send(16384);
    for _ in 0..TAPS { tx.send(0); }
    tx.close();

    // 單位脈衝響應 = 係數本身
    for k in 0..TAPS {
        assert ry.recv() == (COEF[k] * 16384).to::<i16>(round = nearest, ovf = sat);
    }
}

test fir_dc_gain {
    // 係數總和 ≈ 1，直流輸入穩態輸出 ≈ 輸入
    let (tx, rx) = chan::<i16>(depth = 4);
    let (ty, ry) = chan::<i16>(depth = 64);
    spawn Fir(rx, ty);
    for _ in 0..40 { tx.send(1000); }
    tx.close();
    let mut last: i16 = 0;
    for v in ry { last = v; }
    assert last >= 998 && last <= 1002, "DC gain off";
}
