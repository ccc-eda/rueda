# Fluxa 語言規格 v0.1

* claude -- https://claude.ai/share/0795e7a4-aac3-450f-ad8c-31d9160037ae

版本 0.1 草案｜2026-10-29｜狀態：設計草案，尚未實作

## 0. 範圍與前版更正

本文件把前一輪的設計概要補成可實作的規格：

- **第一部分**：詞法、EBNF 文法、型別系統的形式化規則（§1–§3）。
- **第二部分**：FIR 與脈動矩陣乘法的完整範例、預期電路、編譯器架構、風險與路線圖（§4–§9）。

**更正**：前版記憶體範例 `buf[i] = buf[i + 1]` 在 `ports = 1r1w`（讀寫埠分離）下其實是合法的，因為讀與寫各自在每個 bank 只有一次存取。要觸發衝突，應改用單埠 `ports = 1`，或步長為 2 的存取。判定式見 §3.5。

## 1. 詞法

| 類別 | 規則 |
| --- | --- |
| 識別字 | `[A-Za-z_][A-Za-z0-9_]*` |
| 註解 | `// 單行`、`/* 區塊，可巢狀 */` |
| 型別 token | `u<n>`、`i<n>`（n ≥ 1），如 `u7`、`i12` |
| 整數字面量 | `42`、`0xFF`、`0b1010_0101`，底線僅作分隔 |
| 定寬字面量 | `8'hFF`、`4'b1010`、`12'd100`，寬度明確、型別為 `uN` |
| 定點字面量 | `0.75`，需型別標註或由上下文推導 |
| 範圍 | `a..b` 右開、`a..=b` 右閉 |

**保留字**：`use pub const type fn comb task stage comptime iface schedule test let mut carry for in if else match spawn parallel pipeline unroll serial assert always eventually as true false chan mem fixed int bits bit bool reg lutram bram uram dram`。`reverse`、`depth`、`round`、`ovf` 為上下文關鍵字。

## 2. 文法（EBNF）

### 2.1 記號與頂層

```
(* = 定義  | 選擇  { } 零或多次  [ ] 可選  ( ) 分組  "x" 終結符 *)

program       = { item } ;
item          = [ "pub" ] ( use_decl | const_decl | type_decl | fn_decl
                          | comb_decl | task_decl | stage_decl | comptime_decl
                          | iface_decl | schedule_decl | test_decl ) ;

use_decl      = "use" path [ "as" IDENT ] ";" ;
path          = IDENT { "::" IDENT } ;
const_decl    = "const" IDENT ":" type "=" expr ";" ;
type_decl     = "type" IDENT [ generics ] "=" type ";" ;
generics      = "<" gparam { "," gparam } ">" ;
gparam        = "const" IDENT ":" type | IDENT ;
```

### 2.2 三個時間層級的宣告

```
fn_decl       = "fn" IDENT [ generics ] "(" [ params ] ")" [ "->" type ] block ;
comb_decl     = "comb" "fn" IDENT [ generics ] "(" [ params ] ")" "->" type block ;
task_decl     = "task" IDENT [ generics ] "(" [ params ] ")" [ "->" type ] block ;
stage_decl    = "stage" IDENT [ generics ] "(" [ params ] ")" "@" clock block ;
comptime_decl = "comptime" "fn" IDENT [ generics ] "(" [ params ] ")" "->" type block ;

params        = param { "," param } [ "," ] ;
param         = IDENT ":" [ "in" | "out" ] type ;
clock         = IDENT [ "(" kv_list ")" ] ;
kv_list       = kv { "," kv } ;
kv            = IDENT "=" expr ;
```

### 2.3 型別

```
type          = base_type | fixed_type | range_type | bits_type | tuple_type
              | array_type | chan_type | mem_type | named_type | "(" type ")" ;
base_type     = "bit" | "bool" | UINT | SINT ;            (* UINT=u<n>  SINT=i<n> *)
fixed_type    = "fixed" "<" expr "," expr { "," fixed_opt } ">" ;   (* <I, F, ...> *)
fixed_opt     = ( "round" | "ovf" ) "=" IDENT ;
range_type    = "int" "<" expr "..=" expr ">" ;
bits_type     = "bits" "<" expr ">" ;
tuple_type    = "(" type { "," type } ")" ;
array_type    = "[" type ";" expr "]" ;
chan_type     = "chan" "<" type [ "," "depth" "=" expr ] ">" ;
mem_type      = "mem" "<" type "," expr ">" [ placement ] ;
placement     = "@" ( "reg" | "lutram" | "bram" | "uram" | "dram" ) [ "(" kv_list ")" ] ;
named_type    = path [ "<" targ { "," targ } ">" ] ;
targ          = type | expr ;
```

### 2.4 介面、排程、測試

