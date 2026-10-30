# 13. 验证与开发

这一章给想确认工具可信度、或想修改 rueda 本身的人。

## 怎么知道工具是对的

| 检查 | 证明什么 | 在哪里 |
|---|---|---|
| RTL 仿真 vs 官方 | `reda sim` 的语意与官方 `veryl test` / iverilog 一致 | 示例的 testbench |
| 门级仿真 | 同一个 testbench 在通用门、iCE40、vlib、SKY130 网表上都通过 | `cargo test`、`reda gatesim` |
| **形式等价** | 综合后的网表与官方 `veryl build` 产生的 SystemVerilog **数学上等价**（组合电路完整证明；循序电路 k-induction 或有界检查） | `veryl/synth/test.sh`（yosys + slang） |
| SKY130 网表 | yosys 用 PDK 的 Liberty 函数读单元，与 RTL 等价 | `veryl/synth/test.sh` |
| 比特流 | 自研布局布线产生的 `.asc`，用 IceStorm 的 `icebox_vlog` 转回电路，与网表随机输入逐周期比对；`.bin` 与 icepack 逐字节相同 | `veryl/synth/fpga/pnr_test.sh`、`veryl/synth/fpga/cosim.py` |
| FPGA 流程 | RISC-V SoC 从对应后的网表、从布好的比特流各仿真一次 | `veryl/synth/fpga/flow.sh` |
| 版图 | DRC 0 违规；KLayout 读回 GDS，检查单元数、不重叠、宽度与间距 | `asic` 流程、`eda/tools/gds_check.py` |
| C 编译器 | `riscv/rvtools/tests/*.c` 的输出与主机的 `cc` 相同；RISC-V 程序的结果与 spike 相同 | `cargo test` |
| 虚拟板 | 板子引擎与 netsim 一致；比特流解码与 nextpnr 的结果一致 | `cargo test board`、`cargo test bitstream` |
| 网页 | 每个示例都用和网页相同的 wasm + TypeScript API 跑过一遍 | `npm test` |

## 跑测试

```sh
./test.sh            # CI 跑的：整个 workspace 的 cargo test、reda sim、教程的流程、reda emu 的比对、
                     # Fluxa 的处理器、rvemu 的 JIT、npm test
./test.sh --full     # 再加上 veryl/synth/test.sh（需要 OSS CAD Suite，数十分钟）
```

每一步失败也会继续跑，最后打印摘要。单独运行：

```sh
cargo test --release                            # 整个 workspace：eda、riscv、veryl、fluxa、reda
cargo test --release -p rueda-veryl-synth       # 综合所有示例、在网表上跑 testbench
cargo test --release -p rueda --test cli        # 只测 reda 的各个子指令
veryl/synth/test.sh                             # 形式等价 + FPGA 流程 + SKY130（含 GDS 检查）
veryl/synth/fpga/pnr_test.sh                    # 自研布局布线：每个设计的比特流 vs 网表 + icetime
cd web && npm test                              # 网页核心与 IDE 逻辑（要先 npm run build）
cd web && npm run e2e                           # 在 Chrome 里端对端测 IDE（Playwright）：流程图、建议步骤、Fluxa
```

`veryl/synth/test.sh` 用到的环境变量：

| 变量 | 用途 |
|---|---|
| `YOSYS_SLANG` | 有 slang 前端的 yosys（OSS CAD Suite 的）。Homebrew 的 yosys 读不了 `veryl build` 输出的 enum 转型与 function |
| `GDS_PY` | 有 KLayout 模块的 Python（`pip install klayout`），用来检查 SKY130 的 GDS；没设置时若 `python3` 有就用它 |
| `OUT` | 输出目录（默认暂存目录） |
| `VERYL_EDA_CHIPDB` | IceStorm 芯片数据库；`./test.sh` 找不到时从 `web/dist/chipdb.js` 取出 |

等价检查使用 `-set-init-zero`，因为优化器假设触发器上电为 0（iCE40 是这样）。

## 添加测试设计

