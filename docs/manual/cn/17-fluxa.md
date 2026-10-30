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
  用四个流水线任务写成的 RV32I 处理器，
  以及 RV64G 处理器（M/S/U 特权模式、Sv39、IEEE-754 FPU、迷你 OS；90 个 test）。

## 还没有的

Fluxa 目前只有行为仿真。`reda synth / fpga / asic / gatesim` 不接收 `.fx` 文件，IDE 在 Fluxa 项目里也会把这些按钮关掉。
综合要做的是一个 Fluxa 前端，产生和 Veryl 一样的门级网表（`eda::netlist::Netlist`）；从那里开始，FPGA 与 ASIC 流程都不用改。
