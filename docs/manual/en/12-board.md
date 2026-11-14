# 12. Virtual board

You can "flash" without an iCEBreaker: the IDE's ⚡ **Virtual board** draws an iCEBreaker (with the PMOD2 expansion board: 5 LEDs, 3 buttons),
puts your design on it and runs it — buttons can be pressed, LEDs light up, and you can attach a serial terminal, external modules, and a breadboard.
It runs in its own background thread and never blocks the editor.

## Three modes

| Mode | What runs | Speed | When to use |
|---|---|---|---|
| **Bitstream (real)** (default) | synthesis → its own place & route → `.bin`, then the bitstream is **decoded back into a circuit** and simulated cycle by cycle | real time for small designs, slower for large ones | check what would actually be flashed to a real chip (place & route included) |
| **Netlist (fast)** | the iCE40-mapped netlist, without place & route | much faster | editing the design, checking behaviour |
| **CPU (C program, real time)** | the project's `.c` runs on an instruction-set simulator, with the same peripherals, memory map, and cycles per instruction as `RvSoc` | real time at 12 MHz | C programs (the default for the "C programs" example) |

- After flashing, the status bar shows the virtual time and "× real time"; the speed menu can pick "no faster than real time" or "fastest", and you can pause.
- ⟲ Power cycle: start over (all registers back to 0, like a real chip).
- 📂 `.bin` (or drag a file onto the board): loads **any** iCE40UP5K bitstream, e.g. one made by `reda fpga --bin` or nextpnr + icepack.
- ⬇ .bin downloads the current bitstream, ready to be flashed to a real board with `iceprog`.

Made from the command line and run on the virtual board:

```sh
reda fpga --pnr veryl --pcf counter.pcf --bin counter.bin counter.veryl
```

Then drag `counter.bin` onto the virtual board.

## Controls