- 综合用的测试设计放在 `veryl/synth/examples/`，并加进 `veryl/synth/test.sh` 的 `DESIGNS`（格式 `檔名:模組:模式`，
  模式 `comb` / `ind` / `bmcN`）。这些文件会被拷贝进官方 Veryl 项目产生标准答案，所以 module 名称不能和 `veryl/examples/src` 重复。
- 用 `veryl check`（在 `veryl/examples/` 里运行）确认语法；`reda` 的「not supported」不是语法错误，只是本项目没支持。

## 源代码地图

rueda 是一个 Cargo workspace。门级网表（`eda::netlist::Netlist`）是语言与后端的分界：前端（现在是 Veryl，之后是 Fluxa）
产生它，之后的流程都不知道原始语言是什么。

| 目录 | 内容 |
|---|---|
| `reda/` | `reda` 指令：每个工具一个子指令（`sim`、`synth`、`fpga`、`asic`、`gatesim`、`emu`、`cc`、`rvemu`、`pdk`）；语言由文件的扩展名决定 |
| `eda/src/` | 与语言无关的后端（crate `rueda-eda`）：`netlist.rs` 门级网表与优化、`bram.rs`、`output.rs`（yosys JSON / Verilog）、`netsim.rs`、`sdc.rs`、`power.rs` |
| `eda/src/fpga/` | `ice40.rs`、`ecp5.rs` 工艺映射；`pnr/` 自研的 iCE40 布局布线与比特流；`board/` 虚拟开发板（引擎、iCEBreaker、比特流解码、外设） |
| `eda/src/asic/`、`eda/src/pdk/` | 标准单元流程（`map` `place` `route` `detail` `sta` `layout` `drc` `gds`）；Liberty / LEF / DEF 读取与 SKY130 绑定；`eda/pdk/sky130hd/` 内置的子集 |
| `eda/bits/` | eda 与前端共用的固定宽度位向量 |
| `eda/tools/` | SKY130 下载与子集、KLayout 的 GDS / LVS / 天线检查 |
| `veryl/sim/` | Veryl 仿真器：`elab.rs` 展开、`sim.rs` 事件驱动仿真、`value.rs` 四值、`gates.rs` 门级模型 |
| `veryl/synth/` | Veryl 综合：`elab.rs` 符号执行成 eda 的门、`gatesim.rs`、`cli.rs`；`fpga/` 与 yosys / nextpnr 比对的流程、`test.sh` |
| `veryl/emu/` | 编译式仿真机：设计编成 WebAssembly（`ir.rs`、`gen.rs`、`wasm.rs`）、`js/emu.mjs` |
| `veryl/examples/`、`veryl/tutorial/` | Veryl 示例（官方 Veryl 项目；`rv64/` RV64 处理器与 xv6 / Linux、`c/` C 程序、`sw/` 汇编测试）；教程与虚拟实验课 |
| `riscv/rvtools/` | C 编译器、汇编器、RV32IMA 指令集仿真器 |
| `riscv/rvemu/` | RV64 开发板的指令集仿真机（含 WebAssembly JIT） |
| `fluxa/` | Fluxa 语言：`sim/` 仿真器（Kahn 进程网络解释器）、`examples/`、`rv32i/`、`rv64/`、`docs/` |
| `web/` | IDE 与展示页（TypeScript）；`web/wasm/` WebAssembly 核心（C ABI，JSON 进出）；`web/dist/` 建好的网页（提交进 git） |
| `docs/manual/` | 本手册 |

更多细节见 `AGENTS.md`、`web/PLAN.md`。

## 建置

```sh
./build.sh                 # reda + web/dist；改了 Rust 或 TypeScript 后重建 web/dist
./build.sh --no-web        # 只建 reda（target/release/reda）
```

文档有三种语言：`README.md`（英文）、`README.tw.md`（繁体）、`README.cn.md`（简体，由 `tools/tocn.py` 从繁体转换）。
网页界面文字写成 `tr('繁體', 'English')`；简体由对照表自动产生。

CI（`.github/workflows/ci.yml`）跑 `./test.sh` 的内容，推到 main 时把新建的 `web/dist` 部署到 GitHub Pages。
