# 17. Fluxa

Fluxa 是 rueda 的第二個硬體語言，用於高階合成（HLS）。Veryl 一個時脈一個時脈地描述暫存器與導線；Fluxa 程式描述的是
**用通道連起來的任務**：每個 `task` 各自執行，通道是有界 FIFO，FIFO 滿了 `send` 就等、空了 `recv` 就等。
這是 Kahn 程序網路，所以結果與任務怎麼排程、FIFO 有多深都無關（只有死結會受影響）。
語言規格在 [`fluxa/docs/Fluxa語言規格v0.1.md`](../../../fluxa/docs/Fluxa語言規格v0.1.md)。

```fluxa
task Double(x: in chan<i16>, y: out chan<i17>) {
    for v in x { y.send(v * 2); }          // x 關閉且清空時結束
}

test doubles {
    let (tx, rx) = chan::<i16>(depth = 4);
    let (ty, ry) = chan::<i17>(depth = 4);
    spawn Double(rx, ty);                  // 端點的所有權轉給任務
    for k in 0..3 { tx.send(k); }
    tx.close();
    for k in 0..3 { assert ry.recv() == 2 * k; }
}
```

## 命令列

`reda sim` 與 `reda list` 收 `.fx` 檔，就像收 `.veryl` 檔（副檔名決定語言）：

```sh
reda sim fluxa/examples/fir.fx                  # 檔案裡所有的 test
reda sim fluxa/rv32i/rv32i.fx fluxa/rv32i/tests_basic.fx   # 多個檔案合併成一個程式
reda sim fluxa/examples/fir.fx --test fir_impulse
reda sim fluxa/examples/gemm.fx --stats         # 每條通道：送出、收到的 token 數，FIFO 最大佔用
reda sim fluxa/examples/mac.fx --trace          # 每一次 send 與 recv
reda sim fluxa/examples/fir.fx --coop           # 瀏覽器裡的排程器（見下）
reda list fluxa/examples/fir.fx                 # 列出 test
```

全部通過時結束碼是 0；有 test 失敗（`assert`、值放不進它的型別、死結）時是 1；語法錯誤是 2。
死結會列出每個任務當時在等什麼。

## 兩種排程

命令列上每個任務是一條作業系統執行緒。WebAssembly 沒有執行緒，所以在瀏覽器裡所有任務在同一條執行緒上輪流跑：
直譯器是非同步的，每個通道操作都是任務可以停下來、讓別的任務先跑的地方。`--coop` 在命令列上也用這個排程器。
兩種的結果相同（回歸測試把每個範例都用兩種方式跑過並比對）。

## 網頁 IDE

- 歡迎頁的 **＋ 新 Fluxa 專案** 會建立一個有一個 `.fx` 檔的專案；在 Fluxa 專案裡 **＋ 新增檔案** 會建 `.fx` 檔。
  編輯器會為 Fluxa 上色；打字時語法錯誤就會出現在「問題」面板。
- **▶ 執行** 跑 test（全部，或從側欄的「測試」點一個）。「輸出」面板顯示每個 test 的 `print` 輸出、
  `[PASS]` / `[FAIL]` 與原因，以及通道統計。
- 範例庫有 **Fluxa** 一組：基本語法、FIR 濾波器、乘加器、脈動陣列矩陣乘法、broadcast 扇出、（故意的）死結，
  4 階 FIR、任務網路，用四個管線任務寫成的 RV32I 處理器，
  以及 RV64G 處理器（M/S/U 特權模式、Sv39、IEEE-754 FPU、迷你 OS；90 個 test）。

## 合成

`reda synth / fpga / asic / gatesim` 也收 `.fx` 檔，前端產生和 Veryl 一樣的閘級網表（`eda::netlist::Netlist`），
後面的 FPGA 與 ASIC 流程不變。

```sh
reda synth --top Fir fluxa/examples/fir.fx          # 網表統計；-o 寫 yosys JSON、--verilog 寫 Verilog
reda fpga --asc fir4.asc --bin fir4.bin fluxa/examples/fir4.fx   # iCE40 布局繞線與位元流
reda asic --pdk sky130 --gds fir.gds fluxa/examples/fir.fx       # SKY130 晶片版圖
reda gatesim ice40 fluxa/examples/fir.fx            # test 照跑，但 task 換成它的 iCE40 網表
reda synth --list fluxa/examples/pipeline.fx        # 可以合成的 task
```

- 頂層是 `--top` 指定的 task（預設最後一個）。每個通道參數變成串流埠 `NAME_data`、`NAME_close`、
  `NAME_valid`、`NAME_ready`（valid / ready 交握；close 是串流結束），其他參數是輸入埠，或用
  `--param NAME=VALUE` 固定成常數。另有 `clk` 與 `rst`。
- 一個 task 是一個有限狀態機加資料路徑：每個通道操作是狀態的邊界，中間的純計算在一個時脈裡做完。
  位寬照規格的範圍推導；定點數的捨入與溢位模式、`@reg`（讀到舊值、迭代結束提交）、陣列、`unrolled()`、
  函式都會展開成閘。
- `pipeline(ii = 1)` 的迴圈變成一個狀態：輸入都 valid、輸出都 ready 的那個時脈就跑一次迭代（FIR 一個時脈一個樣本）。
  每個通道每次迭代最多收一次、送一次，通道操作不能放在 `if` 裡，收不能在送之後；做不到時報 E0412 並說明原因
  （`ii ≥ 2` 則退回一般狀態機）。
- `let m: mem<T, N> = [0; N]` 只用 `m[i]` 讀寫時，每次存取佔一個時脈（讀出的字直接進暫存器），
  iCE40 會把它做成 block RAM（`SB_RAM40_4K`）；其他用法退回暫存器陣列並給警告。
- 建網路的 task（`chan`、`spawn`、`broadcast`）整個變成一個網表：每個 spawn 一個電路，每條內部通道一個
  照它的 `depth` 做的 FIFO（`fluxa/examples/pipeline.fx`）。
- 還不能合成的：泛型 task、元素是 tuple 或陣列的通道、陣列參數（所以 RV32I / RV64 處理器還不行）。
- 網頁 IDE 的 Fluxa 專案也有「⚙ 合成」：① 顯示 task 與它的實例，②–⑦ 和 Veryl 一樣；
  「模擬對象」選通用閘 / iCE40 / 標準元件時，▶ 執行 會把合成頂層換成它的網表再跑 test，
  「✔ 全面驗證」把每種實作的結果並排比對。虛擬板不收 Fluxa（它的埠是串流，不是板子的腳位）。
  範例「FIR 濾波器（4 階）」放得進 iCE40，「任務網路合成成一個電路」示範網路。

設計與進度見 `fluxa/docs/synthesis-plan.md`。