| Action | How |
|---|---|
| Press a button | hold it with the mouse; or keyboard `0` = BTN_N (the board's button), `1`–`3` = BTN1–BTN3 |
| Latch a button | Shift+click (click again to release) |
| Inspect pins | hover over a pin: which port of the design, the current level; pins the design "doesn't use" are marked too |

LED brightness follows the fraction of time lit, so PWM dimming and a multiplex-scanned seven-segment display look just like the real thing.

## Pins: 📌

Which pin each port of the design connects to is decided by the project's `pins.pcf` (format in chapter 7). The 📌 Pins dialog lists every port with a dropdown of pins;
"Apply and flash" writes it back to `pins.pcf` and flashes again. You can also edit `pins.pcf` directly in the editor.

## USB terminal 🖥

Wired to the FPGA's pin 6 (RX, computer → FPGA) and pin 9 (TX, FPGA → computer).

- Type and press Enter to send; what the program sends back appears above.
- Baud-rate menu: clocks per bit = 12 MHz ÷ baud rate. There is also "8 clocks per bit" for test designs (e.g. `UartLoop`).
- A "format error" usually means the baud rate doesn't match the design.

## PMOD modules

PMOD1A and PMOD1B each take one module:

| Module | Wiring |
|---|---|
| 8 LEDs | pins 1–4, 7–10 → LED 0–7 (active high) |
| 8 switches | click to toggle; pins 1–4, 7–10 = switch 0–7 |
| Seven-segment display (2 digits) | pins 1–4, 7–9 = segments a–g, pin 10 = digit select (multiplexed scan) |
| VGA monitor | the PmodVGA wiring, taking 1A + 1B; shows a 640×480 picture |
| Buzzer | a square wave on pin 1; it sounds only with 🔊 checked |

PMOD1A pins: 1–4 = 4, 2, 47, 45, 7–10 = 3, 48, 46, 44; PMOD1B: 1–4 = 43, 38, 34, 31, 7–10 = 42, 36, 32, 28.

## Breadboard 🧪

With 🧪 Breadboard checked, a breadboard appears next to the board, so you can wire the PMOD pins to your own circuit.

- **Parts**: wires, resistors, LEDs, buttons, switches.
- **Placing**: pick a tool, click two holes to place a part (for an LED click the anode first).
- **Deleting**: select then press Delete, or right-click.
- **Operating**: hold buttons, click switches.

It is a simplified circuit model:

- Copper strips, power rails, wires, pressed buttons, and closed switches join holes into the same node.
- Resistors provide a "weak" level (pull-up / pull-down).
- An LED lights when its anode is above its cathode.

Warnings are shown when:

- there is a short circuit (e.g. an FPGA output tied directly to GND);
- an LED has no series current-limiting resistor;
- an input pin floats (add a pull-up or pull-down resistor).

Pins already occupied by a PMOD module cannot be wired to the breadboard.

## Your own module: `.board` 🔌

When a project has a `.board` file, ⚡ Flash plugs it into the virtual board as **a PMOD module of your own design**:
its parts and nets are simulated as the file says, and a card next to the board shows it (LEDs, a temperature to drag,
a shift register's outputs, buttons, and the electrical rule check, ERC).
Example: "Lab: an I²C thermometer" (`veryl/examples/thermo.board`): an LM75 temperature sensor on I²C and a 74HC595
with 8 LEDs.

```text
board "PMOD thermometer"
part J1 pmod                     # connector: pins 1-4, 7-10 signals, 5 / 11 GND, 6 / 12 3V3
part U1 lm75 temp=23.5           # address 0x48 + A2A1A0
part R1 res 4.7k
net SCL J1.1:od U1.SCL R1.1      # :od = the FPGA drives it open drain (an output 1 lets go)
net 3V3 J1.6 R1.2 U1.VCC
net GND J1.5 U1.GND U1.A0 U1.A1 U1.A2
plug J1 icebreaker.PMOD1A        # which header it goes into
```

- **Parts**: `res`, `led`, `btn`, `pmod`, `lm75` (I²C temperature sensor), `74hc595` (shift register); `reda board --parts` lists their pins.
- **Levels**: anything pulling low makes 0 (open drain, an I²C ACK); else a push-pull 1 is 1; else a resistor decides (pull-ups, an LED's series resistor); else the net floats and the FPGA reads 1.
- **I²C**: the FPGA has no tri-state pins here, so SDA takes two pins: an output (`:od`, 0 = pull low, 1 = let go) and an input reading the same net back.
- **ERC**: outputs fighting, open drain without a pull-up, floating inputs, unconnected power pins, LEDs without a series resistor, connectors plugged into nothing. On the command line: `reda board thermo.board`.
- **🗺 Nets** (a button on the card): a floating window with two views to switch between: **Nets** (part boxes and their connections, the netlist) and **Schematic** (symbols for resistors, LEDs and buttons, right-angle wires, net labels (flags) for nets that reach several places, power symbols for 3V3 / GND); the choice is remembered. The nets view draws every part and the nets between them. Connectors on the left, ICs in the middle (inputs left, outputs right), resistors and LEDs on the right; 3V3 / GND are labels. While the board runs, each net takes its level's color (1 orange, 0 blue, floating dashed) and LEDs glow by their brightness; hovering a net shows its pins and level, and nets with an ERC problem are marked ⚠.
- The CPU mode (C programs) does not plug the `.board` module in yet.

This is the first step of the PCB plan (`docs/note/pcb-plan.md`): later the same `.board` exports a KiCad project and Gerbers.

## Sharing labs

A lab contains the files, `pins.pcf`, modules, and the breadboard. Two ways to share:

- **🔗 Share lab (make a link)**: the whole lab is compressed into the URL (`index.html#lab=…`); whoever opens it imports it as a new project — no server needed.
- **Export the lab file `.lab.json`**: open it with ⇪ Import.

`veryl/tutorial/labs.tw.md` is a course of 8 labs using the virtual board; each lab is a `#ex=…&board=1` link.

## Differences from a real board

- Bitstream mode simulates the logic and routing inside the bitstream, **not** analog effects: no glitches from delays, no metastability.
- Timing violations are not simulated: a design with Fmax below 12 MHz still works fine here but wouldn't on a real chip. Check the Fmax printed by `reda fpga`.
- Buttons don't bounce.
- The board drawing is a schematic, not a picture.
