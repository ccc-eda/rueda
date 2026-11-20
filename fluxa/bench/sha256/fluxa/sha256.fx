// SHA-256 compression and a Bitcoin SHA-256d miner.
//
// A word in the header stream contains four header bytes in big-endian order.
// The nonce is the exception: its four bytes are serialized little-endian
// before the header is hashed.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2
];

fn add32(a: u32, b: u32) -> u32 {
    let mut result: u32 = a;
    result = result +% b;
    result
}

fn rotr(x: u32, n: u32) -> u32 {
    ((x >> n) | (x << (32 - n))).wrap::<u32>()
}

fn shr(x: u32, n: u32) -> u32 {
    (x >> n).wrap::<u32>()
}

// The SHA-256 compression function for one 512-bit block.
fn compress(block: [u32; 16]) -> [u32; 8] {
    let mut w: [u32; 64] = [0; 64];

    for i in 0..16 {
        w[i] = block[i];
    }
    for i in 16..64 {
        let x: u32 = w[i - 15];
        let y: u32 = w[i - 2];
        let s0: u32 = (rotr(x, 7) ^ rotr(x, 18) ^ shr(x, 3)).wrap::<u32>();
        let s1: u32 = (rotr(y, 17) ^ rotr(y, 19) ^ shr(y, 10)).wrap::<u32>();
        w[i] = add32(add32(s0, w[i - 16]), add32(w[i - 7], s1));
    }

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19
    ];

    let mut a: u32 = h[0];
    let mut b: u32 = h[1];
    let mut c: u32 = h[2];
    let mut d: u32 = h[3];
    let mut e: u32 = h[4];
    let mut f: u32 = h[5];
    let mut g: u32 = h[6];
    let mut hh: u32 = h[7];

    for t in 0..64 {
        let big_s1: u32 = (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)).wrap::<u32>();
        let ch: u32 = ((e & f) ^ ((~e) & g)).wrap::<u32>();
        let t1: u32 = add32(add32(add32(add32(hh, big_s1), ch), K[t]), w[t]);
        let big_s0: u32 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)).wrap::<u32>();
        let maj: u32 = ((a & b) ^ (a & c) ^ (b & c)).wrap::<u32>();
        let t2: u32 = add32(big_s0, maj);

        hh = g;
        g = f;
        f = e;
        e = add32(d, t1);
        d = c;
        c = b;
        b = a;
        a = add32(t1, t2);
    }

    h[0] = add32(h[0], a);
    h[1] = add32(h[1], b);
    h[2] = add32(h[2], c);
    h[3] = add32(h[3], d);
    h[4] = add32(h[4], e);
    h[5] = add32(h[5], f);
    h[6] = add32(h[6], g);
    h[7] = add32(h[7], hh);
    h
}

// SHA-256("abc").  The one-block padded representation of "abc" is
// 61626380 followed by zeroes and the final 24-bit bit length.
fn sha256_abc() -> [u32; 8] {
    let mut block: [u32; 16] = [0; 16];
    block[0] = 0x61626380;
    block[15] = 24;
    compress(block)
}

// Serialize a nonce in little-endian byte order as one big-endian SHA word.
fn nonce_word(nonce: u32) -> u32 {
    (
        ((nonce & 0x000000FF) << 24) |
        ((nonce & 0x0000FF00) << 8) |
        ((nonce & 0x00FF0000) >> 8) |
        ((nonce & 0xFF000000) >> 24)
    ).wrap::<u32>()
}

// SHA-256d of an 80-byte Bitcoin header.  header[0..19] are the first 19
// big-endian words and header[19] is the nonce word.
fn sha256d_header(header: [u32; 20]) -> [u32; 8] {
    // The 80-byte header fills block zero.  SHA-256 padding adds 0x80 at
    // byte 64 and the 640-bit message length in the last eight bytes.
    let mut first: [u32; 16] = [0; 16];
    for i in 0..16 {
        first[i] = header[i];
    }
    first[15] = 0x00000280;

    // The second SHA-256 hashes the first 32-byte digest.
    let digest: [u32; 8] = compress(first);
    let mut second: [u32; 16] = [0; 16];
    for i in 0..8 {
        second[i] = digest[i];
    }
    second[8] = 0x80000000;
    second[15] = 0x00000100;
    compress(second)
}

// The input consists of 19 header words, start nonce, and count.  On success
// y contains the nonce followed by the eight digest words.  Otherwise it
// contains 0xffffffff.
task Miner(hdr: in chan<u32>, y: out chan<u32>) {
    let mut header: [u32; 20] = [0; 20];
    for i in 0..19 {
        header[i] = hdr.recv();
    }

    let start: u32 = hdr.recv();
    let count: u32 = hdr.recv();
    let mut nonce: u32 = start;
    let mut found: bool = false;

    for _ in 0..count {
        header[19] = nonce_word(nonce);
        let hash: [u32; 8] = sha256d_header(header);

        if hash[7] == 0 {
            y.send(nonce);
            for i in 0..8 {
                y.send(hash[i]);
            }
            found = true;
            break;
        }

        nonce = (nonce + 1).wrap::<u32>();
    }

    if !found {
        y.send(0xFFFFFFFF);
    }
    y.close();
}

test sha256_abc {
    let got: [u32; 8] = sha256_abc();
    let expected = [
        0xba7816bf, 0x8f01cfea, 0x414140de, 0x5dae2223,
        0xb00361a3, 0x96177a9c, 0xb410ff61, 0xf20015ad
    ];
    for i in 0..8 {
        assert got[i] == expected[i];
    }
}

test bitcoin_genesis {
    let (tx, rx) = chan::<u32>(depth = 32);
    let (ty, ry) = chan::<u32>(depth = 16);
    spawn Miner(rx, ty);

    let header = [
        0x01000000, 0x00000000, 0x00000000, 0x00000000,
        0x00000000, 0x00000000, 0x00000000, 0x00000000,
        0x00000000, 0x3ba3edfd, 0x7a7b12b2, 0x7ac72c3e,
        0x67768f61, 0x7fc81bc3, 0x888a5132, 0x3a9fb8aa,
        0x4b1e5e4a, 0x29ab5f49, 0xffff001d
    ];
    for word in header {
        tx.send(word);
    }
    tx.send(2083236873);
    tx.send(30);
    tx.close();

    let expected = [
        2083236893,
        0x6fe28c0a, 0xb6f1b372, 0xc1a6a246, 0xae63f74f,
        0x931e8365, 0xe15a089c, 0x68d61900, 0x00000000
    ];
    for i in 0..9 {
        assert ry.recv() == expected[i];
    }
}
