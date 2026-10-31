# 17. Fluxa

Fluxa 是 rueda 的第二个硬件语言，用于高层次综合（HLS）。Veryl 一个时钟一个时钟地描述寄存器与导线；Fluxa 程序描述的是
**用通道连起来的任务**：每个 `task` 各自运行，通道是有界 FIFO，FIFO 满了 `send` 就等、空了 `recv` 就等。
这是 Kahn 进程网络，所以结果与任务怎么调度、FIFO 有多深都无关（只有死锁会受影响）。
语言规格在 [`fluxa/docs/Fluxa語言規格v0.1.md`](../../../fluxa/docs/Fluxa語言規格v0.1.md)（繁体中文）。

```fluxa
task Double(x: in chan<i16>, y: out chan<i17>) {
    for v in x { y.send(v * 2); }          // x 关闭且清空时结束
}

test doubles {
    let (tx, rx) = chan::<i16>(depth = 4);
    let (ty, ry) = chan::<i17>(depth = 4);
    spawn Double(rx, ty);                  // 端点的所有权转给任务
    for k in 0..3 { tx.send(k); }
    tx.close();
    for k in 0..3 { assert ry.recv() == 2 * k; }
}
```

## 命令行

`reda sim` 与 `reda list` 接收 `.fx` 文件，就像接收 `.veryl` 文件（扩展名决定语言）：

```sh
reda sim fluxa/examples/fir.fx                  # 文件里所有的 test
reda sim fluxa/rv32i/rv32i.fx fluxa/rv32i/tests_basic.fx   # 多个文件合并成一个程序
reda sim fluxa/examples/fir.fx --test fir_impulse
reda sim fluxa/examples/gemm.fx --stats         # 每条通道：送出、收到的 token 数，FIFO 最大占用
reda sim fluxa/examples/mac.fx --trace          # 每一次 send 与 recv
reda sim fluxa/examples/fir.fx --coop           # 浏览器里的调度器（见下）
reda list fluxa/examples/fir.fx                 # 列出 test
```

全部通过时退出码是 0；有 test 失败（`assert`、值放不进它的类型、死锁）时是 1；语法错误是 2。
死锁会列出每个任务当时在等什么。

## 两种调度

命令行上每个任务是一条操作系统线程。WebAssembly 没有线程，所以在浏览器里所有任务在同一条线程上轮流跑：
解释器是异步的，每个通道操作都是任务可以停下来、让别的任务先跑的地方。`--coop` 在命令行上也用这个调度器。
两种的结果相同（回归测试把每个示例都用两种方式跑过并比对）。

## 网页 IDE

- 欢迎页的 **＋ 新 Fluxa 项目** 会建立一个有一个 `.fx` 文件的项目；在 Fluxa 项目里 **＋ 新增文件** 会建 `.fx` 文件。
  编辑器会为 Fluxa 着色；打字时语法错误就会出现在「问题」面板。
- **▶ 运行** 跑 test（全部，或从侧栏的「测试」点一个）。「输出」面板显示每个 test 的 `print` 输出、
  `[PASS]` / `[FAIL]` 与原因，以及通道统计。
- 示例库有 **Fluxa** 一组：基本语法、FIR 滤波器、乘加器、脉动阵列矩阵乘法、broadcast 扇出、（故意的）死锁，
  4 阶 FIR、任务网络，用四个流水线任务写成的 RV32I 处理器，
  以及 RV64G 处理器（M/S/U 特权模式、Sv39、IEEE-754 FPU、迷你 OS；93 个 test）。

## 综合

`reda synth / fpga / asic / gatesim` 也接收 `.fx` 文件，前端产生和 Veryl 一样的门级网表（`eda::netlist::Netlist`），
后面的 FPGA 与 ASIC 流程不变。

```sh
reda synth --top Fir fluxa/examples/fir.fx          # 网表统计；-o 写 yosys JSON、--verilog 写 Verilog
reda fpga --asc fir4.asc --bin fir4.bin fluxa/examples/fir4.fx   # iCE40 布局布线与比特流
reda asic --pdk sky130 --gds fir.gds fluxa/examples/fir.fx       # SKY130 芯片版图
reda gatesim ice40 fluxa/examples/fir.fx            # test 照跑，但 task 换成它的 iCE40 网表
reda synth --list fluxa/examples/pipeline.fx        # 可以综合的 task
```

- 顶层是 `--top` 指定的 task（默认最后一个）。每个通道参数变成流端口 `NAME_data`、`NAME_close`、
  `NAME_valid`、`NAME_ready`（valid / ready 握手；close 是流结束），其他参数是输入端口，或用
  `--param NAME=VALUE` 固定成常数。另有 `clk` 与 `rst`。
- 一个 task 是一个有限状态机加数据路径：每个通道操作是状态的边界，中间的纯计算在一个时钟里做完。
  位宽照规格的范围推导；定点数的舍入与溢出模式、`@reg`（读到旧值、迭代结束提交）、数组、`unrolled()`、
  函数都会展开成门。
