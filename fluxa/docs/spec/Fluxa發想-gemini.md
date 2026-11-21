# ccc: 如果專為 HLS 設計一個新語言，應該長什麼樣子？

* chat -- https://share.gemini.google/JzN6s7FJukOO

如果我們要從零開始，**專門為 HLS（高階綜合）設計一門全新的程式語言**，它的核心哲學必須徹底擺脫「軟體思維（馮紐曼架構的順序執行）」的束縛，轉而擁抱「空間思維（Spatial Computing / Dataflow）」。

現有 C/C++ 在 HLS 中的最大痛點，在於它本質上是「順序執行的」，工程師必須靠大量的 `#pragma`（編譯指導指令）去強行扭轉它的行為。

一門理想的 HLS 專用語言，應該具備以下五大核心特徵：

---

### 1. 原生的「精確位元」與「定點數」型別（Bit-Precise & Fixed-Point Types）

在軟體裡，最小的單位是 `byte`（8 bits）；但在硬體裡，如果一個狀態機只需要 3 種狀態，它只需要 **2 bits**。

* **現狀**：C++ 必須透過函式庫（如 `ap_int<13>`）來模擬，語法極其冗長。
* **理想設計**：型別系統直接支援任意位元寬度。例如：
* `u7`：無號 7 位元整數
* `i12`：有號 12 位元整數
* `fixed<16, 8>`：內建定點數（16位元總長，8位元小數），讓演算法工程師不用手動處理複雜的小數位移與截斷。



---

### 2. 用「語法結構」取代 `#pragma`（將硬體意圖內置）

目前 HLS 最讓人詬病的是 `#pragma HLS pipeline II=1` 這種像「咒語」一樣的註解，漏掉一個程式碼意義就完全變了。

* **理想設計**：將流水線（Pipeline）與迴圈展開（Unrolling）變成語言的**一級公民語法（First-Class Syntax）**。
* 例如，如果要宣告一個管線化迴圈，可以直接用專用關鍵字：
```rust
// 想像中的語法範例
pipeline for (let i = 0; i < 100; i++) {
    // 這裡的運算會自動被編譯器排程為 II=1 的硬體管線
}

```



---

### 3. 第一級公民的「資料流與通道」（Dataflow & Channels）

硬體的本質不是函式呼叫，而是**管線與資料流（Stream）**。

* **理想設計**：語言原生內建通道（Channel）與資料流圖（Dataflow Graph）的語意。
* 變數之間的傳遞不透過抽象的指標（Pointer），而是透過顯式的管道：
```rust
let stream_in: Channel<u32, 16> = ...; // 容量為 16 的硬體 FIFO
let stream_out = stream_in.map(|x| x * 3);

```


這能讓編譯器一眼看出哪些資料可以平行處理、哪些需要硬體 FIFO 緩衝，完全消除了 C 語言指標帶來記憶體別名（Aliasing）的災難。

---

### 4. 記憶體階層的顯式宣告（Explicit Memory Hierarchy）

在晶片設計中，「資料存在哪裡」決定了效能與面積（暫存器 Register vs. 晶片內 SRAM / BRAM vs. 外部 DRAM）。

* **理想設計**：型別或變數宣告時，可以直接指定其硬體物理歸屬：
```rust
reg<u32, 1024> fast_buffer;   // 強制分配在 FPGA 的分散式暫存器 / LUT 裡
bram<u32, 65536> table_data;  // 強制分配在專屬的 Block RAM 塊中

```


工程師可以不用猜測編譯器會把陣列放在哪裡，而是直接對硬體資源進行精確管控。

---

### 5. 強大的編譯期硬體架構生成（Comptime Architecture Generation）

硬體設計最常遇到的需求是「參數化」（例如：這個濾波器的矩陣大小是可以客製化的）。

* **理想設計**：借鑒 Zig 的 `comptime` 或 Rust 的巨集，但專門用於**在編譯期計算並組合硬體拓樸**。你可以用高階邏輯在編譯期「寫一段程式去畫出你想要的電路連接方式」，而不是靠 C++ 晦澀難懂的 Template。

---

### 具體想像：這門語言寫出來會是什麼樣子？

假設我們要寫一個簡單的「乘加運算單元（MAC，Multiply-Accumulate）管線」：

```rust
// 定義一個硬體元件 (Component)
component MacUnit(
    input clk: Clock,
    input a: Stream<i16>,
    input b: Stream<i16>,
    output out: Stream<i32>
) {
    // 宣告一個管線化處理區塊
    pipeline for (x, y) in zip(a, b) {
        // 自動生成硬體乘法器與累加暫存器，II = 1
        let mult = (x as i32) * (y as i32);
        emit out.write(mult);
    }
}

```

### 總結

如果為 HLS 打造一門新語言，它看起來會非常像 **「融合了 Rust 的嚴謹型別、Go 的通道並行語意、以及硬體描述精確度的全新混合體」**。

它不再試圖讓你在寫「軟體」，而是提供一個既有軟體的抽象便利性、又能完全掌控硬體空間與時間（時脈）的完美折衷點。這也是近年許多 EDA 新創與學術團隊一直在探索的前沿方向。