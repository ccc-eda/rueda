# rueda 的 AI 回饋介面（`reda --format json`）設計筆記

> 分支：`aitest`。狀態：第一階段已實作（Fluxa 的 `sim` / `test` / `list` / `synth` / `fpga`），見第 5 節。

## 1. 為什麼要做

AI 時代程式碼的產生變便宜，**確認它是對的**仍然昂貴。對寫硬體的 agent 來說，最有價值的不是另一個生成器，而是一個「判官」：

- **快**：毫秒到秒級回饋，agent 一小時能迭代上千次；
- **可信**：判斷來自模擬器、合成器、等價檢查這些確定性的工具，不是另一個 LLM；
- **結構化**：錯誤是機器能讀的資料，附位置、錯誤碼與反例，不是給人看的 log。

rueda 已經有完整的流程（Fluxa / Veryl → 模擬 → 合成 → FPGA / ASIC → 虛擬板 → RV64），而且全部是自己的程式、沒有 license、可重現。缺的只是**給 agent 的介面**。

```mermaid
flowchart LR
    A[Agent / LLM] -- 寫 .fx / .veryl --> S[原始碼]
    S --> R["reda … --format json"]
    R -- "diagnostics / tests / metrics" --> A
    R --> J{判官}
    J -- 模擬 vs golden model --> R
    J -- gatesim：RTL vs netlist --> R
    J -- "（之後）SAT 等價檢查" --> R
```

## 2. 現況（`51beac0`）

`reda` 已是統一的 CLI（`sim`、`test`、`synth`、`fpga`、`asic`、`gatesim`、`emu`、`cc`、`rvemu`、`board`），輸出是給人讀的文字。實際跑出來的樣子：

```text
$ reda sim fluxa/examples/err_overflow.fx
[FAIL] overflow_is_caught (0.000s)
       fluxa/examples/err_overflow.fx:4: runtime error in task `Mac`: assignment to `acc`:
       value 1073676289 does not fit i24 (range -8388608..=8388607)

$ reda sim fluxa/examples/err_deadlock.fx
[FAIL] circular_wait (0.000s)
       deadlock: all 3 live task(s) are blocked
         task `A` (#1) is blocked: recv on `test:circular_wait:t1->r1` (FIFO empty)
         ...

$ reda fpga --pnr veryl --bin fir4.bin fluxa/examples/fir4.fx
  cells: 191
    SB_CARRY 37 / SB_DFF 24 / SB_DFFSR 5 / SB_LUT4 125
```

觀察到的缺口：

| 缺口 | 例子 | 對 agent 的影響 |
|---|---|---|
| 規格有錯誤碼，訊息常常沒帶 | 溢位是規格的 E0301，訊息裡沒有；E0412 有 | agent 無法依錯誤種類決定修法 |
| 執行期錯誤只有行號，沒有欄位 | `err_overflow.fx:4:` | 一行有多個運算時定位不準 |
| 反例是自由文字 | deadlock 的等待環 | 要靠 regex 解析，脆弱 |
| 指標混在文字裡 | cells、LUT 數、時序 | 無法直接比較兩次嘗試的 PPA |
| 錯誤型別只有 `{line, col, msg}` | `fluxa/sim/src/lexer.rs` 的 `FxError` | 沒有錯誤碼、檔名、範圍、附註 |

## 3. 設計原則

1. **加旗標，不改預設**：沒有 `--format json` 時輸出完全不變；有的話 stdout **只有一份 JSON 文件**。旗標不叫 `--json`，因為 `synth` / `fpga` 早就用 `--json OUT.json` 當 `-o`（yosys netlist）的別名，手冊也寫了。
2. **一個信封，所有子命令共用**：agent 只要學一種格式。
3. **可重現**：同樣的輸入得到同樣的 JSON（位元組層級）。耗時這類不確定的欄位放在 `timing_info`，比較時可以略過。
4. **錯誤碼穩定**：沿用 Fluxa 規格（`fluxa/docs/Fluxa語言規格v0.1.md` §6.2）既有的 `E0xxx`；沒有規格碼的錯誤給暫定碼，並在文件中列表。§6.2 已要求每個錯誤都包含**位置、證據、可行修正**，JSON 只是把這三件事變成欄位：`file/line/col`、`data`、`notes` / `fix`。
5. **反例是一等公民**：測試失敗時給出「第一個出錯的地方」與足以重現的資訊（時間 / 週期、訊號、期望值、實際值、波形檔）。
6. **exit code 不變**：0 通過、1 失敗、2 參數錯誤或用盡指令數。

