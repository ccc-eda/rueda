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
  以及 RV64G 處理器（M/S/U 特權模式、Sv39、IEEE-754 FPU、迷你 OS；93 個 test）。

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
- 通道可以傳 tuple；陣列參數是一個輸入埠（全部位元）；`match` / `if` 的分支裡可以有通道操作與 `return`。
  RV32I 處理器的四個階段（Fetch、Decode、Execute、Mem）都合成得出來，24 個 test 在閘級全部通過：
  `reda gatesim opt fluxa/rv32i/rv32i.fx fluxa/rv32i/tests_basic.fx`（每個 task 換成網表，約 7 分鐘）。
- 泛型 task 的 const 泛型參數用 `--param NAME=VALUE` 給值（在網路裡或閘級測試時，取 spawn 的 `::<…>`）。
  網路的陣列參數變成它的一個輸入埠，接到拿到它的實例。所以 RV32I 的 `Core` 是一個網表（四個階段加上中間的
  FIFO，約 36,500 個通用閘）：`reda synth --top Core --param WBD=8 fluxa/rv32i/rv32i.fx`；
  `reda gatesim opt --top Core fluxa/rv32i/rv32i.fx fluxa/rv32i/tests_basic.fx` 在它上面跑 24 個 test（看門狗那個要跑
  3,000 個指令，約半小時；其餘 23 個約一分鐘）。
- `--param` 的值若不是整數，就是在程式裡求值的 Fluxa 運算式，所以陣列參數也能是常數：用索引讀它就是 ROM
  （併進邏輯閘），用它當初值的記憶體一開始就是這些字。程式與資料內建的 RV32I `Core` 只剩兩條輸出串流當埠，
  重置後就跑程式，在 ECP5 上完成佈局佈線（21,163 LUT4、5,516 FF、72 支 IO，約 16 MHz）：
  ```
  reda fpga --arch ecp5 --bit core.bit --top Core --param WBD=8 --param 'data=dimage([])' \
       --param 'prog=image([addi(1, 0, 0), addi(2, 0, 1), addi(3, 0, 11), add(1, 1, 2), addi(2, 2, 1), bne(2, 3, -8), ecall()])' \
       fluxa/rv32i/rv32i.fx
  ```
- 值域：`if` 的每個分支裡（以及 `&&` / `||` 的右邊），條件界定的變數有較窄的值域，所以
  `if sh <= 0 { x << (0 - sh) }`、`if b != 0 { a / b }` 都能合成；函式的回傳值保留比宣告型別窄的值域。
- 沒有通道操作的 `serial for` 每次迭代一個時脈（狀態）；其他這種迴圈展開成邏輯閘。長的運算就是這樣變成
  多週期單元，迭代結束就是時脈邊界。
- 用陣列參數當初值的記憶體（`let m: mem<T, N> = data`）一開始就是它的字：有 `--param data=…` 時是
  block RAM 的初值（iCE40 `INIT_*`、ECP5 `INITVAL_*`），否則閘級模擬時直接載入（不佔腳位）。同一個記憶體的
  讀取共用一個讀埠。
- RV64G 處理器（`fluxa/rv64`）合成成一個網表，在 ECP5-85 上完成佈局佈線：72,834 個 LUT 位置（87%）、
  12,179 FF、32 個 DP16KD、8.35 MHz。它的 FPU 是 `fpu.fx` 裡的多週期 task `Fpu`（乘法 53 拍、除法 116 拍、
  開方 60 拍，精確加法器與捨入器只有一份、大家共用），在 `tests_fpu.fx` 的每個向量上與 `comb fn` 參考模型
  `fp_op` 逐位元相同；M 擴充是 64 拍的 task `MulDiv`。
  ```
  reda fpga --arch ecp5 --clock 5 --bit rv64.bit --top Core --param WBD=8 --param TLBN=8 --param 'data=dimage([])' \
       --param 'prog=image([addi(1, 0, 0), addi(2, 0, 1), addi(3, 0, 11), add(1, 1, 2), addi(2, 2, 1), bne(2, 3, -8), simexit()])' \
       fluxa/rv64/rv64.fx fluxa/rv64/fpu.fx
  ```
