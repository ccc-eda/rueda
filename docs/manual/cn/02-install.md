# 2. 安装

## 不安装：在线版

打开 <https://ccc-eda.github.io/rueda/web/dist/index.html> 就是完整的 IDE（第 11 章）。
它不需要服务器，下载 `web/dist/` 整个目录后，直接用浏览器开 `index.html` 也能脱机使用。

## 安装 `reda`

需要 Rust（<https://rustup.rs>）。`reda` 像 `git` 一样，一个指令包含所有工具（仿真、综合、FPGA 与 ASIC 流程、
C 编译器、仿真机），从仓库安装：

```sh
cargo install --git https://github.com/ccc-eda/rueda rueda
reda version
```

## 从源代码建置

```sh
git clone https://github.com/ccc-eda/rueda
cd rueda
./build.sh            # Rust 工具（release）与 web/dist；--no-web 只建 Rust 工具
```

建好的指令是 `target/release/reda`。可以把这个目录加进 `PATH`，或在项目里任何地方用
`cargo run --release --bin reda -- ...`。建网页需要 wasm32 target 与 Node.js（`build.sh` 会自动补装 target 与 npm 套件）。

## 外部工具（依需要）

| 工具 | 用途 | 从哪里来 |
|---|---|---|
| `iceprog` | 把 `.bin` 烧进实体 iCEBreaker | [OSS CAD Suite](https://github.com/YosysHQ/oss-cad-suite-build/releases) |
| `nextpnr-ice40`、`icepack` | `reda fpga` 默认的布局布线工具（用 `--pnr veryl` 就不需要） | OSS CAD Suite |
| `nextpnr-ecp5`、`ecppack`、`openFPGALoader` | ECP5（`reda fpga --arch ecp5`）的布局布线、比特流、刻录 | OSS CAD Suite |
| IceStorm 的 `chipdb-5k.txt` | `--pnr veryl`（自研布局布线）需要的芯片数据库 | OSS CAD Suite 的 `share/icebox/`；或用 `VERYL_EDA_CHIPDB` 指定 |
| `gtkwave` | 看 `reda sim --vcd` 产生的波形 | OSS CAD Suite 或系统套件 |
| 官方 `veryl` | `veryl check` / `veryl test` / `veryl build`（语法的标准答案） | `cargo install veryl` |
| KLayout | 看 `.gds` 芯片版图；`eda/tools/gds_check.py` 用它的 Python 模块 | <https://www.klayout.de>，Python 模块：`pip install klayout` |
| 完整 SKY130 文件 | 只有 `reda pdk`、`--pdk sky130:目錄` 需要（`--pdk sky130` 用内附的子集） | `eda/tools/sky130.sh` 下载 |

没有 OSS CAD Suite 也能做大部分的事：仿真、综合、`reda fpga --pnr veryl`（只要有芯片数据库）、ASIC 流程、C 程序。

## 环境变量

| 变量 | 用途 |
|---|---|
| `VERYL_EDA_CHIPDB` | IceStorm `chipdb-5k.txt` 的路径（自研布局布线用） |
| `VERYL_EDA_SKY130` | 完整 SKY130 hd 文件的目录（`reda pdk`） |
| `VERYL_EDA_PROFILE` | 设置后打印 ASIC 流程各步骤的耗时与详细布线每一轮的冲突数 |
| `YOSYS_SLANG`、`GDS_PY` | 开发者的验证引脚本用（第 13 章） |