```
iface_decl    = "iface" IDENT [ generics ] "{" ifield { "," ifield } [ "," ] "}" ;
ifield        = IDENT ":" type [ "@" "reverse" ] ;

schedule_decl = "schedule" path "{" { sched_item } "}" ;
sched_item    = "target" kv_list ";"
              | IDENT ":" sched_dir ";" ;                (* IDENT 為迴圈或變數標籤 *)
sched_dir     = "pipeline" "(" kv_list ")" | "unroll" "(" expr ")"
              | "share" "(" kv_list ")" | "retime" "(" kv_list ")" | placement ;

test_decl     = "test" IDENT block ;
```

### 2.5 陳述式

```
block         = "{" { stmt } [ expr ] "}" ;
stmt          = let_stmt | carry_stmt | assign_stmt | loop_stmt | if_stmt
              | match_stmt | spawn_stmt | parallel_stmt | stage_sep
              | assert_stmt | expr ";" | ";" ;

let_stmt      = "let" [ "mut" ] pattern [ ":" type [ placement ] ] [ "=" expr ] ";" ;
carry_stmt    = "carry" IDENT ":" type "=" expr ";" ;
assign_stmt   = lvalue ( "=" | "+=" | "-=" | "*=" | "&=" | "|=" | "^="
                       | "<<=" | ">>=" ) expr ";" ;
lvalue        = IDENT { "[" expr "]" | "." IDENT } ;

loop_stmt     = [ IDENT ":" ] [ loop_attr ] "for" pattern "in" expr block ;
loop_attr     = "pipeline" [ "(" kv_list ")" ] | "unroll" [ "(" expr ")" ] | "serial" ;

if_stmt       = "if" expr block [ "else" ( if_stmt | block ) ] ;
match_stmt    = "match" expr "{" arm { "," arm } [ "," ] "}" ;
arm           = pattern [ "if" expr ] "=>" ( expr | block ) ;

spawn_stmt    = "spawn" postfix_expr ";" ;               (* 必為呼叫 *)
parallel_stmt = "parallel" "{" { stmt } "}" ;
stage_sep     = ";;" ;                                  (* 時脈邊界，僅 stage *)
assert_stmt   = "assert" [ "always" | "eventually" ] expr ";" ;
pattern       = "_" | IDENT | "(" pattern { "," pattern } ")" ;
```

`for x in ch` 走訪通道直到生產者關閉；`for i in a..b` 走訪範圍。`pipeline` 不帶參數表示由編譯器求最小 II。

### 2.6 運算式

```
expr          = range_expr ;
range_expr    = or_expr [ ( ".." | "..=" ) or_expr ] ;
or_expr       = and_expr  { "||" and_expr } ;
and_expr      = cmp_expr  { "&&" cmp_expr } ;
cmp_expr      = bor_expr  [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) bor_expr ] ;
bor_expr      = bxor_expr { "|" bxor_expr } ;
bxor_expr     = band_expr { "^" band_expr } ;
band_expr     = shift_expr { "&" shift_expr } ;
shift_expr    = add_expr  { ( "<<" | ">>" ) add_expr } ;
add_expr      = mul_expr  { ( "+" | "-" | "+%" | "-%" ) mul_expr } ;
mul_expr      = unary_expr { ( "*" | "/" | "%" | "*%" ) unary_expr } ;
unary_expr    = ( "-" | "!" | "~" ) unary_expr | cast_expr ;
cast_expr     = postfix_expr { "as" type } ;
postfix_expr  = primary { call | index | slice | "." IDENT [ call ] } ;
call          = "(" [ expr { "," expr } ] ")" ;
index         = "[" expr "]" ;
slice         = "[" expr ":" expr "]" ;                  (* 位元切片 x[hi:lo]，含兩端 *)
primary       = literal | path [ "::" "<" targ { "," targ } ">" ]
              | "(" expr { "," expr } ")" | lambda | array_lit
              | "comptime" block | if_stmt | match_stmt | block ;
lambda        = "|" [ IDENT { "," IDENT } ] "|" expr ;
array_lit     = "[" [ expr { "," expr } ] "]" | "[" expr ";" expr "]" ;
literal       = INT | SIZED_INT | FIXED_LIT | "true" | "false" ;
```

泛型引數在型別位置直接寫 `<..>`，在運算式位置必須用 turbofish `::<..>`，以避免與比較運算子歧義。

**運算子優先序（高到低）**：後綴 > 一元 > `as` > `* / % *%` > `+ - +% -%` > `<< >>` > `&` > `^` > `|` > 比較 > `&&` > `||` > 範圍。`+%`、`-%`、`*%` 是明確的回繞運算，結果寬度等於左運算元寬度。

### 2.7 語境限制（由型別檢查器強制，不在文法內）

| 構造 | `comb fn` | `fn` | `task` | `stage` |
| --- | --- | --- | --- | --- |
| `let` 與算術 | 可 | 可 | 可 | 可 |
| `carry` | 不可 | 不可 | 可 | 可 |
| `pipeline` 迴圈 | 不可 | 不可 | 可 | 不可 |
| `unroll` 迴圈 | 可 | 可 | 可 | 可 |
| 通道阻塞 send/recv | 不可 | 不可 | 可 | 可 |
| 通道非阻塞 try\_recv/empty | 不可 | 不可 | 不可 | 可 |
| `spawn`、`parallel` | 不可 | 不可 | 可 | 不可 |
| `;;` 時脈邊界 | 不可 | 不可 | 不可 | 可 |
| `mem` 寫入 | 不可 | 不可 | 可 | 可 |

