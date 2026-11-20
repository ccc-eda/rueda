# simt4: a tiny SIMT GPU core, simulation only

Four lanes execute the same instruction stream on their own registers (SIMT), with an active
mask for divergent branches, and share a 16-word memory.

Streams:
- `m` of `i32`: the initial memory, 16 words (address 0..15);
- `ins` of `(u8, u8, u8, u8, i32)`: instructions (op, rd, a, b, imm), see below;
- `y` of `i32`: after HALT, the 16 memory words in address order; then close `y`.

Each lane has registers r0..r7 (`i32`, all 0 at start). Only active lanes execute; all
lanes start active.

| op | name | effect (per active lane) |
|---|---|---|
| 1 | LI rd imm | rd = imm |
| 2 | LANE rd | rd = lane number 0..3 |
| 3 | ADD rd a b | rd = ra + rb |
| 4 | SUB rd a b | rd = ra − rb |
| 5 | MUL rd a b | rd = ra · rb |
| 12 | ANDI rd a imm | rd = ra & imm |
| 8 | SLT rd a b | rd = 1 if ra < rb else 0 |
| 6 | LD rd a imm | rd = mem[ra + imm] |
| 7 | ST a b imm (rd unused) | mem[ra + imm] = rb |
| 9 | IF a | push the mask; active = active and (ra ≠ 0) |
| 10 | ELSE | active = (pushed mask) and not active |
| 11 | ENDIF | pop the mask |
| 0 | HALT | dump memory on `y`, close it, stop |

Structure (required): one task per lane (spawned four times) holding its registers; a
front-end task that decodes `ins` and broadcasts each instruction to the lanes, keeps the
mask stack, and a memory task that serves the lanes' loads and stores (lanes in order 0..3
for the same instruction).

Task name: `Simt4` with parameters (m, ins, y).

Test: saxpy (y = 3·x + y over 4 elements, x at 0..3, y at 4..7) followed by a divergent
branch (odd lanes store lane·10, even lanes store −lane, at 8..11).
  m   = 2, -5, 7, 100, 1, 1, -1, -300, 0, 0, 0, 0, 0, 0, 0, 0
  ins = (2, 1, 0, 0, 0),
        (1, 2, 0, 0, 3),
        (6, 3, 1, 0, 0),
        (6, 4, 1, 0, 4),
        (5, 5, 2, 3, 0),
        (3, 5, 5, 4, 0),
        (7, 0, 1, 5, 4),
        (12, 6, 1, 0, 1),
        (9, 0, 6, 0, 0),
        (1, 7, 0, 0, 10),
        (5, 7, 7, 1, 0),
        (7, 0, 1, 7, 8),
        (10, 0, 0, 0, 0),
        (1, 7, 0, 0, 0),
        (4, 7, 7, 1, 0),
        (7, 0, 1, 7, 8),
        (11, 0, 0, 0, 0),
        (0, 0, 0, 0, 0)
  y   = 2, -5, 7, 100, 7, -14, 20, 0, 0, 10, -2, 30, 0, 0, 0, 0

Only `reda sim` has to pass (no synthesis).
