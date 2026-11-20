# Fluxa 標準庫

> Fluxa 標準庫提供以 Fluxa task 網路實作的參數化電路產生器；不是黑盒 IP。

## 1. 是什麼

標準庫原始碼位於 [`fluxa/sim/std/std.fx`](../sim/std/std.fx)，本身也是用 Fluxa 寫成的。

- 程式裡出現 `std::` 時，載入器會自動把這個檔案併入程式。
- 使用者自己定義的同名任務優先。
- 標準庫內部的輔助任務以 `Sm` 為前綴。
- 這些元件是一般的 Fluxa task 網路：行為模擬、閘級模擬（`reda gatesim`）、`reda check` 與合成都照常工作，不是黑盒 IP。

## 2. `std::SystolicMatmul::<N, AW, CW>(a, b, c)`

### 介面

- `a`、`b`：`in chan<int<-(1 << (AW-1)) ..= (1 << (AW-1)) - 1>>`，即 AW 位元有號整數；`AW = 8` 時就是 `i8`。
- `c`：`out chan<int<-(1 << (CW-1)) ..= (1 << (CW-1)) - 1>>`，即 CW 位元有號整數。

### 契約

- `A`、`B` 都是 `N × N`、row-major 的矩陣。
- `a` 和 `b` 以 lockstep 輪流讀取：每一輪一個 `a` 值配一個 `b` 值，一組矩陣共 `N·N` 次。呼叫端要交錯送出，或給輸入通道足夠的深度。
- 輸出為 `C = A × B`，順序是 row-major。
- 一直重複上述工作，直到 `a` 關閉（`b` 同時關閉），然後關閉 `c`。
- 每個輸出元素是 `N` 個乘積之和；`CW` 要足以容納它。程式不檢查溢位，累加使用 wrap。

### 結構

內部是 output-stationary 的 `N × N` 個 PE：

- `A` 往東傳播，`B` 往南傳播。
- 結果沿著每一列往東，再沿最後一行往下送出。
- 產生的內部通道深度都是 2。

### 範例

```rust
task Mm4(a: in chan<i8>, b: in chan<i8>, c: out chan<i32>) { spawn std::SystolicMatmul::<4, 8, 32>(a, b, c); }
```

### 面積

在 iCE40 HX8K、`N = 4`、`AW = 8`、`CW = 32` 時：

| 實作 | LC |
|---|---:|
| 標準庫產生器 | 16,011 |
| AI 臨場手接 | 37,233 |

HX8K 只有 7,680 個 LC，因此標準庫版本也放不進 HX8K。大部分面積在 PE（每個約 1,162 個通用閘）；任務會合成成循序狀態機，乘法器使用 LUT。改善合成品質是另一件待辦。

## 3. `std::Window3x3::<W, H, PW>(x, w)`

### 介面與契約

- `x`：`in chan<uPW>`，`uPW` 是 PW 位元無號整數。輸入是 `W × H` 的 frame，row-major；frame 會一直重複，直到 `x` 關閉。
- `w`：`out chan<[[uPW; 3]; 3]>`。
- 對每個內部像素 `(r, c)`（`1 ≤ r ≤ H−2`、`1 ≤ c ≤ W−2`），依 row-major 順序輸出一個視窗：

  ```text
  w[i][j] = x[r−1+i][c−1+j]
  ```

  其中 `0 ≤ i, j < 3`。

### 實作

只使用兩條各 W 個像素的 line buffer，以及一個 3×3 的滑動視窗。

因為目前還不能把函式或任務當參數傳遞，視窗的後處理要另寫一個 task：

```rust
task Blur(w: in chan<[[u8; 3]; 3]>, y: out chan<u8>) {
    for v in w { y.send((v[0][0] + 2 * v[0][1] + v[0][2] + 2 * v[1][0] + 4 * v[1][1] + 2 * v[1][2] + v[2][0] + 2 * v[2][1] + v[2][2]) >> 4); }
}
task Blur8x6(x: in chan<u8>, y: out chan<u8>) {
    let (tw, rw) = chan::<[[u8; 3]; 3]>(depth = 2);
    spawn std::Window3x3::<8, 6, 8>(x, tw);
    spawn Blur(rw, y);
}
```

加權和最大為 `16 × 255 = 4080`，右移 4 位後最大為 255：位寬推導證明結果放得進 `u8`，不需要 `.wrap()`，`reda check` 也沒有發現。

## 4. 寫法上的限制（目前的實作）

- 型別參數不能合成，所以元素位寬使用 const 泛型。
- task 參數不能是通道陣列（合成不支援）。
- 還不能把函式或任務當參數；`map(f)` 這類平行模式是之後的事。

## 5. 範例與測試

- [`fluxa/examples/std_generators.fx`](../examples/std_generators.fx)：標準庫生成器的範例與測試。
- [`fluxa/examples/std_matmul_wide.fx`](../examples/std_matmul_wide.fx)：`N = 3`、16 位元輸入、40 位元累加的矩陣乘法。

生成器初稿由 MiniMax-M3.1-Flash 依 [`fluxa/bench/std/spec.md`](../bench/std/spec.md) 寫成，經審閱、補測試後修正。