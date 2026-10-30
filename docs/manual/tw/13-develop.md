# 13. 驗證與開發

這一章給想確認工具可信度、或想修改 rueda 本身的人。

## 怎麼知道工具是對的

| 檢查 | 證明什麼 | 在哪裡 |
|---|---|---|
| RTL 模擬 vs 官方 | `reda sim` 的語意與官方 `veryl test` / iverilog 一致 | 範例的 testbench |
| 閘級模擬 | 同一個 testbench 在通用閘、iCE40、vlib、SKY130 網表上都通過 | `cargo test`、`reda gatesim` |
| **形式等價** | 合成後的網表與官方 `veryl build` 產生的 SystemVerilog **數學上等價**（組合電路完整證明；循序電路 k-induction 或有界檢查） | `veryl/synth/test.sh`（yosys + slang） |
| SKY130 網表 | yosys 用 PDK 的 Liberty 函式讀元件，與 RTL 等價 | `veryl/synth/test.sh` |
| 位元流 | 自己的佈局繞線產生的 `.asc`，用 IceStorm 的 `icebox_vlog` 轉回電路，與網表隨機輸入逐週期比對；`.bin` 與 icepack 逐位元組相同 | `veryl/synth/fpga/pnr_test.sh`、`veryl/synth/fpga/cosim.py` |
| FPGA 流程 | RISC-V SoC 從對應後的網表、從繞好的位元流各模擬一次 | `veryl/synth/fpga/flow.sh` |
| 版圖 | DRC 0 違規；KLayout 讀回 GDS，檢查元件數、不重疊、寬度與間距 | `asic` 流程、`eda/tools/gds_check.py` |
| C 編譯器 | `riscv/rvtools/tests/*.c` 的輸出與主機的 `cc` 相同；RISC-V 程式的結果與 spike 相同 | `cargo test` |
| 虛擬板 | 板子引擎與 netsim 一致；位元流解碼與 nextpnr 的結果一致 | `cargo test board`、`cargo test bitstream` |
| 網頁 | 每個範例都用和網頁相同的 wasm + TypeScript API 跑過一遍 | `npm test` |

## 跑測試

```sh
./test.sh            # CI 跑的：整個 workspace 的 cargo test、reda sim、教學的流程、reda emu 的比對、
                     # Fluxa 的處理器、rvemu 的 JIT、npm test
./test.sh --full     # 再加上 veryl/synth/test.sh（需要 OSS CAD Suite，數十分鐘）
```

每一步失敗也會繼續跑，最後印出摘要。個別執行：

```sh
cargo test --release                            # 整個 workspace：eda、riscv、veryl、fluxa、reda
cargo test --release -p rueda-veryl-synth       # 合成所有範例、在網表上跑 testbench
cargo test --release -p rueda --test cli        # 只測 reda 的各個子指令
veryl/synth/test.sh                             # 形式等價 + FPGA 流程 + SKY130（含 GDS 檢查）
veryl/synth/fpga/pnr_test.sh                    # 自己的佈局繞線：每個設計的位元流 vs 網表 + icetime
cd web && npm test                              # 網頁（要先 npm run build）
```

`veryl/synth/test.sh` 用到的環境變數：

| 變數 | 用途 |
|---|---|
| `YOSYS_SLANG` | 有 slang 前端的 yosys（OSS CAD Suite 的）。Homebrew 的 yosys 讀不了 `veryl build` 輸出的 enum 轉型與 function |
| `GDS_PY` | 有 KLayout 模組的 Python（`pip install klayout`），用來檢查 SKY130 的 GDS；沒設定時若 `python3` 有就用它 |
| `OUT` | 輸出目錄（預設暫存目錄） |
| `VERYL_EDA_CHIPDB` | IceStorm 晶片資料庫；`./test.sh` 找不到時從 `web/dist/chipdb.js` 取出 |

等價檢查使用 `-set-init-zero`，因為最佳化器假設正反器上電為 0（iCE40 是這樣）。

## 新增測試設計