- 網頁 IDE 的 Fluxa 專案也有「⚙ 合成」：① 顯示 task 與它的實例，②–⑦ 和 Veryl 一樣；
  「模擬對象」選通用閘 / iCE40 / 標準元件時，▶ 執行 會把合成頂層換成它的網表再跑 test，
  「✔ 全面驗證」把每種實作的結果並排比對。虛擬板不收 Fluxa（它的埠是串流，不是板子的腳位）。
  範例「FIR 濾波器（4 階）」放得進 iCE40，「任務網路合成成一個電路」示範網路。

## fluxa-emu：編譯後再跑

Fluxa 模擬器是直譯器（每個運算走一次語法樹、變數查雜湊表），RV64 上每秒約一萬個指令。**fluxa-emu**
（`fluxa/emu`）像 veryl-emu 一樣先編譯再跑：task 照合成的切法變成狀態（每個通道操作是一個邊界），狀態裡的運算
依值域放進 64 或 128 位元的暫存器，task 的變數在 frame 裡，通道是記憶體裡的環形緩衝區；最後產生 WebAssembly。

```sh
reda emu fluxa/examples/fir.fx                      # test 照跑，但 task 換成編譯後的版本（結果必須和模擬器相同）
reda emu --top Core fluxa/rv64/rv64.fx fluxa/rv64/fpu.fx fluxa/rv64/tests_basic.fx
reda emu -o core.wasm --top Core --param WBD=8 --param TLBN=8 --param 'data=dimage([])' \
     --param 'prog=image([addi(1, 0, 5), ecall()])' fluxa/rv64/rv64.fx fluxa/rv64/fpu.fx
```

- 和模擬器一樣嚴格：值放不進型別、索引越界、除以零都會讓 test 失敗。
- `-o` 寫出 WebAssembly 模組與 JSON（通道、記憶體的位址），`fluxa/emu/js/emu.mjs` 是 Node 與瀏覽器的測試平台。
- 驗證：`fluxa/emu/tests/agree.rs` 讓每個範例、RV32I、RV64 的 test 在兩邊各跑一次，結果相同；
  `fluxa/emu/tests/wasm.sh` 把 Rust 執行器的輸入、輸出 token 錄下來，WebAssembly 重播後輸出相同。
- 速度：管線版的 RV64 在 Node 上每秒約 230 萬個指令；一個 RV64 迴圈，模擬器要 1,247 秒，fluxa-emu 不到 2 秒。

## Fluxa 開發板：Linux 跑在 Fluxa 寫的 RV64 上

`fluxa/rv64/linux/soc.fx` 是用 Fluxa 寫的 RV64 開發板，和 Veryl 的 Rv64Soc 有相同的記憶體映射與指令集，
所以開同一個映像（OpenSBI + Linux 6.12 + busybox + MicroPython）：

- `Hart`：一個 task 就是一顆多週期的 RV64IMAC + Zicsr + Zicntr + Zifencei + Sstc + Svadu，M / S / U、Sv39（512 項 TLB），
  RAM 與 CLINT 在它裡面；照 riscv-emu 寫，`TRACE` 送出 commit log。
- `Devices`：UART、PLIC、GPIO、關機裝置。外界的輸入（鍵盤、按鈕）與時間用一對詢問通道向外界要：Fluxa 沒有
  非阻塞的 recv，所以 hart 每 16384 個指令問一次，等待中斷（wfi）時告訴外界下一個計時器在什麼時候。

```sh
reda emu -o soc.wasm --top Soc --param RAMW=33554432 --param TRACE=false fluxa/rv64/linux/soc.fx
node fluxa/rv64/linux/boot.mjs soc.wasm fw_payload.bin --script cmds.txt     # 開機、在提示字元打 cmds.txt 的指令
fluxa/rv64/linux/test.sh                       # 前 100 萬個指令的 commit log 和 riscv-emu 逐行相同，開到 shell
```

開到 shell 約 8,450 萬個指令，Node 上不到 10 秒。網頁 IDE 的範例庫有「Fluxa 開發板」一組：Linux + MicroPython + C、
跑馬燈、俄羅斯方塊、貪食蛇、呼吸燈、反應時間遊戲，程式和 RV64 開發板相同；🖥 模擬機的引擎選 fluxa-emu（硬體本身，
即時）或 riscv-emu（指令集）。

設計與進度見 `fluxa/docs/synthesis-plan.md`。
