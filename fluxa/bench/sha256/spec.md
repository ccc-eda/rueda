# sha256: a SHA-256d miner (Bitcoin), simulation only

Write a SHA-256 compression function and a miner around it.

1. `sha256` of a message: test it on the ASCII bytes "abc" →
   ba7816bf 8f01cfea 414140de 5dae2223 b00361a3 96177a9c b410ff61 f20015ad
2. The miner task `Miner`: input stream `hdr` of `u32` gives the first 19 big-endian words
   of an 80-byte Bitcoin block header (76 bytes), then a start nonce and a count. For each
   nonce n from start to start + count - 1 it builds the 80-byte header (the nonce is
   appended as 4 bytes **little-endian**), computes SHA-256(SHA-256(header)), and checks
   whether the last 4 bytes of the 32-byte hash are zero (the leading zeros of the displayed,
   byte-reversed hash). Output stream `y` of `u32`: for the first nonce that passes, send the
   nonce and then the 8 big-endian words of the hash; if none passes, send 0xFFFFFFFF. Then
   close `y`.

Use wrapping 32-bit arithmetic explicitly (`+%`, `.wrap::<u32>()`): rotations and sums of
SHA-256 overflow 32 bits by design.

Test vector: the Bitcoin genesis block.
  hdr words = 0x01000000, 0, 0, 0, 0, 0, 0, 0, 0, 0x3ba3edfd, 0x7a7b12b2, 0x7ac72c3e,
              0x67768f61, 0x7fc81bc3, 0x888a5132, 0x3a9fb8aa, 0x4b1e5e4a, 0x29ab5f49, 0xffff001d
  start = 2083236873 (the answer minus 20), count = 30
  y = 2083236893,
      0x6fe28c0a, 0xb6f1b372, 0xc1a6a246, 0xae63f74f, 0x931e8365, 0xe15a089c, 0x68d61900, 0x00000000

Only `reda sim` has to pass (no synthesis). The simulator is an interpreter: keep the test to
this one search (21 double hashes).