- 合成用的測試設計放在 `veryl/synth/examples/`，並加進 `veryl/synth/test.sh` 的 `DESIGNS`（格式 `檔名:模組:模式`，
  模式 `comb` / `ind` / `bmcN`）。這些檔案會被複製進官方 Veryl 專案產生標準答案，所以 module 名稱不能和 `veryl/examples/src` 重複。
- 用 `veryl check`（在 `veryl/examples/` 裡執行）確認語法；`reda` 的「not supported」不是語法錯誤，只是本專案沒支援。

## 原始碼地圖

rueda 是一個 Cargo workspace。閘級網表（`eda::netlist::Netlist`）是語言與後端的分界：前端（現在是 Veryl，之後是 Fluxa）
產生它，之後的流程都不知道原始語言是什麼。

| 目錄 | 內容 |
|---|---|
| `reda/` | `reda` 指令：每個工具一個子指令（`sim`、`synth`、`fpga`、`asic`、`gatesim`、`emu`、`cc`、`rvemu`、`pdk`）；語言由檔案的副檔名決定 |
| `eda/src/` | 與語言無關的後端（crate `rueda-eda`）：`netlist.rs` 閘級網表與最佳化、`bram.rs`、`output.rs`（yosys JSON / Verilog）、`netsim.rs`、`sdc.rs`、`power.rs` |
| `eda/src/fpga/` | `ice40.rs`、`ecp5.rs` 技術對應；`pnr/` 自己的 iCE40 佈局繞線與位元流；`board/` 虛擬開發板（引擎、iCEBreaker、位元流解碼、周邊） |
| `eda/src/asic/`、`eda/src/pdk/` | 標準元件流程（`map` `place` `route` `detail` `sta` `layout` `drc` `gds`）；Liberty / LEF / DEF 讀取與 SKY130 綁定；`eda/pdk/sky130hd/` 內建的子集 |
| `eda/bits/` | eda 與前端共用的固定寬度位元向量 |
| `eda/tools/` | SKY130 下載與子集、KLayout 的 GDS / LVS / 天線檢查 |
| `veryl/sim/` | Veryl 模擬器：`elab.rs` 展開、`sim.rs` 事件驅動模擬、`value.rs` 四值、`gates.rs` 閘級模型 |
| `veryl/synth/` | Veryl 合成：`elab.rs` 符號執行成 eda 的閘、`gatesim.rs`、`cli.rs`；`fpga/` 與 yosys / nextpnr 比對的流程、`test.sh` |
| `veryl/emu/` | 編譯式模擬機：設計編成 WebAssembly（`ir.rs`、`gen.rs`、`wasm.rs`）、`js/emu.mjs` |
| `veryl/examples/`、`veryl/tutorial/` | Veryl 範例（官方 Veryl 專案；`rv64/` RV64 處理器與 xv6 / Linux、`c/` C 程式、`sw/` 組合語言測試）；教學與虛擬實驗課 |
| `riscv/rvtools/` | C 編譯器、組譯器、RV32IMA 指令集模擬器 |
| `riscv/rvemu/` | RV64 開發板的指令集模擬機（含 WebAssembly JIT） |
| `fluxa/` | Fluxa 語言：`sim/` 模擬器（Kahn 程序網路直譯器）、`examples/`、`rv32i/`、`rv64/`、`docs/` |
| `web/` | IDE 與展示頁（TypeScript）；`web/wasm/` WebAssembly 核心（C ABI，JSON 進出）；`web/dist/` 建好的網頁（提交進 git） |
| `docs/manual/` | 本手冊 |

更多細節見 `AGENTS.md`、`web/PLAN.md`。

## 建置

```sh
./build.sh                 # reda + web/dist；改了 Rust 或 TypeScript 後重建 web/dist
./build.sh --no-web        # 只建 reda（target/release/reda）
```

文件有三種語言：`README.md`（英文）、`README.tw.md`（繁體）、`README.cn.md`（簡體，由 `tools/tocn.py` 從繁體轉換）。
網頁介面文字寫成 `tr('繁體', 'English')`；簡體由對照表自動產生。

CI（`.github/workflows/ci.yml`）跑 `./test.sh` 的內容，推到 main 時把新建的 `web/dist` 部署到 GitHub Pages。
