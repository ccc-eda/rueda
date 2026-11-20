# crc32: CRC-32 of a byte stream

Input stream `x` of `u8`, output stream `y` of `u32`. Compute the CRC-32 (IEEE 802.3, the one
of zlib: reflected, polynomial 0xEDB88320, initial value 0xFFFFFFFF, final XOR 0xFFFFFFFF)
of all bytes received until `x` closes; then output the CRC and close `y`.
Throughput goal: one byte per clock.

Task / module name: `Crc32`.

Test vectors (two separate runs, i.e. two tests or two resets):
  x = the ASCII bytes of "123456789"        → y = 0xCBF43926
  x = 0x00, 0xFF, 0x10, 0x20                → y = 0xEE70D968
