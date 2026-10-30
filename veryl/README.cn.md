# rueda 里的 Veryl
[English](README.md) | [繁體中文](README.tw.md) | **简体中文**

这是 [rueda](../README.tw.md) 里原本叫 veryl2chip 的部分；它原本包含、与语言无关的后端现在在 [`eda/`](../eda/)。

**在线直接使用**（免安装）：[IDE](https://ccc-eda.github.io/rueda/web/dist/index.html)

**从零开始的教学**（课程教材）：[写 Veryl → 仿真 → 看波形 → 综合 → 烧进 iCEBreaker](tutorial/README.tw.md)；**虚拟实验课**（不用买板子）：[虚拟 iCEBreaker 上的 8 个实验](tutorial/labs.tw.md)

Veryl HDL 的 EDA 工具：仿真、逻辑综合、FPGA 流程（iCE40UP5K，一路到比特流）与 ASIC 流程
（标准单元、布局、布线、时序分析），全部用 Rust 写成。
从 v2.0 起，ASIC 流程也能用 SkyWater 的开源 **SKY130** PDK（`sky130_fd_sc_hd`），一路做到通过 DRC 的 GDSII 版图
（[说明](synth/README.tw.md)）。v2.2 起 FPGA 流程也能对应到 Lattice **ECP5**（ULX3S），布局布线交给 nextpnr-ecp5（`reda fpga --arch ecp5`）。[`veryl/examples/rv64/`](examples/rv64/README.md) 是用 Veryl 写的 RV64IMAC 处理器（M / S / U 模式、SV39），能启动 **xv6** 与 **Linux**（用官方 `veryl build` + Verilator 验证）。各版本的历史：[history.md](history.md)。使用手册：[docs/manual/](../docs/manual/README.md)（命令行一律用 `reda`）。

| | |
|---|---|
| [`veryl/sim`](sim/) | Veryl testbench 的事件驱动仿真器（`$tb::clock_gen`、`$assert`、VCD） |
| [`veryl/synth`](synth/) | Veryl → 门级网表；它驱动的后端（iCE40 与 ECP5 映射、自研布局布线 + 比特流、ASIC 流程）在 [`eda/`](../eda/) |
| [`veryl/emu`](emu/) | 把设计编译成 WebAssembly 做快速的周期仿真（像 Verilator，在 Node 或浏览器里跑）：RV64 SoC 在它上面约 2 MHz 开 Linux |
| [`web/wasm`](../web/wasm/) | 两个 crate 编译成 WebAssembly，以 JSON API 调用 |
| [`web/`](../web/) | 纯前端的网页环境（TypeScript） |
| [`veryl/examples/`](examples/) | 示例设计的 Veryl 项目，含一颗单周期 RV32I CPU |

## 网页环境
用浏览器打开 `web/dist/index.html` 就是 VS Code 风格的 IDE，不需要服务器、不需要安装，从 `file://` 开也能用。

- 活动列与侧栏：项目文件与模块大纲、示例库、测试、说明
- 分页编辑器：Veryl 语法上色、即时语法检查（问题面板、行号旁标记、波浪底线）
- 项目存在浏览器：可有多个项目，添加 / 改名 / 删除 / 导入文件，导出 `.zip`
- ▶ 测试（RTL 或门级）；✔ 全面验证：每个 testbench 都在 RTL 与三种综合后网表上跑一遍
- 综合：标题列的后端（**FPGA**、**ASIC · vlib** 或 **ASIC · SKY130**：SkyWater 的开源 PDK，有 DRC，可下载 GDS）与**阶段**菜单（① RTL→Logic → ② Logic Netlist → ③ Tech-indep Syn，接着 FPGA ④–⑦
  或 ASIC ④–⑨，最后一项是 FPGA vs ASIC 比较），选了就在「综合流程」分页显示该阶段；分页只有一行面包屑与图标按钮
  （ⓘ 阶段说明、连接、退火动画、⬇ 下载、缩放），数字在状态栏
- 下方面板（问题、输出、测试结果、波形）、状态栏、命令菜单（Ctrl/⌘+Shift+P）、快速开档（Ctrl/⌘+P）
- ⚡ **虚拟 iCEBreaker**：把设计刻录到画出来的开发板上，LED 会亮（亮度依工作周期）、按钮可以按，引脚和真实板相同（跑的是布局布线后真正的比特流：解码回电路、逐周期仿真，小设计约真实速度的 0.3–0.4 倍；任何 iCE40UP5K 的 `.bin`，例如 nextpnr 产生的，都可以拖放上去）；可以接上 USB 序列终端机与 PMOD 模块（8 颗 LED、8 个开关、两位数七段显示器、VGA 屏幕、蜂鸣器）与**面包板**（LED、电阻、按钮、开关、导线接在 PMOD 引脚上；短路、没有限流电阻、输入浮接都会警告）；引脚表会写入项目的 `pins.pcf`；整个实验（文件、引脚、模块、面包板）可以用一个链接分享
- **在你自己的 RISC-V 上跑 C**：`RvSoc`（`veryl/examples/src/rvsoc.veryl`，多周期 RV32IMA + Zicsr、有中断，寄存器堆与 8 KB 存储器都在 block RAM，加上 LED、按钮、开关、UART、计时器）跑在浏览器里由 veryl/synth 自己的编译器（`src/rv/`：C 子集、汇编器、指令集仿真器）编出来的 C 程序；⚡ 把程序放进 block RAM（只改程序时不用重新综合、布局布线），虚拟板的 CPU 模式以即时速度运行，门级 SoC 约真实的 1%；还有 VGA 文本模式（`RvVgaBoard`，40 × 30 字符）、用 C 写的抢占式多线程内核（计时器中断、ecall、原子指令）；示例在「C 程序」群组
- 🖥 **仿真机**：放不进 FPGA、也不适合 ASIC 的大设计：veryl/emu 把设计编成 WebAssembly，在 worker 里当成一台机器跑，UART 接到终端（项目的 `machine.json` 说明引脚）；示例 **RV64 + Linux** 在浏览器里开 OpenSBI + Linux + busybox（每秒约 370 万拍，约 1 分钟开到 shell）
- 接口可用繁体中文、简体中文或英文（标题列的菜单切换；默认依浏览器语言）

`index.html#ex=fifo&stage=place` 会打开某个示例并跳到某个阶段；加上 `&run=1` / `&verify=1` 会直接开始测试，`&board=1` 会刻录到虚拟板。
IDE 以前是 `ide.html`：那一页现在会（带着 `#…`）转到 `index.html`，旧链接照样能用。

网址的 hash 可以指定示例与阶段，例如 `index.html#ex=soc&synth=1&target=asic`。

在项目根目录一次建置、一次测试全部：

```sh
./build.sh          # veryl/sim、veryl/synth（release）與 web/dist（wasm + 網頁）；--no-web 只建 Rust 工具
./test.sh           # veryl/sim、veryl/synth 的 cargo test、教學流程、npm test（和 CI 相同）
./test.sh --full    # 再加 veryl/synth/test.sh：用 yosys 做形式等價、FPGA 流程（需要 OSS CAD Suite）
```

从 crates.io 安装命令行工具（`reda`、`veryl/sim`、`veryl/synth`）：

```sh
cargo install --git https://github.com/ccc-eda/rueda rueda
```

重新建置（需要 Rust + wasm32 target、Node.js）：

```sh
cd web
npm install
npm run build
npm test
```

`npm run build` 重建 `web/dist`。`npm test` 用建好的 `dist/` 把每个示例跑过一遍：仿真、综合、FPGA 布局布线
（`.asc` 与 CLI 相同，`.bin` 与 icepack 相同）、ASIC 流程（结果与 CLI 相同）、每个示例的门级仿真、
解开导出的项目 `.zip` 并以 `veryl test` 检查，以及接口翻译。

## 命令行：`reda`
所有功能一个指令，像 git 一样用子命令（`reda help`、`reda help <子命令>` 或 `reda <子命令> -h` 看选项）：

| | |
|---|---|
| `reda sim [--top M] [--vcd F] 檔案...` | 跑 `#[test]` testbench（`$assert` 失败时结束码为 1） |
| `reda list 檔案...` | 列出模块，标出哪些是 testbench |
| `reda synth [-o OUT.json] [--verilog OUT.v] 檔案...` | 门级网表（yosys JSON、Verilog）与统计 |
| `reda fpga [--pcf P] [--asc A] [--bin B] [--pnr veryl] 檔案...` | iCE40UP5K：LUT 对应、布局布线、比特流 |
| `reda asic [--pdk vlib\|sky130] [--def D] [--gds G] 檔案...` | 标准单元、布局、布线、时序、功耗、DRC（SKY130）、GDSII |
| `reda fpga --arch ecp5 [--lpf L] [--bit B] 檔案...` | ECP5：LUT4 / CCU2C 对应，nextpnr-ecp5 + ecppack |
| `reda gatesim opt\|ice40\|ecp5\|asic\|sky130 檔案...` | 在综合后的网表上跑 testbench |
| `reda cc PROG.c [-o PROG.bin] [--run]` | 给 RISC-V SoC 的 C（或汇编语言）；`--run` 在指令集仿真器上运行 |
| `reda pdk [目錄]` | 检查完整的 SKY130 PDK 目录 |

```sh
reda sim --vcd wave.vcd veryl/tutorial/src/blinky.veryl
reda fpga --pnr veryl --pcf veryl/tutorial/icebreaker.pcf --bin blinky.bin veryl/tutorial/src/blinky.veryl
reda asic --pdk sky130 --gds alu.gds veryl/examples/src/rv32i.veryl --top Alu
```

`reda` 随 `rueda` 安装（`cargo install --git https://github.com/ccc-eda/rueda rueda`，或 `./build.sh`：`target/release/reda`）；
原本的 `veryl/sim` 与 `veryl/synth`（`reda synth --ice40`、`--asic`、`--cc`…）照旧可用。

指令与架构见 [AGENTS.md](AGENTS.md)；各 crate 的 README（[veryl/sim](sim/README.tw.md)、[veryl/synth](synth/README.tw.md)）有更多细节。