## 4. 信封格式（`rueda/1`）

```json
{
  "schema": "rueda/1",
  "command": "sim",
  "lang": "fluxa",
  "inputs": [{ "path": "fluxa/examples/err_overflow.fx", "bytes": 466 }],
  "ok": false,
  "exit": 1,
  "diagnostics": [
    {
      "code": "E0301",
      "severity": "error",
      "phase": "sim",
      "message": "assignment to `acc`: value 1073676289 does not fit i24",
      "file": "fluxa/examples/err_overflow.fx",
      "line": 4,
      "context": { "task": "Mac", "test": "overflow_is_caught" },
      "data": { "var": "acc", "type": "i24", "value": "1073676289",
                "range": ["-8388608", "8388607"], "min_type": "i31" },
      "notes": ["the value needs at least i31 (or saturate / wrap it explicitly: .sat::<T>(), .wrap::<T>())"]
    }
  ],
  "tests": [
    { "name": "overflow_is_caught", "ok": false, "diagnostic": 0 }
  ],
  "metrics": {},
  "artifacts": [],
  "timing_info": { "secs": 0.001 }
}
```

說明：

- 數值一律用**字串**表示大整數（Fluxa 內部是 120 位元精確整數，超過 JSON 安全整數範圍）。
- `data` 依錯誤碼而定，結構寫在錯誤碼表裡；agent 依 `code` 解析 `data`。
- `tests[].diagnostic` 指向 `diagnostics` 的索引，不重複內容。
- `phase` ∈ `parse | check | sim | synth | map | pnr | timing | gatesim | drc`。

### 4.1 反例（counterexample）

deadlock 不再是文字，而是一張等待圖：

```json
{
  "code": "E0701",
  "phase": "sim",
  "message": "deadlock: all 3 live tasks are blocked",
  "data": {
    "blocked": [
      { "task": "A", "id": 1, "op": "recv", "chan": "t1->r1", "fifo": "empty" },
      { "task": "A", "id": 2, "op": "recv", "chan": "t2->r2", "fifo": "empty" },
      { "task": "test", "id": 0, "op": "recv", "chan": "tz->rz", "fifo": "empty" }
    ]
  }
}
```

（規格的錯誤碼表沒有執行期 deadlock；E0601 已是「comptime 超過步數上限」。這裡用暫定的 E0701，見第 9 節。）

Veryl `$assert` 失敗、gatesim 與 RTL 不一致，則用：

```json
"data": { "time": 1234, "signal": "dut.y_data", "expected": "0x3f", "actual": "0x3e",
          "vcd": "out/fir_tb.vcd" }
```

### 4.2 各子命令的 `metrics`

| 子命令 | metrics |
|---|---|
| `sim`（Fluxa） | 每條通道的 tokens、最大 FIFO 佔用（現有 `--stats`）；每個測試的步數 |
| `sim`（Veryl） | 模擬時間、assert 次數 |
| `synth` | 各種 cell 的數量、總數；port 列表 |
| `fpga` | `SB_LUT4 / SB_CARRY / SB_DFF* / SB_RAM40_4K` 數量與佔晶片百分比；P&R 後的 Fmax、worst slack、關鍵路徑兩端 |
| `asic` | 面積、cell 數、WNS / TNS、功耗、DRC 違規數 |
| `gatesim` | 同 `sim`，加上使用的 netlist 種類 |

`metrics` 的鍵名固定，讓 agent 能直接比較兩次嘗試（例如「LUT 從 125 降到 98，Fmax 不變」）。

## 5. 實作（第一階段）

沒有照原先「全域 Report 加改寫每個 `process::exit`」的方式做，而是**讓 `reda` 自己走一條 JSON 路徑，直接呼叫各工具的函式庫**，工具的 CLI 一行都不用改：

