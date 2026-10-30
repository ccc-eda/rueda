// N×N 脈動矩陣乘法。PE 以阻塞式 recv 自然形成斜向波前，不需手寫 skew。
const N: u32 = 3;
const K: u32 = 5;
const A: [[i8; K]; N] = [[1, -2, 3, 4, -5], [6, 7, -8, 9, 10], [-11, 12, 13, -14, 15]];
const B: [[i8; N]; K] = [[1, 2, 3], [-4, 5, 6], [7, -8, 9], [10, 11, -12], [-13, 14, 15]];

task Pe(a_in: in chan<i8>, b_in: in chan<i8>,
        a_out: out chan<i8>, b_out: out chan<i8>,
        k: u16, c_out: out chan<i32>) {
    carry acc: i32 = 0;
    pipeline(ii = 1) for _ in 0..k {
        let a = a_in.recv();
        let b = b_in.recv();
        acc += a * b;
        a_out.send(a);
        b_out.send(b);
    }
    c_out.send(acc);
}

task FeedA(i: u32, tx: out chan<i8>) { for t in 0..K { tx.send(A[i][t]); } }
task FeedB(j: u32, tx: out chan<i8>) { for t in 0..K { tx.send(B[t][j]); } }
task Sink(rx: in chan<i8>) { for _ in rx { } }

test gemm_systolic {
    // ah[i*(N+1)+j]：進入 PE(i,j) 的 a；bv[i*N+j]：進入 PE(i,j) 的 b
    let ah = array(N * (N + 1), |_| chan::<i8>(depth = 2));
    let bv = array((N + 1) * N, |_| chan::<i8>(depth = 2));
    let cc = array(N * N, |_| chan::<i32>(depth = 1));

    for i in 0..N {
        for j in 0..N {
            spawn Pe(ah[i * (N + 1) + j].1, bv[i * N + j].1,
                     ah[i * (N + 1) + j + 1].0, bv[(i + 1) * N + j].0,
                     K, cc[i * N + j].0);
        }
    }
    for i in 0..N { spawn FeedA(i, ah[i * (N + 1)].0); spawn Sink(ah[i * (N + 1) + N].1); }
    for j in 0..N { spawn FeedB(j, bv[j].0);           spawn Sink(bv[N * N + j].1); }

    for i in 0..N {
        for j in 0..N {
            let mut s: i32 = 0;
            for t in 0..K { s += A[i][t] * B[t][j]; }
            assert cc[i * N + j].1.recv() == s, "C[i][j] mismatch";
        }
    }
}