`fn` 是無狀態純函數，可被任何層級呼叫，由編譯器行內或共用為單一硬體模組（由 `schedule` 的 `share` 決定）。`comb fn` 額外保證零週期且無迴圈。

## 3. 型別系統

### 3.1 型別語法與等價

```
τ ::= int<a..=b>           範圍整數（a、b 為編譯期整數）
    | fixed<I,F,r,o>       有號定點：I 個整數位元（含符號）、F 個小數位元
    | bit | bits<N>        單一位元／無算術語意的位元向量
    | (τ1,..,τn) | [τ;n]    tuple、陣列
    | Tx<τ> | Rx<τ>        通道的生產端與消費端（chan<τ,depth=d> 建立一對）
    | mem<τ,n>@P           記憶體，P 為放置與分割資訊

uN  ≡ int<0..=2^N-1>       iN ≡ int<-2^(N-1)..=2^(N-1)-1>
bits(int<a..=b>) = a ≥ 0 ? max(1, ⌈log2(b+1)⌉)
                         : 1 + ⌈log2(max(-a, b+1))⌉
```

所有整數型別統一為範圍型別，位寬是範圍的**推導結果**而不是輸入，因此下列算術規則只需寫一套。

### 3.2 算術與位寬推導

```
(T-LIT)   n : int<n..=n>

(T-ADD)   e1 : int<a..=b>   e2 : int<c..=d>
          ────────────────────────────────
          e1 + e2 : int<a+c..=b+d>

(T-SUB)   e1 - e2 : int<a-d..=b-c>

(T-MUL)   e1 * e2 : int<min P..=max P>,   P = {ac, ad, bc, bd}

(T-SHL)   e << k  (k 為常數) : int<a·2^k..=b·2^k>
(T-SHR)   e >> k  (k 為常數) : int<⌊a/2^k⌋..=⌊b/2^k⌋>
(T-DIV)   e1 / e2 要求 e2 : int<c..=d> 且 0 ∉ [c,d]，否則 E0102
(T-MOD)   e1 % e2 : int<0..=max(|c|,|d|)-1>   （同樣要求除數非零）

(T-WRAP)  e1 +% e2 : 與 e1 同型別，結果取低位元（顯式回繞）
```

驗證：`u8 + u8` 的範圍為 `[0,510]`，需 9 位元；`u8 * u8` 為 `[0,65025]`，需 16 位元；`i16 * i16` 為 `[-1073709056, 1073741824]`，需 32 位元。結果與前版的 `u9`、`i32` 一致。

**定點數**：

```
(T-FADD)  fixed<I1,F1> + fixed<I2,F2> : fixed<max(I1,I2)+1, max(F1,F2)>
(T-FMUL)  fixed<I1,F1> * fixed<I2,F2> : fixed<I1+I2, F1+F2>
(T-FCONV) e.to::<fixed<I2,F2,round=r,ovf=o>>()
          r ∈ {trunc, nearest, even}    o ∈ {wrap, sat, check}
```

`iN` 可視為 `fixed<N,0>`，所以 `i16 * fixed<1,15>` 的型別是 `fixed<17,15>`。`check` 表示在模擬中斷言、在硬體中不產生邏輯。

### 3.3 子型別、窄化與迴圈攜帶

```
(S-RANGE)  [a,b] ⊆ [c,d]
           ──────────────────────────
           int<a..=b> <: int<c..=d>

(T-ASSIGN) e : τ1    τ1 <: τ2
           ───────────────────   （放大自動，窄化必須顯式）
           x : τ2 = e  合法

(T-SAT)    e : int<a..=b>   e.sat::<int<c..=d>>() : int<c..=d>
(T-TRUNC)  e.wrap::<T>() : T      取低位元，二補數語意
```

**迴圈攜帶變數**是前版 MAC 範例能抓到溢位的原因。設 `carry x : T = e0`，迴圈體每輪執行 `x += e`，其中 `e : int<lo..=hi>`，迴圈次數上界為 `Nmax`：

```
(T-CARRY)  e0 : int<a..=b>
           終值範圍  R = int<a + Nmax·lo ..= b + Nmax·hi>
           ─────────────────────────────────────────────
           需要  R <: T，否則 E0301（錯誤訊息列出需要的最小 T）
```

非加法累加（如乘法、移位）不做線性展開，編譯器要求使用者以 `.sat()`、`.wrap()` 或更大的 `T` 明示。

迴圈變數 `for i in 0..N` 的型別為 `int<0..=N-1>`，所以 `a[i]` 的索引位寬自動為 `⌈log2 N⌉`。

