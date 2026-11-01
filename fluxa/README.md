# Fluxa — 專門為 HLS（高階綜合）電路設計的高階硬體設計語言

Fluxa 的核心哲學必須徹底擺脫「軟體思維（馮紐曼架構的順序執行）」的束縛，轉而擁抱「空間思維（Spatial Computing / Dataflow）」。

Fluxa **「融合了 Rust 的嚴謹型別、Go 的通道並行語意、以及硬體描述精確度的全新混合體」**。它不再試圖讓你在寫「軟體」，而是提供一個既有軟體的抽象便利性、又能完全掌控硬體空間與時間（時脈）的折衷點。

Fluxa 是 [rueda](../README.md) 的語言之一：`reda sim` / `reda list` 遇到 `.fx` 檔就用這裡的模擬器，網頁 IDE 也可以開 Fluxa 專案。
這裡是一個 Fluxa 的模擬器，像 `iverilog` / `vvp` 之於 Verilog：這是 Fluxa 語言（見 [〈Fluxa 語言規格 v0.1〉](docs/Fluxa語言規格v0.1.md)）的直譯器與模擬器，零外部依賴，純 Rust。


## 建置與執行

```
cargo build --release -p rueda                # 建出 reda（在 rueda 的根目錄；或 cargo install，見根目錄的 README）
reda sim examples/fir.fx                      # 跑檔案內所有 test（語言由副檔名 .fx 決定）
reda sim examples/gemm.fx --stats             # 另外印出每條通道的 token 數與 FIFO 最大佔用
reda sim examples/mac.fx --trace              # 印出每一次 send / recv
reda sim examples/fir.fx --test fir_impulse
reda sim examples/fir.fx --coop               # 用瀏覽器裡的單執行緒排程器跑
reda list examples/fir.fx
cargo test --release -p rueda-fluxa           # 完整回歸測試：examples + rv32i + rv64，兩種排程各跑一次（含 100 次死結偵測重複測試）
./test.sh                                     # 建置 + cargo test + 兩個處理器腳本，一次跑完
```

多檔載入：`fluxa a.fx b.fx` 會把多個檔案合併成一個程式（類似 `iverilog a.v b.v`），fn 名稱不可重複。

結束碼：全部通過 0，有失敗 1，載入/語法錯誤 2。

## 合成

`fluxa/synth`（crate `rueda-fluxa-synth`）把 task 合成成和 Veryl 一樣的閘級網表，後面接 rueda 的 FPGA / ASIC 流程：

```
reda synth --top Fir examples/fir.fx           # 網表；-o 寫 yosys JSON、--verilog 寫 Verilog
reda fpga --asc fir4.asc --bin fir4.bin examples/fir4.fx    # iCE40 位元流
reda asic --pdk sky130 --gds fir.gds examples/fir.fx        # SKY130 版圖
reda gatesim ice40 examples/fir.fx             # test 照跑，task 換成它的網表
cargo test --release -p rueda-fluxa-synth      # 合成與閘級共模擬的回歸測試
```

task 變成狀態機加資料路徑，通道變成 valid / ready 串流埠；`pipeline(ii = 1)` 一個時脈一次迭代；
`mem` 在 iCE40 上變成 block RAM；建網路的 task 整個變成一個網表（內部通道是 FIFO）。
細節見手冊第 17 章與 [`docs/synthesis-plan.md`](docs/synthesis-plan.md)。

## fluxa-emu：編譯後再跑

這個模擬器是直譯器，RV64 上每秒約一萬個指令。`fluxa/emu`（crate `rueda-fluxa-emu`）像 veryl-emu 一樣先編譯：
task 照合成的切法變成狀態，運算依值域放進 64 / 128 位元暫存器，通道是環形緩衝區，產生 WebAssembly（或在 Rust
執行器上跑）。結果必須和本模擬器相同。

```
reda emu examples/fir.fx                       # test 照跑，task 換成編譯後的版本
reda emu -o core.wasm --top Core --param WBD=8 ... rv64/rv64.fx rv64/fpu.fx   # WebAssembly 模組 + JSON（emu/js/emu.mjs）
cargo test --release -p rueda-fluxa-emu        # 每個範例、rv32i、rv64 的 test：與本模擬器逐個比對
emu/tests/wasm.sh                              # WebAssembly 重播 Rust 執行器的輸入，輸出必須相同
```