- `pipeline(ii = 1)` 的循环变成一个状态：输入都 valid、输出都 ready 的那个时钟就跑一次迭代（FIR 一个时钟一个样本）。
  每个通道每次迭代最多收一次、送一次，通道操作不能放在 `if` 里，收不能在送之后；做不到时报 E0412 并说明原因
  （`ii ≥ 2` 则退回一般状态机）。
- `let m: mem<T, N> = [0; N]` 只用 `m[i]` 读写时，每次访问占一个时钟（读出的字直接进寄存器），
  iCE40 会把它做成 block RAM（`SB_RAM40_4K`）；其他用法退回寄存器数组并给警告。
- 建网络的 task（`chan`、`spawn`、`broadcast`）整个变成一个网表：每个 spawn 一个电路，每条内部通道一个
  照它的 `depth` 做的 FIFO（`fluxa/examples/pipeline.fx`）。
- 通道可以传 tuple；数组参数是一个输入端口（全部位）；`match` / `if` 的分支里可以有通道操作与 `return`。
  RV32I 处理器的四个阶段（Fetch、Decode、Execute、Mem）都综合得出来，24 个 test 在门级全部通过：
  `reda gatesim opt fluxa/rv32i/rv32i.fx fluxa/rv32i/tests_basic.fx`（每个 task 换成网表，约 7 分钟）。
- 泛型 task 的 const 泛型参数用 `--param NAME=VALUE` 给值（在网络里或门级测试时，取 spawn 的 `::<…>`）。
  网络的数组参数变成它的一个输入端口，接到拿到它的实例。所以 RV32I 的 `Core` 是一个网表（四个阶段加上中间的
  FIFO，约 36,500 个通用门）：`reda synth --top Core --param WBD=8 fluxa/rv32i/rv32i.fx`；
  `reda gatesim opt --top Core fluxa/rv32i/rv32i.fx fluxa/rv32i/tests_basic.fx` 在它上面跑 24 个 test（看门狗那个要跑
  3,000 条指令，约半小时；其余 23 个约一分钟）。
- `--param` 的值若不是整数，就是在程序里求值的 Fluxa 表达式，所以数组参数也能是常数：用索引读它就是 ROM
  （并进逻辑门），用它当初值的存储器一开始就是这些字。程序与数据内建的 RV32I `Core` 只剩两条输出流当端口，
  复位后就跑程序，在 ECP5 上完成布局布线（21,163 LUT4、5,516 FF、72 个 IO，约 16 MHz）：
  ```
  reda fpga --arch ecp5 --bit core.bit --top Core --param WBD=8 --param 'data=dimage([])' \
       --param 'prog=image([addi(1, 0, 0), addi(2, 0, 1), addi(3, 0, 11), add(1, 1, 2), addi(2, 2, 1), bne(2, 3, -8), ecall()])' \
       fluxa/rv32i/rv32i.fx
  ```
- 值域：`if` 的每个分支里（以及 `&&` / `||` 的右边），条件界定的变量有较窄的值域，所以
  `if sh <= 0 { x << (0 - sh) }`、`if b != 0 { a / b }` 都能综合；函数的返回值保留比声明类型窄的值域。
- 没有通道操作的 `serial for` 每次迭代一个时钟（状态）；其他这种循环展开成逻辑门。长的运算就是这样变成
  多周期单元，迭代结束就是时钟边界。
- 用数组参数当初值的存储器（`let m: mem<T, N> = data`）一开始就是它的字：有 `--param data=…` 时是
  block RAM 的初值（iCE40 `INIT_*`、ECP5 `INITVAL_*`），否则门级仿真时直接加载（不占引脚）。同一个存储器的
  读取共用一个读端口。
- RV64G 处理器（`fluxa/rv64`）综合成一个网表，在 ECP5-85 上完成布局布线：72,834 个 LUT 位置（87%）、
  12,179 FF、32 个 DP16KD、8.35 MHz。它的 FPU 是 `fpu.fx` 里的多周期 task `Fpu`（乘法 53 拍、除法 116 拍、
  开方 60 拍，精确加法器与舍入器只有一份、大家共用），在 `tests_fpu.fx` 的每个向量上与 `comb fn` 参考模型
  `fp_op` 逐位相同；M 扩展是 64 拍的 task `MulDiv`。
  ```
  reda fpga --arch ecp5 --clock 5 --bit rv64.bit --top Core --param WBD=8 --param TLBN=8 --param 'data=dimage([])' \
       --param 'prog=image([addi(1, 0, 0), addi(2, 0, 1), addi(3, 0, 11), add(1, 1, 2), addi(2, 2, 1), bne(2, 3, -8), simexit()])' \
       fluxa/rv64/rv64.fx fluxa/rv64/fpu.fx
  ```
- 网页 IDE 的 Fluxa 项目也有「⚙ 综合」：① 显示 task 与它的实例，②–⑦ 和 Veryl 一样；
  「仿真对象」选通用门 / iCE40 / 标准单元时，▶ 运行 会把综合顶层换成它的网表再跑 test，
  「✔ 全面验证」把每种实现的结果并排比对。虚拟板不接收 Fluxa（它的端口是数据流，不是板子的引脚）。
  示例「FIR 滤波器（4 阶）」放得进 iCE40，「任务网络综合成一个电路」示范网络。

设计与进度见 `fluxa/docs/synthesis-plan.md`。
