# Common rules (both languages)

## Stream interface (hardware)

Every channel / stream `c` is four signals: `c_data`, `c_valid`, `c_ready`, `c_close`.
A token moves on a rising clock edge when `c_valid && c_ready`. A token with `c_close = 1`
is the end-of-stream marker (its data is ignored); after it no more tokens come.
Inputs (`x`, `a`, `b`): the module drives `x_ready`. Outputs (`y`): the module drives
`y_data`, `y_valid`, `y_close` and must hold them until `y_ready`.
Every module also has `clk` (clock) and `rst` (synchronous reset, active high).

## Fluxa

The module is a `task` with channel parameters; `reda synth --top NAME FILE.fx` turns every
channel parameter `c` into the four signals above. Write the task and a `test` block that
feeds the test vectors and `assert`s every expected output.

## Veryl

Write the module (ports: `clk: input clock`, `rst: input reset`, then the stream signals
named as above) and a `#[test(...)]` testbench module
in the same file that instantiates it, drives the test vectors with the valid/ready
handshake, and `$assert`s every expected output.
