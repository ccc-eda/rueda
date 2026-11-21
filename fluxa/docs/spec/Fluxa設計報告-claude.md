# Fluxa:一門專為 HLS 設計的語言

* claude -- https://claude.ai/share/0795e7a4-aac3-450f-ad8c-31d9160037ae

文件裡的五點(精確位元、一級語法取代 `#pragma`、通道、顯式記憶體、comptime)是很好的起點。下面把它補成一個可以實作的設計,並補上原文沒有談到的幾個關鍵問題。

## 一、設計哲學

| 原則 | 含義 |
|---|---|
| **空間優先** | 程式是「行程(task)+ 通道(channel)」組成的圖,不是一條呼叫堆疊 |
| **時間可見** | 延遲、II、時脈邊界是型別與語法的一部分,不是事後優化 |
| **做不到就報錯** | 要求 `ii = 1` 但達不到,是編譯錯誤,而不是默默退化成 II=3 |
| **演算法與排程分離** | 借鏡 Halide / TVM:邏輯寫一處,硬體映射寫另一處 |
| **模擬即語意** | 軟體模擬與 RTL 行為由語言語意保證一致,不會有 C-sim 過了、Co-sim 失敗的情況 |

## 二、型別系統

```rust
u7  i12  bit  bits<24>            // 任意位寬
fixed<I=8, F=8, round=nearest, ovf=sat>   // 定點數,捨入/溢位策略寫在型別裡
int<0..=99>                       // 範圍型別,位寬由範圍推導(7 bits)
```

**位寬自動推導,窄化必須顯式:**

```rust
let a: u8 = ..; let b: u8 = ..;
let c = a + b;            // 推導為 u9,不會悄悄溢位
let d: u8 = c;            // ❌ 編譯錯誤:u9 → u8 可能丟失資訊
let d: u8 = c.sat();      // ✅ 飽和
let d: u8 = c.wrap();     // ✅ 回繞
```

這樣可以一次處理 C++ 兩個痛點:`ap_int` 冗長,以及整數提升造成的隱性位寬浪費。

## 三、時間模型:三個層級

這是原文沒有談、但最重要的部分。

```rust
comb fn parity(x: u8) -> bit { x.xor_reduce() }   // 零週期組合邏輯,不能含狀態

task Mac(...) { ... }          // 由編譯器排程的行程(延遲不敏感,自動握手)

stage Mac2(...) @clk { ... }   // 週期精確:每個 `;;` 代表一個時脈邊界
```

- `comb`:純組合,編譯器保證無迴圈、無狀態。
- `task`:預設層級。編譯器負責排程與握手,延遲變動不影響正確性。
- `stage`:需要精確控制時序(例如對接固定延遲的 IP)時才下降到這一層。

三層可以在同一個專案中混用,所以不必二選一:純演算法選擇 `task`,時序敏感的介面層選擇 `stage`。

## 四、通道與資料流(線性型別)

```rust
chan<u32, depth=16>        // 硬體 FIFO
```

通道是**線性資源**:每條通道恰好一個生產者、一個消費者,由編譯器檢查。

```rust
let (tx, rx) = chan::<u32>(depth = 16);
spawn producer(tx);
spawn consumer(rx);
spawn other(rx);    // ❌ 錯誤:rx 已被 consumer 消耗
```

好處:
1. 沒有指標,因此沒有別名問題。
2. 對靜態速率的圖,可在編譯期做死結檢查與 FIFO 深度推導(SDF 分析)。
3. 需要扇出時必須顯式寫 `broadcast(rx, n=2)`,硬體成本一目了然。

## 五、記憶體:Bank 衝突是型別錯誤

```rust
let mut buf: mem<u8, 1024> @bram(banks = 4, partition = cyclic, ports = 1r1w);

unroll(4) for i in 0..1024 {
    buf[i] = f(i);          // ✅ i mod 4 各落在不同 bank,編譯器可證明無衝突
}

unroll(4) for i in 0..1024 {
    buf[i] = buf[i + 1];    // ❌ 同一週期內可能存取同一 bank,埠數不足
}
```

這借用了 Dahlia 的仿射型別(affine types)思路:資源衝突在編譯期被證明或拒絕,不會留到合成後才發現 II 變差。