### 3.4 通道與線性資源

通道端點是**線性資源**，由資源環境 Δ 追蹤，與普通變數環境 Γ 分開。端點不可複製、不可無聲丟棄。

```
(T-CHAN)   let (tx, rx) = chan::<τ>(depth = d)
           ⟹ Δ' = Δ, tx : Tx<τ>, rx : Rx<τ>

(T-SPAWN)  task f(.., p : in chan<τ>, .., q : out chan<σ>)
           Δ, r : Rx<τ>, t : Tx<σ>  ⊢  spawn f(.., r, .., t)  ⊣  Δ
           （r、t 被消耗，之後本作用域不可再用）

(T-SEND)   t : Tx<τ> ∈ Δ    e : τ'    τ' <: τ   ⟹   t.send(e) : ()
(T-RECV)   r : Rx<τ> ∈ Δ                           ⟹   r.recv() : τ

(T-BCAST)  Δ, r : Rx<τ> ⊢ broadcast(r, n) : (Rx<τ>)^n ⊣ Δ     （r 被消耗）

(T-END)    作用域結束時，Δ 中每個端點必須已被消耗
           （spawn、broadcast、作為結果傳出、或顯式 close）
```

**靜態速率分析（SDF）**：若所有任務的每次觸發的讀寫個數都是編譯期常數，對每條通道 `c`（生產者 `p`、消費者 `q`）建立平衡方程：

```
rate_p(c) · reps(p) = rate_q(c) · reps(q)
```

方程無解代表速率不一致（E0501）；有解則得到最小重複向量，並以一次 PASS 模擬求每條通道所需的最小 FIFO 深度。含環的圖需要足夠的初始 token 才不會死結，由檢查器驗證。速率動態的通道則退回使用者指定的 `depth`，並警告無法證明無死結。

### 3.5 記憶體與仿射資源（bank 衝突判定）

資源環境 Σ 記錄每個「資源 × 埠類別 × 週期槽」的剩餘容量，每次存取消耗一單位，超額即錯誤（E0401）。在 `pipeline(ii = n)` 迴圈內，排在第 `c` 個週期的存取歸入槽 `c mod n`，所以所有同槽的存取共用容量。

**循環分割（cyclic，B 個 bank）**：`bank(addr) = addr mod B`。在 `unroll(k)` 內，存取位址為 `a·i + b`（`i` 為展開索引）：

```
g = gcd(a, B)
i 與 j 衝突  ⟺  (B/g) 整除 (i - j)
每個 bank 的最大同週期存取數 = ⌈ k·g / B ⌉
無衝突條件：  ⌈ k·g / B ⌉ ≤ p        （p 為該存取類別的埠數）
```

| 存取 | 設定 | k | 需求 | 結果 |
| --- | --- | --- | --- | --- |
| `buf[i]` 寫 | B=4，1r1w | 4 | ⌈4·1/4⌉ = 1 | 通過 |
| `buf[i+1]` 讀 + `buf[i]` 寫 | B=4，1r1w | 4 | 讀 1、寫 1 | 通過 |
| `buf[i+1]` 讀 + `buf[i]` 寫 | B=4，單埠 p=1 | 4 | 同 bank 共 2 | E0401 |
| `buf[2*i]` | B=4，p=1 | 4 | g=2，⌈4·2/4⌉ = 2 | E0401 |
| `buf[i]` | B=4，p=1 | 8 | ⌈8/4⌉ = 2 | 需 `ii = 2` 或 B=8 |

**區塊分割（block）**：以區間分析判定 k 個位址是否落在互異區塊或不超過 p 個同區塊位址。**資料相依位址**（索引來自運算結果）視為可命中任何 bank，只有在埠數 ≥ 該週期存取數時才通過，或必須顯式使用 `arbitrate(...)` 並接受不固定的延遲。

### 3.6 時序與 II 檢查

型別檢查器從目標元件庫（延遲與 DSP/BRAM 埠數）取得每個運算的延遲估計。

```
RecMII = max over 相依環 C of  ⌈ Σ_{e∈C} latency(e) / Σ_{e∈C} distance(e) ⌉
ResMII = max over 資源 r of    ⌈ uses(r) / capacity(r) ⌉

(T-PIPE)   pipeline(ii = n) 合法  ⟺  n ≥ max(RecMII, ResMII)
           否則 E0412，並列出構成 RecMII 的關鍵環或超額使用的資源
```

`comb fn` 的組合路徑延遲必須小於時脈週期，超過是錯誤 E0210。`task` 內則由排程器自動插入暫存器（retime）。

### 3.7 `stage` 層：值的有效區間

`stage` 內的運算式帶有週期標註，借用 Filament 的想法：

```
x : τ @ [t, t+n)      x 在週期 t 產生，保證有效 n 個週期

(T-USE)   x : τ @ [t, t+n)    使用發生於週期 u    t ≤ u < t+n
          ─────────────────────────────────────────────────────
          使用合法；否則 E0230（值已失效或尚未產生）

(T-SEP)   每個  ;;  使目前週期 +1
(T-REG)   reg(x) : τ @ [u, ∞)      以暫存器延長有效區間
```

