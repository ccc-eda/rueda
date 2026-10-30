# Fluxa 合成（synth + PnR）規劃

目標：`reda synth / fpga / asic / gatesim` 收 `.fx` 檔，Fluxa 程式一路做到 iCE40 / ECP5 位元流與 SKY130 GDS，
而且每一步都能用 Fluxa 自己的 `test` 驗證。

## 1. 架構：一個前端，後端不動

```
.fx ──► 解析（fluxa/sim 現有的 lexer / parser / AST）
     ──► fluxa/synth（新 crate：rueda-fluxa-synth，lib fluxa_synth）
           ① 展開：comptime 與 const 用現有直譯器求值；從 --top 的 task 出發，靜態展開 spawn 樹、
              泛型單態化；每條 chan 成為一個 FIFO 實例
           ② 位寬推導：照規格 §3.2（區間算術：int<lo..=hi>、fixed<I,F>），每個運算式得到靜態位寬
           ③ 降階：每個 task → 控制資料流圖（迴圈、recv / send、if / match、@reg、carry）
           ④ 排程：先做 FSM（每個通道操作是狀態邊界），再做 pipeline(ii = n) 的模數排程
           ⑤ 產生網表：資料路徑、FSM、FIFO 直接用 eda::netlist 的 bit-level builder
     ──► eda::netlist::Netlist ──► 現有流程原封不動：最佳化、iCE40 / ECP5 對應、P&R、位元流、ASIC、GDS
```

閘級網表是語言與後端的分界（根目錄 AGENTS.md 的規則）：`eda` 不需要知道 Fluxa。

### 硬體介面

- 合成的頂層是一個 `task`（`--top Name`，預設：最後一個非 test 的 task）。
- 頂層 task 的通道參數變成串流埠：`x: in chan<i16>` → `x_data[16]`、`x_valid`、`x_ready`（輸出方向相反），
  也就是 valid / ready 握手，和 AXI-Stream 相同。另外有 `clk`、`rst`。
- 非通道參數（`n: u16` 之類）在合成時給定常數：`--param n=8`，等同 Veryl 的參數。
- 內部的 `chan<T>(depth = d)` 變成深度 d 的 FIFO：小的用暫存器，大的交給 `eda::bram` 推導成 block RAM。

### 一開始不支援（報清楚的錯誤）

遞迴、執行期才決定個數的 `spawn`、`print`（在硬體裡丟掉）、浮點數、`stage` 層、
沒有通道操作又不終止的迴圈。

## 2. 驗證：同一個 test 跑兩次

規格 §6.1 的「雙模擬器等價測試」照做，方法和 Veryl 的 `gatesim` 一樣：

- `reda gatesim opt|ice40|ecp5|asic|sky130 file.fx`：跑 Fluxa 的 `test`，但頂層 task 換成它合成出來的網表。
  網表用 `eda::netsim` 逐週期模擬，外面包一個轉接任務：把 KPN 通道接到網表的 valid / ready 埠。
- KPN 的輸出與排程無關（§3.10 性質 5），所以行為模擬與閘級模擬的輸出 token 序列必須完全相同；
  不同就是合成器的 bug。
- `cargo test` 對每個可合成的範例都跑一次：行為、通用閘、iCE40、vlib 標準元件。

## 3. 里程碑

| 版本 | 內容 | 驗收 |
|---|---|---|
| F1 | 重構：把「網表 → 輸出 / FPGA / ASIC 流程」從 veryl/synth/cli.rs 抽到 `eda::flow`，兩個前端共用；`reda synth` 依副檔名選前端 | 行為不變：現有所有測試通過 |
| F2 | fluxa/synth 核心子集：單一 task，`for v in rx { … }`、整數與 bits 運算、if / match、const、carry；FSM 排程；FIFO；`reda synth x.fx` 輸出 yosys JSON / Verilog | 雙倍器、MAC 合成成功；yosys `read_json` 檢查通過 |
| F3 | 閘級共模擬：`reda gatesim … x.fx`，回歸測試 | MAC、basics 的 test 在網表上全部通過 |
| F4 | 多任務圖（靜態 spawn 樹、broadcast）、陣列與 `unroll`、`@reg` 移位暫存器、定點數與 `to::<T>(round, ovf)` | FIR、fanout、GEMM 的 test 在網表上通過 |
| F5 | `pipeline(ii = n)` 模數排程與 RecMII / ResMII 檢查（E0412）；`mem` → block RAM | FIR 每週期一個樣本；II 不可行時報出關鍵環 |
| F6 | FPGA / ASIC：`reda fpga / asic x.fx`；網頁 IDE 的 Fluxa 專案打開「⚙ 合成」（① 顯示任務圖，②–⑦ 同 Veryl） | FIR 燒成 iCE40 位元流、做出 SKY130 GDS；e2e 測試 |
| F7（延伸） | 用 Fluxa 寫的 RV32I 管線走完整個合成流程 | rv32i 的 test 在網表上通過 |

進度：

- F1–F4 完成。
- F5 完成：`pipeline(ii = n)` 的迴圈變成單一狀態，輸入都 valid、輸出都 ready 的那個週期就跑一次迭代
  （FIR 100 個樣本約 100 個週期）。每個通道每次迭代最多收一次、送一次，通道操作不能放在 `if` 裡，
  收不能在送之後；做不到時 `ii = 1` 報 E0412 並說明原因，`ii ≥ 2` 退回一般的 FSM。迴圈本體仍是一個
  週期內的組合邏輯（沒有切成多級管線暫存器）。`mem<T, N>`（初值全 0）只以 `m[i]` 存取時，讀寫各佔
  一個週期，讀出的字直接進暫存器，網表記成 `$mem:` 網路，iCE40 對應成 `SB_RAM40_4K`；其他用法退回
  暫存器陣列並給警告。
- F6 完成：`reda fpga / asic` 收 `.fx`。16 階 FIR 在 iCE40 UP5K 放不下（104% 邏輯、40 支 IO），
  新增的 `examples/fir4.fx`（4 階、8 位元）是 191 個元件、77 MHz，做出位元流；16 階 FIR 做出 SKY130 GDS
  （38 MHz，DRC 0、LVS 相符）。網頁 IDE 的 Fluxa 專案有「⚙ 合成」（① 列出 task 與實例）、閘級的 ▶ 測試
  與 ✔ 全面驗證；`npm test` 對每個有合成頂層的 Fluxa 範例做合成與閘級測試，e2e 從合成跑到繞線再跑閘級測試。
- F7 完成（各階段）：通道可傳 tuple（token 超過 128 位元也行）、陣列參數變成輸入埠、`match` / `if`
  的分支可以有通道操作與 `return`（變成分支狀態），以及常數名稱當 match 樣式時是比較而不是綁定（之前的 bug）。
  RV32I 的 Fetch / Decode / Execute / Mem 是 3.5k / 13k / 5.9k / 12k 個通用閘，24 個 test 全部換成閘級
  （opt 與 iCE40）都通過。`Core` 是泛型 task，整個網路還不能合成成一個網表；RV64 還沒試。

每個版本：先寫測試、再實作，`cargo test`、`npm test`、`npm run e2e` 全過才 commit。

## 4. 需要先決定的事

1. 頂層與介面：用 `--top` 指定 task、通道變 valid / ready 埠（上面的設計），還是另外加一個 `top` / `graph` 宣告？
2. 第一版的排程目標：先求正確（FSM，每次迭代數個週期），F5 再追吞吐（II = 1）。
3. 合成結果要不要也輸出 RTL 層級的 Verilog / Veryl（可讀的 FSM 與資料路徑），還是閘級網表就夠？