放置選項:`@reg`、`@lutram`、`@bram`、`@uram`、`@dram(axi = ...)`。

## 六、取代 `#pragma` 的語法

```rust
pipeline(ii = 1) for i in 0..N { ... }   // 達不到 II=1 → 編譯錯誤,並指出瓶頸
unroll(8) for j in 0..8 { ... }          // 空間展開
parallel { taskA(); taskB(); }           // 顯式並行區塊
```

迴圈攜帶相依(loop-carried dependency)必須用 `carry` 宣告,而不是讓編譯器猜:

```rust
carry acc: i48 = 0;
```

**排程獨立於演算法:**

```rust
schedule Mac {
    target fmax = 300MHz, device = "xcu250";
    loop0: pipeline(ii = 1);
    acc:   @reg;
}
```

演算法工程師寫邏輯,硬體工程師調排程,兩者互不污染。

## 七、編譯期架構生成(comptime)

```rust
comptime fn systolic<const N: u32>() -> Graph {
    let pe = array2d(N, N, |_, _| Pe::new());
    for i in 0..N {
        for j in 0..N - 1 {
            connect(pe[i][j].east, pe[i][j + 1].west);
            connect(pe[j][i].south, pe[j + 1][i].north);
        }
    }
    Graph::from(pe)
}

let array = systolic::<16>();   // 編譯期展開成 16×16 脈動陣列
```

與 C++ template 的差別:這是普通程式碼,可以除錯、可以用迴圈與條件,錯誤訊息是人話。

## 八、介面即型別

```rust
iface AxiStream<T> { data: T, valid: bit, ready: bit @reverse, last: bit }

task Top(in: AxiStream<u32>, out: AxiStream<u32>) { ... }
```

協定的 `valid/ready` 方向由型別決定,編譯器自動產生握手邏輯,不再需要手寫 `hls::stream` + `#pragma HLS INTERFACE`。

## 九、完整範例:修正後的 MAC

原文的範例缺少累加器,而且在 `pipeline` 內使用 `emit`,語意不明。以下是完整版本,並展示位寬檢查:

```rust
task Mac(a: in chan<i16>, b: in chan<i16>, out: out chan<i48>, len: u16) {
    carry acc: i32 = 0;                  // ❌ 編譯器報錯:
                                         //   len 最大 65535,i16*i16 累加最多需 i48
    pipeline(ii = 1) for _ in 0..len {
        acc += a.recv() * b.recv();      // i16 * i16 自動推導為 i32
    }
    out.send(acc);
}
```

改成 `carry acc: i48 = 0;` 後通過。這類錯誤在 C++ 中會是沉默的溢位 bug。

## 十、驗證與工具鏈

```rust
assert always (out.valid -> out.data.abs() < 1 << 40);   // 時序屬性
test mac_basic { ... }                                    // 內建測試,同一份語意跑模擬
```

**編譯流程:**

```
Fluxa 原始碼 → 型別/資源檢查 → Dataflow IR (MLIR/CIRCT)
→ 排程(SDC/ILP) → FSM + 管線生成 → Verilog / FIRRTL / 供應商 IP
```

建議建在 **MLIR + CIRCT** 上,可以直接重用現有的 Verilog 後端與優化 pass,不必從零寫。

## 十一、與既有工作的關係

這些想法並非全新,是把已驗證的點整合起來:Dahlia(仿射型別管理記憶體)、Halide(演算法/排程分離)、Chisel/Bluespec(型別化硬體描述)、Spatial(空間計算)、Filament(時序型別)。真正的設計挑戰在於:**三層時間模型如何互通,以及編譯器的錯誤訊息品質**。

## 十二、實作路線圖

1. **MVP**:位寬型別 + `task`/`chan` + `pipeline(ii)` → 產出 Verilog,跑通 FIR、MAC。
2. **資源型別**:`mem` 的 bank 檢查、`schedule` 分離。
3. **comptime + 介面型別**:脈動陣列、AXI 自動生成。
4. **`stage` 層與時序屬性驗證**。

---

如果你想繼續往下做,我可以接著寫:**(a)** 完整的 EBNF 文法、**(b)** 型別系統的形式化規則、**(c)** 一個 FIR/矩陣乘法的完整範例與預期生成電路,或 **(d)** 把這份設計整理成可編輯、可分享的文件。你想先從哪一塊開始?