設計與量測見 [`docs/emu-plan.md`](docs/emu-plan.md)；三種模擬機的速度比較見 [`../docs/note/emu-compare.md`](../docs/note/emu-compare.md)。

## 模擬模型

- 每個 `task` 是一個獨立執行的程序，通道是有界 FIFO（`depth`）：滿了 `send` 阻塞、空了 `recv` 阻塞。
  這是 Kahn 程序網路，輸出 token 序列與排程、FIFO 深度無關（規格 §3.10 性質 5）。
- 兩種排程，結果相同：命令列預設每個 task 一條 OS 執行緒；`--coop` 與網頁（WebAssembly 沒有執行緒）
  則把所有 task 放在同一條執行緒上輪流跑（直譯器是 async 的，通道操作是讓出點）。回歸測試兩種都跑並比對。
- **死結偵測**：所有存活任務都在等待時，報告每個任務卡在哪條通道。
  深度不足造成的死結（例如測試端不讀輸出、FIFO 填滿）會被如實報出。
- **通道端點是線性資源**：`spawn` 會轉移端點所有權，之後再用會報錯。
  任務結束時自動關閉它仍擁有的 `Tx`（end-of-stream），`for x in rx` 因此會自然結束。
- **位寬安全（動態檢查）**：運算以精確的 120 位元整數進行，在每個有型別的落點
  （`let` / `carry` / 參數 / 通道元素 / 陣列元素）檢查範圍，溢位即報錯（對應規格 E0301）。
  窄化必須顯式：`as`、`.to::<T>(round=, ovf=)`、`.sat::<T>()`、`.wrap::<T>()`。
- **`@reg` 語意**：迭代內讀到舊值，寫入在迭代結束提交，所以 `unroll` 內的移位暫存器寫法與硬體一致。
- 定點數 `fixed<I,F>`：精確算術（加法對齊小數位、乘法小數位相加），捨入 `trunc|nearest|even`，溢位 `wrap|sat|check`。

## 支援的語言子集

`const`、`fn` / `comb fn` / `comptime fn` / `task`（含 const 泛型）、`test`、
`let` / `carry` / 賦值（含索引、tuple 欄位）、`for`（範圍、陣列、通道；`pipeline` / `unroll` / `serial` 屬性）、
`if` / `match` / `return`、`spawn` / `parallel`、`assert`、lambda（供 `.map` 與 `array(n, f)`）、
位元切片 `x[hi:lo]`、陣列切片 `a[lo..hi]`、定寬字面量 `8'hFF`。

內建：`chan::<T>(depth=)`、`broadcast(rx, n)`、`concat`、`zip`、`array`、`len`、`abs`、`min`、`max`、`print`、
`design_lowpass(taps=, cutoff=, window=)`；方法：`send` / `recv` / `close`、`map` / `tree_sum` / `sum` / `rev` / `unrolled` / `quantize`、
`to` / `sat` / `wrap` / `abs` / `xor_reduce` / `popcount`。

`iface`、`schedule`、`use`、`type` 會被解析並略過。

## 刻意不做的事（行為模擬器的邊界）

- **`stage` 層與 `;;`**：週期精確語意需要另一個模擬器，遇到會明確報錯。
- **靜態檢查**：bank 衝突（E0401）、SDF 速率平衡（E0501）沒有實作；II 可達性（E0412）由合成檢查。
  `pipeline(ii = n)` 與 `schedule` 在模擬器裡只被解析，不影響行為。
- **comptime `Graph` 建構器**：用 `array(n, |_| chan::<T>(..))` 加迴圈 `spawn` 可寫出同樣的拓樸（見 `examples/gemm.fx`）。
- **非阻塞通道操作**（`try_recv` / `empty`）在 task 層被禁止，因為它們會破壞 KPN 的確定性。
- 預設一個 task 對應一條 OS 執行緒，數百個 PE 沒問題，數萬個就不適合（`--coop` 沒有這個限制）；`print` 在多任務間的交錯順序不保證。