| 檔案 | 內容 |
|---|---|
| `reda/src/json.rs` | 無依賴的 JSON 值與輸出器（物件保持插入順序，所以同樣的結果印出同樣的位元組）。沒有引入 `serde_json`。 |
| `reda/src/agent.rs` | `--format json` 的所有子命令：解析參數、呼叫 `fluxa::load_many` / `run_test`、`fluxa_synth::synthesize`、`eda::fpga::ice40::map`，組成信封。 |
| `reda/src/main.rs` | 取出 `--format json`（或 `--format=json`）後交給 `agent::run`。 |
| `fluxa/sim/src/lib.rs` | `TestResult` 多了 `failure: Option<Failure>`：錯誤碼、檔名、行號、任務、訊息、死結時被卡住的任務。原本的 `message` 字串不變。 |
| `fluxa/sim/src/runtime.rs` | 記下第一個執行期錯誤的（行號，任務，訊息）與死結的任務清單；兩處重複的死結報告合成 `declare_deadlock`。 |
| `fluxa/sim/src/diag.rs` | 執行期錯誤訊息 → 錯誤碼的對照表。 |
| `reda/tests/format_json.rs` | 驗收測試。 |

細節：

- 模擬用協作式排程（`coop`），測試裡 `print` 的輸出收進 `tests[].output`，不會混進 JSON。
- 語法錯誤、`line N: error[E0412]: …` 這類文字訊息，由 `agent.rs` 拆出檔名、行號與錯誤碼。
- E0301 的 `min_type` 是**由觀察到的那個值推出的下界**：`err_overflow.fx` 第一次溢位的值是 32767²，所以報 i31；把 8 次累加都裝下其實要 i34。要給出真正的需求，得靠合成時的區間分析（之後）。
- 目前不支援的情況（Veryl、ASIC、`--bin` / `--asc` 等輸出旗標、`--arch ecp5`）一律回一份信封，`exit: 2`，訊息說明尚未支援。
- `fpga` 的 metrics 停在 LUT 對應之後；佈局繞線與時序之後再加，信封裡會有一筆 `severity: note` 說明。

### 錯誤碼歸類的取捨

執行期錯誤在模擬器裡是 `RtErr::Msg(行號, 字串)`，到處都有產生點。第一階段用 `diag.rs` 依訊息片段歸類，改動最小；代價是訊息措辭一改就要同步改表，所以由測試鎖住。之後應該把錯誤碼放進 `RtErr` 本身，在產生點就決定。

### 還沒做

- 欄位（col）：執行期錯誤目前只有行號，要讓 AST 節點帶欄位。
- Veryl 的 `sim` / `synth` / `fpga`，以及 `asic`、`gatesim`。
- `fpga` 的佈局繞線與時序 metrics（需要 IceStorm chipdb：`VERYL_EDA_CHIPDB`）。
- Fluxa `--stats` 的每條通道統計放進 `metrics`。

## 6. MCP 外層

做完 `--format json` 後，加一個很薄的 MCP server，讓 Claude Code 這類 agent 直接呼叫：

- 工具：`reda_sim`、`reda_list`、`reda_synth`、`reda_fpga`、`reda_gatesim`；參數就是檔案與旗標，回傳就是信封。
- 形式有兩個選擇（待定）：
  - `reda mcp` 子命令：stdio 上的 JSON-RPC，單一執行檔，部署最簡單；
  - `tools/` 下的 Node 腳本：呼叫 `reda --format json`，跟 Rust 核心解耦。

## 7. 驗收

- `fluxa/examples/err_*.fx` 每一個都恰好產生一筆 diagnostic，錯誤碼與位置正確；
- `err_deadlock.fx`、`fir4.fx` 的輸出跑兩次逐位元組相同（扣掉 `timing_info`）；
- 不加 `--format json` 時，既有的測試結果不變；
- 一個小腳本：故意改壞 `fir.fx`，讓 agent 只看 JSON 把它修好，記錄迭代次數。

## 8. 之後：判官（第 2 步）

1. **協同模擬判官**：Fluxa 測試本身就是 golden model；`gatesim` 已能把 task 換成 netlist 再跑同一組測試，不一致時以 4.1 的格式回報第一個出錯的週期。
2. **形式等價判官**：對合成前後的 netlist 建 miter，用 SAT 檢查；用來驗證 AI 做的改寫或最佳化。
3. **Benchmark**：30–50 個附規格、golden model 與資源上限的硬體題目。

## 9. 待決定

- 規格沒有涵蓋的執行期錯誤要不要正式編碼？暫定：E0701 執行期 deadlock、E0702 通道端點所有權（`err_linear.fx`）、E0703 索引越界。要併入規格 §6.2 的表，還是另開 `docs/note/error-codes.md`？
- MCP 用 `reda mcp`（Rust）還是 Node 腳本？