與 `task` 的互通：`task` 呼叫 `stage` 時，`stage` 的輸入需求區間與輸出有效區間成為其合約；編譯器在邊界插入握手轉接器。

### 3.8 `comptime` 與圖建構

- `comptime` 函式在編譯期以直譯器執行，**不得**讀取執行期值，結果必須是封閉項。
- 預設步數上限 10^6，避免不終止；超限報 E0601。
- `Graph` 是線性值：每個埠恰好被連接一次，建構結束時檢查未連接與重複連接的埠。
- 同樣輸入保證同樣輸出（無隨機、無 I/O），所以生成結果可快取。

### 3.9 介面型別

```
iface AxiStream<T> { data: T, valid: bit, ready: bit @reverse, last: bit }
```

宣告為 `out` 的參數：正向欄位由本任務驅動，`@reverse` 欄位由對端驅動；宣告為 `in` 則相反。編譯器依型別產生握手狀態機，並自動加入協定斷言，例如 `valid` 拉高後在 `ready` 之前不得變更 `data`。

### 3.10 預期的性質

以下為設計目標，完整證明列入實作計畫（§9）。

1. **位寬安全**：良型程式若不含 `.wrap`、`.sat`、`+%` 等顯式窄化，執行中任何值都不會超出其型別範圍。
2. **資源安全**：良型程式在每個週期、每個 bank 的存取數不超過埠數。
3. **通道安全**：每條通道恰有一個生產者與一個消費者，不存在別名。
4. **排程保證**：`pipeline(ii = n)` 通過檢查，則在指定目標模型下，產生的電路啟動間隔不超過 n。
5. **延遲不敏感等價**：`task` 層程式的每條輸出通道上的 token 序列，與參考語意（Kahn 程序網路）相同，且與實作延遲、FIFO 深度無關（深度不足只會降低吞吐，不會改變結果，除非死結）。這個性質要成立，`task` 層必須禁止非阻塞的 `try_recv` 與 `empty`，因為它們會引入非決定性；這些操作只允許出現在 `stage`。

## 4. 完整範例 A：FIR 濾波器

**規格**：16 階低通 FIR，輸入 `i16` 串流，係數 `fixed<1,15>`，輸出 `i16`（最近捨入、飽和），目標 300 MHz、II = 1。

```rust
use std::dsp::{design_lowpass, hamming};

const TAPS: u32 = 16;

// 係數在編譯期由視窗法產生，並量化為 fixed<1,15>
const COEF: [fixed<1, 15>; TAPS] = comptime {
    design_lowpass(taps = TAPS, cutoff = 0.2, window = hamming)
        .quantize::<fixed<1, 15, round = nearest>>()
};

task Fir(x: in chan<i16>, y: out chan<i16>) {
    let mut hist: [i16; TAPS - 1] @reg = [0; TAPS - 1];   // 歷史樣本，全放暫存器

    pipeline(ii = 1) for s in x {
        let win: [i16; TAPS] = concat([s], hist);          // win[0] 是最新樣本
        let acc = (0..TAPS).unrolled()
                           .map(|k| win[k] * COEF[k])      // i16 * fixed<1,15> : fixed<17,15>
                           .tree_sum();                    // 4 層加法樹    : fixed<21,15>
        hist = win[0..TAPS - 1];                           // 迭代結束時提交
        y.send(acc.to::<i16>(round = nearest, ovf = sat));
    }
}

schedule Fir {
    target fmax = 300MHz, device = xcu250;
    mul:  share(factor = 1);     // 每個乘法獨立一個 DSP
    hist: @reg;
}
```

**暫存器語意**：`@reg` 變數在一次迭代內讀到的永遠是迭代開始時的值，寫入在迭代結束才提交。這讓 `hist = win[0..TAPS-1]` 與硬體的移位暫存器一致，不需要手動排順序。

### 4.1 型別檢查的推導結果

| 運算式 | 型別 | 說明 |
| --- | --- | --- |
| `s` | `i16` = `fixed<16,0>` | 輸入通道元素型別 |
| `win[k] * COEF[k]` | `fixed<17,15>` | T-FMUL：16+1 位整數、0+15 位小數 |
| `tree_sum()` 16 項 | `fixed<21,15>` | 每層加法 +1，共 4 層 |
| `acc.to::<i16>(...)` | `i16` | 小數 15 位以最近捨入去除，超出範圍飽和 |

**常數係數的區間精化**：`COEF` 是編譯期常數，編譯器可算出 Σ|h\[k\]|。若 Σ|h\[k\]| ≤ 1，則輸出必在 `i16` 範圍內，此時 `ovf = check` 可被證明永不觸發，飽和比較器會被自動移除；若寫 `ovf = sat` 則保留。

### 4.2 排程變體