## 處理器範例：用 Fluxa 寫 RISC-V

兩顆以 Fluxa 寫成、每個管線階段是一個 task 的行為級處理器，同時也是本模擬器最大的壓力測試：

| 目錄 | 內容 | 執行 | 說明 |
|---|---|---|---|
| `rv32i/` | RV32I 管線（Fetch / Decode / Execute / Mem 四個 task），內含以位寬型別檢查立即值的組譯器函式 | `./rv32i.sh` | [`rv32i/run.md`](rv32i/run.md)、[`docs/v0.2.md`](docs/v0.2.md) |
| `rv64/` | RV64G（IMAFD + Zicsr）、M/S/U 特權模式、Sv39 MMU、計數器與 timer 中斷、整數精確的 IEEE-754 FPU，以及一個迷你 OS 系統測試 | `./rv64.sh` | [`docs/v0.3.md`](docs/v0.3.md) |
| `rv64/linux/` | RV64 開發板（`soc.fx`）：和 Veryl 的 Rv64Soc 同樣的記憶體映射與 ISA（RV64IMAC、Sstc、Svadu、Sv39、CLINT / PLIC / UART / GPIO），一個 hart task 加一個周邊 task，照 riscv-emu 寫；在 fluxa-emu 上開 OpenSBI + Linux + busybox（Node 約 6 秒） | `./rv64.sh`（`tests_soc.fx`）、`rv64/linux/test.sh`（開機、和 riscv-emu 比 commit log） | [`docs/emu-plan.md`](docs/emu-plan.md) |

`rv64/tests_fpu.fx` 是由 `python3 rv64/fpgen.py > rv64/tests_fpu.fx` 產生的 F/D 參考向量（固定亂數種子）。

## 文件

- [`docs/Fluxa語言規格v0.1.md`](docs/Fluxa語言規格v0.1.md)：語言規格（本模擬器依據的規範）
- [`docs/emu-plan.md`](docs/emu-plan.md)：fluxa-emu 與 Fluxa 開發板（Linux）的計畫與量測
- [`docs/v0.1.md`](docs/v0.1.md)、[`docs/v0.2.md`](docs/v0.2.md)、[`docs/v0.3.md`](docs/v0.3.md)：模擬器各版本的設計與變更紀錄
- [`docs/Fluxa發想-gemini.md`](docs/Fluxa發想-gemini.md)、[`docs/Fluxa設計報告-claude.md`](docs/Fluxa設計報告-claude.md)：語言設計初期的發想與報告

## 專案結構

```
sim/src/lexer.rs    詞法分析          sim/src/runtime.rs  排程（執行緒或協作式）、通道、死結偵測
sim/src/parser.rs   語法分析（規格 §2） sim/src/interp.rs   樹走訪直譯器（async）
sim/src/ast.rs      語法樹            sim/src/value.rs    值、型別、定點數與範圍檢查
sim/src/lib.rs      load / run_test   sim/src/cli.rs      命令列（reda sim / reda list）
sim/tests/          回歸測試（examples.rs 驅動下面所有 .fx）
synth/src/          合成：lower.rs（狀態）、gen.rs（閘）、ty.rs（位寬）、graph.rs（任務網路）、hw.rs（閘級共模擬）、cli.rs
synth/tests/        合成與閘級共模擬的回歸測試
emu/src/            fluxa-emu：elab.rs（展開網路）、emit.rs（狀態 → 中間碼）、opt.rs、exec.rs（Rust 執行器）、wgen.rs（WebAssembly）、cli.rs
emu/tests/ emu/js/  與模擬器比對、WebAssembly 重播；Node / 瀏覽器的測試平台
examples/           basics fir fir4 mac gemm fanout pipeline，以及 err_* 四個預期失敗的範例
rv32i/ rv64/        RISC-V 處理器範例（見上）；rv64/linux/：開得了 Linux 的 RV64 開發板
docs/           語言規格與版本紀錄
```

## 授權

MIT，見 [LICENSE](../LICENSE)。
