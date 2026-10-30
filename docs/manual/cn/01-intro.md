# 1. 认识 rueda

## 它能做什么

把一个用 Veryl 写的数字电路设计，一路带到「可以烧的 FPGA」或「可以送去做的芯片版图」：

```
Veryl 源代码
   │  reda sim        仿真：跑 #[test] testbench，看波形
   ▼
门级网表          reda synth      综合：展开、优化成 AND / OR / XOR / MUX / 触发器
   │
   ├─► FPGA   reda fpga       iCE40UP5K：LUT 对应 → 布局 → 布线 → 时序 → 比特流（iceprog 烧进 iCEBreaker）
   │
   └─► ASIC   reda asic       标准单元对应 → 行式布局 → 时钟树 → 布线 → 时序 → 功耗
                              （vlib 教学单元库，或 SkyWater SKY130：再加 DRC 与 GDSII 版图）

综合后的网表 → reda gatesim   用同一个 testbench 再仿真一次，确认综合没有改变行为
C 程序       → reda cc        编译给项目里的 RISC-V SoC（也是用 Veryl 写的）运行
```

全部工具都是这个项目自己写的（仿真器、综合器、FPGA 布局布线与比特流、ASIC 后端、C 编译器），
需要外部工具的地方只有：刻录实体板子（`iceprog`）、选用的 nextpnr 布局布线，以及给开发者用的验证工具。

## 两种用法

| | 网页 IDE | 命令行 `reda` |
|---|---|---|
| 安装 | 不用，打开网页即可（[在线版](https://ccc-eda.github.io/rueda/web/dist/index.html)） | `cargo install --git https://github.com/ccc-eda/rueda rueda` |
| 适合 | 上课、实验、看每个阶段的图、虚拟开发板 | 自动化、大设计、集成到其他流程 |
| 能力 | 仿真、波形、综合各阶段的图、FPGA / ASIC（含 SKY130）、虚拟开发板、C 程序 | 同样的流程，输出文件（JSON、Verilog、`.asc`、`.bin`、DEF、GDS、VCD） |

两者用的是**同一份程序**（网页里跑的是编成 WebAssembly 的同一套 Rust 代码），结果相同。

## 名词

| 名词 | 意思 |
|---|---|
| 顶层（top） | 要仿真或综合的那个 module |
| testbench | 带 `#[test(...)]` 属性、用来测试设计的 module；仿真时默认跑全部 testbench，综合时默认不碰它们 |
| 网表（netlist） | 综合后由逻辑门与触发器组成的电路 |
| LUT / LC | iCE40 FPGA 的 4 输入查表 / 逻辑单元（LUT + 进位 + 触发器） |
| 比特流（bitstream） | 烧进 FPGA 的设置档（`.bin`） |
| 标准单元（standard cell） | ASIC 用的现成逻辑门版图（NAND2、DFF…） |
| PDK | 制程设计套件：单元库、各层金属规则、版图（本项目支持 SkyWater SKY130） |
| Fmax | 电路能跑的最高时钟频率；iCEBreaker 的时钟是 12 MHz |
| DRC | 设计规则检查：金属宽度、间距、短路、断路 |
| GDSII | 芯片版图的标准文件格式（`.gds`） |

## 这个项目里有什么

| 目录 | 内容 |
|---|---|
| `reda/` | `reda` 指令：每个工具都是它的子指令（`reda sim`、`reda synth`、`reda fpga`…） |
| `eda/` | 与语言无关的后端：门级网表、FPGA 映射、布局布线与比特流、ASIC 流程、虚拟开发板 |
| `veryl/` | Veryl 语言：仿真器（`sim/`）、综合（`synth/`）、编译式仿真机（`emu/`）、示例、教程 |
| `fluxa/` | Fluxa 语言（高层次综合：任务与通道）：仿真器、示例、RV32I / RV64 处理器 |
| `riscv/` | RISC-V 软件工具：C 编译器、汇编器、指令集仿真器（`rvtools/`），RV64 开发板仿真机（`rvemu/`） |
| `web/` | 网页 IDE（`web/dist/index.html`）与它的 WebAssembly 核心（`web/wasm/`） |
| `veryl/examples/` | 示例设计（加法器、状态机、UART、VGA、RISC-V…）与 C 程序（`veryl/examples/c/`） |
| `veryl/tutorial/` | 从零开始的教程与虚拟实验课 |
| `docs/manual/` | 本手册 |