| 設定 | II | DSP | 取樣率 @300 MHz | 條件 |
| --- | --- | --- | --- | --- |
| 預設 | 1 | 16 | 300 Msps | 無 |
| `mul: share(factor = 2)`，`ii = 2` | 2 | 8 | 150 Msps | 無 |
| `fold_symmetric`（利用 pre-adder） | 1 | 8 | 300 Msps | 係數對稱，由 comptime 證明 |

若 `COEF` 不對稱卻要求 `fold_symmetric`，編譯器在 comptime 階段報錯並指出第一個不對稱的 tap。

### 4.3 預期電路（示意估算，非實測）

```
 x ──► [FIFO] ──► s ──┬──────────────────────────────┐
                      │      hist: 15 × 16b 移位暫存器  │
                      ▼                               ▼
               win[0] ... win[15]
                  │          │          │
                 ×c0        ×c1  ...   ×c15     16 個 DSP48E2
                  └────┬─────┘          │
               加法樹 L1 (8) → L2 (4) → L3 (2) → L4 (1)
                                  │
                         round（最近）→ sat
                                  │
                                  ▼
                       [FIFO] ──► y
```

| 項目 | 估計值 |
| --- | --- |
| DSP | 16（對稱摺疊版 8） |
| FF | 約 700（歷史線 240 + 管線暫存器） |
| LUT | 約 300（加法樹與 round / sat） |
| BRAM | 0 |
| 延遲 | 約 9 週期：乘法 3 + 加法樹 4 + round 1 + sat / 輸出 1 |
| 吞吐 | 每週期 1 個樣本 |

### 4.4 測試與診斷範例

```rust
test fir_impulse {
    let (tx, rx) = chan::<i16>(depth = 4);
    let (ty, ry) = chan::<i16>(depth = 4);
    spawn Fir(rx, ty);

    tx.send(16384);
    for _ in 0..TAPS { tx.send(0); }
    tx.close();

    // 單位脈衝響應 = 係數本身
    for k in 0..TAPS {
        assert ry.recv() == (COEF[k] * 16384).to::<i16>(round = nearest, ovf = sat);
    }
}
```

若在 schedule 裡誤設 `share(factor = 2)` 卻仍寫 `pipeline(ii = 1)`：

```
error[E0412]: pipeline(ii = 1) 無法達成
  --> fir.fx:14:5
   |
14 |     pipeline(ii = 1) for s in x {
   |     ^^^^^^^^^^^^^^^^ 要求 II = 1，最小可達 II = 2
   |
   = 瓶頸：資源 DSP（每迭代 16 次乘法，可用 8 個，來自 schedule 的 share(factor = 2)）
   = 修正 1：移除 share，使用 16 個 DSP
   = 修正 2：改為 pipeline(ii = 2)
```

## 5. 完整範例 B：脈動矩陣乘法

**規格**：C = A × B，A 為 N×K、B 為 K×N，輸入 `i8`，累加 `i32`，N = 16 為編譯期參數，K ≤ 65535 為執行期參數。

### 5.1 處理單元（PE）

```rust
task Pe(
    a_in:  in  chan<i8, depth = 2>,  b_in:  in  chan<i8, depth = 2>,
    a_out: out chan<i8, depth = 2>,  b_out: out chan<i8, depth = 2>,
    k: u16,
    c_out: out chan<i32>
) {
    // i8 * i8 ∈ [-16256, 16384]；累加 65535 次 ∈ [-1065336960, 1073725440]
    // 編譯器推導最小型別為 i31，這裡用 i32 對齊 DSP 累加器
    carry acc: i32 = 0;

    pipeline(ii = 1) for _ in 0..k {
        let a = a_in.recv();
        let b = b_in.recv();
        acc += a * b;
        a_out.send(a);
        b_out.send(b);
    }
    c_out.send(acc);
}
```

本例只處理單一 tile；多 tile 由外層迴圈包住並重設 `acc`。

### 5.2 以 comptime 組出陣列

```rust
comptime fn systolic<const N: u32>() -> Graph {
    let g = Graph::new();
    let pe = array2d(N, N, |_, _| g.add::<Pe>());

    let a = g.input_vec::<i8>(N);       // a[i]：西側第 i 列輸入
    let b = g.input_vec::<i8>(N);       // b[j]：北側第 j 行輸入
    let c = g.output_grid::<i32>(N, N); // c[i][j]：各 PE 的結果

    for i in 0..N {
        for j in 0..N {
            // a 往東傳，b 往南傳；邊界輸出接 sink 吸收
            if j + 1 < N { g.connect(pe[i][j].a_out, pe[i][j + 1].a_in); }
            else         { g.sink(pe[i][j].a_out); }
            if i + 1 < N { g.connect(pe[i][j].b_out, pe[i + 1][j].b_in); }
            else         { g.sink(pe[i][j].b_out); }
            g.bind(c[i][j], pe[i][j].c_out);
        }
    }
    for i in 0..N { g.bind(a[i], pe[i][0].a_in); }
    for j in 0..N { g.bind(b[j], pe[0][j].b_in); }
    g.broadcast_param(k, pe.map(|p| p.k));   // 執行期參數 k 廣播給全部 PE
    g
}
```

