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
  以及用四個管線任務寫成的 RV32I 處理器。

## 還沒有的

Fluxa 目前只有行為模擬。`reda synth / fpga / asic / gatesim` 不收 `.fx` 檔，IDE 在 Fluxa 專案裡也會把這些按鈕關掉。
合成要做的是一個 Fluxa 前端，產生和 Veryl 一樣的閘級網表（`eda::netlist::Netlist`）；從那裡開始，FPGA 與 ASIC 流程都不用改。
