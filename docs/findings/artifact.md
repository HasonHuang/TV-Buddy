# 目标指纹与取得记录（artifact）

> 遵循 [`../spec/01-target-and-scope.md`](../spec/01-target-and-scope.md) §1、§2。
> **在登记指纹前，任何"对目标的分析"都必须标注所分析的具体副本来源。**

## 1. 待取样本

| 产物 | 架构 | 状态 |
| --- | --- | --- |
| `iptv-proxy-linux-arm64` | arm64 | ⏳ 未取得 |
| `iptv-proxy-linux-amd64` | amd64 | ⏳ 未取得 |

- 版本：**v0.8.2**（基线版本）
- 取得方式：安装脚本 / 发布资产（URL 与时间需登记）
- 本地路径：（待填）

## 2. 指纹登记（每取得一个产物填一行）

| 文件 | sha256(64) | 字节 | `file` 结果 | 取得时间 | 来源 URL |
| --- | --- | --- | --- | --- | --- |
| `iptv-proxy-linux-arm64` | | | | | |
| `iptv-proxy-linux-amd64` | | | | | |

> 计算：`shasum -a 256 <file>`；`file <file>`。

## 3. 基础结构（取得后填）

| 项 | arm64 | amd64 |
| --- | --- | --- |
| ELF 类型（PIE/EXEC） | | |
| 动态/静态链接（`ldd`） | | |
| 解释器（`readelf -l` interp） | | |
| 依赖库（`readelf -d` NEEDED） | | |
| 加固（NX/PIE/RELRO/canary，`readelf`/`checksec`） | | |
| 编译器/语言线索（Go buildinfo / Rust 符号） | | |
| 入口点 / `.text` 范围 | | |

## 4. 采集命令（可复现）

```bash
# 取得后执行并回填
shasum -a 256 iptv-proxy-linux-arm64
file iptv-proxy-linux-arm64
readelf -h iptv-proxy-linux-arm64
readelf -l iptv-proxy-linux-arm64 | grep -A1 INTERP
readelf -d iptv-proxy-linux-arm64
```

## 5. 备注

- 分析副本**只**使用 arm64 与 amd64 两个 Linux 产物（与范围一致）。
- 同一版本只登记一次；换版本另起小节（见规范 `01` §5）。