圖是線性值：若漏接 `pe[3][15].a_out`，comptime 結束時報 E0603（埠未連接）；重複連接同一埠同樣是 E0603。

### 5.3 頂層

```rust
const N: u32 = 16;

task Gemm(
    a_in:  in  AxiStream<[i8; N]>,   // 每拍 N 個 A 元素（每列一個）
    b_in:  in  AxiStream<[i8; N]>,   // 每拍 N 個 B 元素（每行一個）
    k:     u16,
    c_out: out AxiStream<i32>        // 結果以列主序輸出
) {
    let arr = systolic::<N>();
    spawn unpack(a_in, arr.a);                 // [i8; N] 拆成 N 條通道
    spawn unpack(b_in, arr.b);
    spawn arr.run(k);                          // 啟動 N*N 個 PE
    spawn collect_rowmajor(arr.c, c_out);      // N*N 個結果合併為單一串流
}

schedule Gemm {
    target fmax = 300MHz, device = xcu250;
    Pe.mul: @dsp;                              // 乘加綁定到 DSP 的 MACC 模式
}
```

**為什麼不需要手寫 skew**：每個 PE 的 `recv()` 都是阻塞式，必須兩個輸入都到才觸發，所以資料在陣列中自然形成斜向波前。傳統 HLS 要手動寫延遲暫存器，這裡由握手語意免費取得。

**為什麼 depth = 2**：SDF 分析顯示若要 II = 1 全速，相鄰 PE 之間的 FIFO 最小深度為 2（因為 ready 訊號經過暫存）。若寫成 `depth = 1`，編譯器發出 W0502 並報告吞吐降為 1/2。

### 5.4 編譯期檢查結果

| 檢查 | 結果 |
| --- | --- |
| 累加位寬（T-CARRY） | 終值範圍 \[-1065336960, 1073725440\] ⊆ `i32`，通過；最小型別 `i31` |
| 通道線性 | 每個埠恰連接一次，通過 |
| SDF 平衡 | 全部速率為 1，方程有解；最小深度 2 |
| RecMII | 32 位元累加在 DSP 內部累加器為 1 週期，RecMII = 1 |
| ResMII | 每 PE 1 個 DSP 乘加，ResMII = 1 |

若把 `acc` 宣告成 `i24`：

```
error[E0301]: carry 變數 acc 的型別 i24 太小
  --> gemm.fx:9:11
   |
 9 |     carry acc: i24 = 0;
   |           ^^^^^^^^^^^^ 終值可能超出 [-8388608, 8388607]
   |
   = 每次累加項：[-16256, 16384]（i8 * i8）
   = 迴圈上界：k ≤ 65535（u16）
   = 終值範圍：[-1065336960, 1073725440]
   = 修正 1：改為 i31 或 i32
   = 修正 2：把 k 限制為 int<0..=512>，此時 i24 足夠
```

### 5.5 預期電路（N = 16，示意估算）

```
          b0         b1         b2
           │          │          │
 a0 ──► PE00 ────► PE01 ────► PE02 ──► sink
           │          │          │
 a1 ──► PE10 ────► PE11 ────► PE12 ──► sink
           │          │          │
 a2 ──► PE20 ────► PE21 ────► PE22 ──► sink
           ▼          ▼          ▼
         sink       sink       sink

每個 PE：DSP48E2 MACC（8×8 乘、32 位累加）+ 2 個深度 2 的 FIFO + 16 位 k 計數器
```

| 項目 | 估計值 |
| --- | --- |
| DSP | 256 |
| FF | 約 23000 |
| LUT | 約 15000 |
| BRAM | 0（資料由外部串流供應） |
| 峰值吞吐 | 256 MAC/週期 × 300 MHz = 76.8 GMAC/s |
| 單 tile 延遲 | PE(i,j) 在第 i+j 週期開始，最後一個 PE 於 k + 2N - 2 = k + 30 週期完成 |
| 輸出排空 | 256 個結果經單一串流，需 256 週期（可與下一 tile 重疊） |

當 K = 256 且不重疊時，利用率約 256 / (256 + 30) ≈ 89%；以雙緩衝累加器重疊輸出與下一 tile 計算，可逼近 100%，這是第一個列入路線圖的最佳化。

## 6. 編譯器架構

```
原始碼 .fx
  │  詞法 / 語法分析
  ▼
AST
  │  名稱解析 + comptime 求值（泛型單態化、Graph 展開）
  ▼
型別 / 資源檢查：位寬範圍、線性通道、bank 衝突、SDF、II      ← 錯誤在此報告
  ▼
Fluxa IR（MLIR dialect：fx.task / fx.chan / fx.loop / fx.mem）
  │  降階
  ▼
資料流 IR（CIRCT 的 Handshake、DC 等 dialect）
  │  排程（SDC / ILP，目標 fmax、II、資源）+ 綁定（DSP / BRAM / LUT）
  ▼
FSM + 管線展開（CIRCT 的 hw / comb / seq）
  ▼
SystemVerilog + testbench + 廠商約束檔
```

