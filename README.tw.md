# rueda
[English](README.md) | **繁體中文**

用 Rust 寫的 IC 設計 EDA：硬體語言（Veryl、Fluxa）→ 模擬 → 合成 → FPGA 位元流或 ASIC 佈局，
再加上 RISC-V 軟體工具，以及在瀏覽器裡就能跑這一切的網頁 IDE。

**線上試用**（不用安裝）：[IDE](https://ccc-eda.github.io/rueda/web/dist/index.html) ·
使用手冊：[docs/manual/](docs/manual/README.md)

## 一個指令：`reda`

所有工具都是 `reda` 的子指令，像 `git` 一樣：

```sh
reda sim counter.veryl                 # 跑 testbench（Veryl）；語言由副檔名決定
reda sim fir.fx                        # 跑 Fluxa 程式的 test
reda synth -o adder.json adder.veryl   # 合成成閘級網表（yosys JSON / Verilog）
reda fpga --pnr veryl --pcf pins.pcf --bin top.bin top.veryl   # iCE40：對應、佈局繞線、位元流
reda asic --pdk sky130 --gds alu.gds alu.veryl                 # 標準元件一路做到通過 DRC 的 GDS
reda gatesim ice40 fifo.veryl          # 在合成後的網表上跑 testbench
reda emu --top Rv64Soc -o soc.wasm *.veryl   # 把設計編成 WebAssembly（快速模擬）
reda emu fir.fx                        # Fluxa 的 test，task 換成編譯後的版本（fluxa-emu）；-o：WebAssembly
reda cc --run hello.c                  # 給 RISC-V SoC 的 C 程式，在指令集模擬器上執行
reda rvemu fw_payload.bin              # RV64 開發板模擬機（OpenSBI + Linux）
reda help [子指令]
```

安裝：`cargo install rueda`，或從原始碼建置（見下）。

## 資料夾結構

閘級網表（`eda::netlist::Netlist`）是語言與後端的分界：語言前端產生它，之後的流程都不知道原始語言是什麼。

```
rueda/
├── reda/            reda 指令（每個工具一個子指令）
├── eda/             與語言無關的後端
│   ├── src/           閘級網表、block RAM、輸出格式、閘級模擬、時序約束、功耗
│   ├── src/fpga/      iCE40 / ECP5 對應、自己的 iCE40 佈局繞線與位元流、虛擬 iCEBreaker
│   ├── src/asic/      標準元件流程：擺置、時脈樹、繞線、時序、DRC / LVS、GDS
│   ├── src/pdk/       Liberty / LEF / DEF、SKY130 綁定；pdk/sky130hd：內建的元件
│   ├── bits/          與前端共用的位元向量
│   └── tools/         SKY130 下載、KLayout 的 GDS / LVS / 天線檢查
├── veryl/           Veryl 語言
│   ├── sim/           事件驅動模擬器
│   ├── synth/         Veryl → eda 的閘級網表、合成後模擬
│   ├── emu/           Veryl → WebAssembly（週期式，像 Verilator）
│   ├── examples/      設計範例、RISC-V SoC、RV64 開發板（xv6、Linux）、C 程式
│   └── tutorial/      從零開始的教學與虛擬實驗課
├── fluxa/           Fluxa 語言（高階合成：任務與通道，Kahn 程序網路）
│   ├── sim/           模擬器（多執行緒，或瀏覽器裡的協作式排程）
│   ├── synth/         Fluxa → eda 的閘級網表（task：狀態機 + 資料路徑；網路：一個網表）
│   ├── emu/           fluxa-emu：把 Fluxa 編譯成快速模擬（Rust 執行器、WebAssembly）
│   ├── examples/      FIR、脈動陣列…；rv32i/、rv64/：用 Fluxa 寫的處理器；
│   │                  rv64/linux/：用 Fluxa 寫的 RV64 開發板，開得了 Linux
│   └── docs/          語言規格與版本紀錄
├── riscv/           RISC-V 軟體
│   ├── rvtools/       C 編譯器、組譯器、RV32IMA 指令集模擬器
│   └── rvemu/         RV64IMAC 開發板模擬機（含 WebAssembly JIT）
├── web/             網頁 IDE（TypeScript）；wasm/：它的 WebAssembly 核心；dist/：建好的網頁
└── docs/manual/     使用手冊（English、繁體中文、简体中文）
```

| 語言 | 模擬 | 合成、FPGA、ASIC | 網頁 IDE |
|---|---|---|---|
| Veryl（`.veryl`） | `reda sim`、`reda emu` | `reda synth / fpga / asic / gatesim` | 有 |
| Fluxa（`.fx`） | `reda sim`、`reda emu` | `reda synth / fpga / asic / gatesim`（一個 task，或 task 組成的網路） | 有 |

兩種語言寫的硬體都開得了 Linux：Veryl 的 Rv64Soc（veryl-emu）和用 Fluxa 寫的同一塊板子（`fluxa/rv64/linux/soc.fx`，
fluxa-emu）開的是同一個 OpenSBI + Linux + busybox 映像；網頁 IDE 的「RV64 開發板」與「Fluxa 開發板」範例在上面跑
MicroPython、C 程式、跑馬燈、俄羅斯方塊、貪食蛇。各種模擬機有多快、為什麼：[docs/note/emu-compare.md](docs/note/emu-compare.md)。

## 建置與測試

需要 Rust；網頁另外需要 `wasm32-unknown-unknown` target 與 Node.js。

```sh
./build.sh             # target/release/reda 與 web/dist（--no-web：只建 reda）
./test.sh              # 整個 workspace 的 cargo test、教學流程、模擬機、Fluxa 處理器、npm test
./test.sh --full       # 再加上 yosys 形式等價、nextpnr 流程（需要 OSS CAD Suite）
```

用瀏覽器打開 `web/dist/index.html` 就是 IDE（不需要伺服器）。更多：[docs/manual/tw/13-develop.md](docs/manual/tw/13-develop.md)、
[AGENTS.md](AGENTS.md)。

## 來源

rueda 合併了兩個先前的專案：[veryl2chip](veryl/history.md)（Veryl 到晶片；版本歷史在 `veryl/history.md`）
與 Fluxa（`fluxa/docs/`）。

## 授權

MIT