### 6.1 雙模擬器與等價測試

- **行為模擬**：把 `task` 當作 Kahn 程序網路直接解譯，速度快，用於演算法開發。
- **週期精確模擬**：由 IR 生成 C++ 模型（或接 Verilator），含 FIFO 深度與排程延遲。
- **等價測試**：同一個 `test` 在兩個模擬器上都跑，比對每條輸出通道的 token 序列。依 §3.10 性質 5，兩者必須一致，不一致視為編譯器 bug，這取代了傳統 HLS 的 C-sim / Co-sim 差異問題。

### 6.2 診斷設計原則

每個錯誤訊息必須包含三件事：位置、**證據**（具體數值、關鍵環、超額資源）、至少一個**可行修正**。

| 代碼 | 意義 |
| --- | --- |
| E0102 | 除數範圍包含 0 |
| E0210 | `comb fn` 路徑延遲超過時脈週期 |
| E0230 | 值在使用週期已失效或尚未產生 |
| E0301 | 窄化或 `carry` 溢位 |
| E0401 | bank / 埠衝突 |
| E0412 | `pipeline(ii)` 無法達成 |
| E0501 | SDF 速率不一致 |
| E0601 | comptime 超過步數上限 |
| E0603 | 圖的埠未連接或重複連接 |
| W0502 | FIFO 深度不足導致吞吐下降 |

## 7. 標準庫概要

| 模組 | 內容 |
| --- | --- |
| `std::num` | `sat`、`wrap`、`abs`、`clz`、`popcount`、`to::<T>(round, ovf)` |
| `std::stream` | `unpack`、`pack`、`zip`、`broadcast`、`merge`、`arbitrate`、`fifo` |
| `std::mem` | `rom`、`ram`、`line_buffer`、`ping_pong` |
| `std::axi` | `AxiStream`、`AxiLite`、`AxiMaster`（含 burst） |
| `std::dsp` | `design_lowpass`、窗函數、`tree_sum`、`fir`、`cordic` |
| `std::test` | 隨機串流、黃金模型比對、覆蓋率 |

`e.to::<T>(round = r, ovf = o)` 是 T-FCONV 的簡寫：當 `T` 為整數型別時，等價於 `to::<fixed<N,0,round=r,ovf=o>>()` 再轉回 `T`。

## 8. 風險與開放問題

- **三層時間模型的互通**：`task` 呼叫 `stage` 的合約是否都能自動推導，還是需要使用者標註，目前未定。
- **動態速率資料流**：輸出個數依資料而定時，死結檢查不可判定，只能退化為深度提示加模擬期的看門狗。
- **區間分析的保守性**：資料相依或非線性累積可能算出過寬的位寬。需要 `assume x < 100;` 這類使用者斷言，並決定它在 RTL 中是丟棄還是保留為 SVA。
- **排程品質**：要追上 Vitis HLS 的 QoR，需要精確的元件延遲模型，初期很可能落後。
- **浮點數**：本文未涵蓋。可能以 `float<E,M>` 加 IP 庫對接，多週期延遲會使 RecMII 變大，需要在型別層明確表達。
- **外部 IP 與遺留 RTL**：需要 `extern stage` 宣告其時序合約。
- **學習曲線**：線性與仿射型別對硬體工程師陌生，錯誤訊息與教學的品質決定可用性。
- **生態依賴**：建在 CIRCT 上可省大量工作，但其 dialect 仍在變動，需要鎖定版本並預留升級成本。

## 9. 實作路線圖

| 階段 | 範圍 | 驗收標準 |
| --- | --- | --- |
| M1 | 詞法與語法、範圍整數型別、`task` / `chan`、`pipeline(ii)` 檢查、產出 Verilog | FIR 與 MAC 在 Verilator 與 FPGA 上和參考模型一致 |
| M2 | `mem` bank 檢查、`schedule` 分離、SDF 分析、雙模擬器等價測試 | 矩陣乘與卷積核通過等價測試 |
| M3 | comptime `Graph`、`iface` 與 AXI 自動生成、標準庫 | 16×16 脈動陣列在 300 MHz 時序收斂 |
| M4 | `stage` 層、時序屬性斷言、`extern` IP | 對接 DDR 控制器與 PCIe DMA |
| M5 | 核心演算的機械化證明（Coq 或 Lean）、QoR 對標 | §3.10 性質 1 至 5 完成證明；與 Vitis HLS 比較基準 |

**建議先做的三件事**：(1) 範圍整數型別與 `carry` 檢查，這是最快能展示價值的部分；(2) 用 CIRCT 的 Handshake 路徑打通 `task` 到 Verilog；(3) 把 FIR 與脈動陣列兩個範例寫成回歸測試，之後每個階段都必須保持通過。
