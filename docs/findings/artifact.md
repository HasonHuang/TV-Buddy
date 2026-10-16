# 目标指纹与取得记录（artifact）

> 遵循 [`../spec/01-target-and-scope.md`](../spec/01-target-and-scope.md) §1、§2。
> **在登记指纹前，任何"对目标的分析"都必须标注所分析的具体副本来源。**

## 1. 取得

- 来源仓库：`YanG-1989/rust`，子目录 `IPTV Proxy/`。**源码未公开**，仓库只托管预编译产物。
- 取得方式：**raw 直取指定提交的产物**。该仓库**无 tag / 无 release**。
  - 模板：`https://raw.githubusercontent.com/YanG-1989/rust/<sha>/IPTV%20Proxy/iptv-proxy-linux-{arch}`
- 取得时间：**2026-10-15/16**（本地，Asia/Shanghai）

| 角色 | 版本 | 上游提交 | 提交时间 | 说明 |
| --- | --- | --- | --- | --- |
| **主分析对象** | **v0.8.2** | `d302316fb8` | 2026-09-09 | **与基线一致**（基线 v0.8.2，实测 2026-10-02/03） |
| 对照（次） | v0.8.8 | `b6331774f8` | 2026-10-15 | 当前 `main`，用于版本差异比对 |

> **为何选 `d302316fb8` 作 v0.8.2**：基线文档（2026-10-02/03）面板显示版本 `0.8.2`。
> 上游该时段唯一的产物提交是 `d302316fb8`（2026-09-09）；其后 `ae6bf91bb1`（`2026-10-03T18:05Z` = **CST 10-04 02:05**，即 v0.8.8）。
> 该产物内嵌版本常量确为 **`0.8.2`**（见 §5）。证据见
> [`static/01-sample-packing-and-toolchain.md`](static/01-sample-packing-and-toolchain.md) §5。

- 本地布局（`samples/` **已 gitignore**，只登记指纹）：
  - `samples/v0.8.2/`（主）：`iptv-proxy-linux-{arm64,amd64}` + `unpacked/`
  - `samples/v0.8.8/`（次）：同上

## 2. 指纹登记

### 2.1 v0.8.2（主分析对象，`d302316fb8`）

**发布产物（加壳态，上游分发原件）**

| 文件 | sha256 | 字节 | `file` 结果 |
| --- | --- | --- | --- |
| `iptv-proxy-linux-arm64` | `2948ee1f0eff22a48a351653f2df9f29dc424708bacd670f34378476efab5a4b` | 3803420 | ELF 64-bit LSB executable, ARM aarch64, statically linked, no section header |
| `iptv-proxy-linux-amd64` | `ff34976227157319a251770e14b2f907e6237f0001f9d8c5ea79a517a43c01a7` | 4682544 | ELF 64-bit LSB executable, x86-64, statically linked, no section header |

**脱壳产物（UPX `-d` 后，静态分析工作副本）**

| 文件 | sha256 | 字节 | `file` 结果 |
| --- | --- | --- | --- |
| `iptv-proxy-arm64` | `7e66d2cec012efe9c9b732936d43bdfa6eaa4af61288ac53656e8e0622e3d045` | 12899832 | ELF 64-bit LSB executable, ARM aarch64, statically linked, stripped |
| `iptv-proxy-amd64` | `1975036f3e1a6dc031658c9f65ebf0fab691befbd0d7bc9b534ca398b3fbdf42` | 14613456 | ELF 64-bit LSB executable, x86-64, statically linked, stripped |

### 2.2 v0.8.8（对照，`b6331774f8`）

| 文件 | sha256 | 字节 | `file` 结果 |
| --- | --- | --- | --- |
| `iptv-proxy-linux-arm64` | `a3d3e4d31dd5107b11b6f8bfe9ef00388966ed0b67570d190dc7fdaf998a3501` | 4042256 | ELF 64-bit LSB executable, ARM aarch64, statically linked, no section header |
| `iptv-proxy-linux-amd64` | `6acdaf73f792dbcfe05b14845b641f6d6a16996e1aba6142328b62dbd3e86264` | 5049432 | ELF 64-bit LSB executable, x86-64, statically linked, no section header |
| `iptv-proxy-arm64`（脱壳） | `84042e3f48329f9240012c1d2e8f88e6cf3dabacc4fe3cb660c155d9906fed59` | 19428384 | ELF 64-bit LSB executable, ARM aarch64, statically linked, stripped |
| `iptv-proxy-amd64`（脱壳） | `9a39a691436a2fdfe04765ca94b877520c2b9fed0ed404351256e521fafe8cd1` | 21358456 | ELF 64-bit LSB executable, x86-64, statically linked, stripped |

## 3. 基础结构（**v0.8.2** 脱壳后，`readelf`）

| 项 | arm64 | amd64 |
| --- | --- | --- |
| ELF 类型 | `ET_EXEC`（**非 PIE**） | `ET_EXEC`（**非 PIE**） |
| 入口点 | `0x1322d80` | `0x1317940` |
| 解释器（INTERP） | 无（静态） | 无（静态） |
| 动态段 / NEEDED | 无（`There is no dynamic section`） | 无（静态） |
| `GNU_STACK` | `RW`（**NX 开**） | `RW`（NX 开） |
| `GNU_RELRO` | 有 | 有 |
| 节数（`readelf -S`） | 19 | 18 |
| 符号表 | 已剥离（`stripped`） | 已剥离 |
| 语言 / 工具链 | **Rust**（rustc **1.98.1**） | **Rust**（rustc 1.98.1） |
| 加壳 | **UPX 4.24**（`-d` 可解） | **UPX 4.24**（`-d` 可解） |

## 4. 采集命令（可复现）

```bash
SHA=d302316fb8            # v0.8.2
for a in arm64 amd64; do
  curl -fsSL "https://raw.githubusercontent.com/YanG-1989/rust/$SHA/IPTV%20Proxy/iptv-proxy-linux-$a" \
    -o "samples/v0.8.2/iptv-proxy-linux-$a"
done
shasum -a 256 samples/v0.8.2/iptv-proxy-linux-arm64
file samples/v0.8.2/iptv-proxy-linux-arm64

# 脱壳（容器内官方 upx，跨格式可解）
docker run --rm -v "$PWD/samples:/s" --platform linux/arm64 -w /s ubuntu:24.04 bash -c '
  apt-get update -qq && apt-get install -y -qq upx-ucl binutils file >/dev/null
  mkdir -p v0.8.2/unpacked
  cp v0.8.2/iptv-proxy-linux-arm64 v0.8.2/unpacked/iptv-proxy-arm64
  upx -d v0.8.2/unpacked/iptv-proxy-arm64'
```

## 5. 版本判定（为何是 v0.8.2）

- Rust 用 `env!("CARGO_PKG_VERSION")` 把版本写成**编译期常量字符串**。
- 该常量固定落在 `.rodata` 的同一"字符串槽"：`…local_path.m3u8start_epoch**<VER>**logo…`。
- 实测该槽取值：`d302316fb8` → **`0.8.2`**；`b6331774f8`（main）→ **`0.8.8`**。
- 旁证：`d302316fb8` 内嵌面板 `[vX.Y.Z]` 标注最高为 **v0.8.1**（面板改动标注会**滞后**于版本号）；main 最高为 `[v0.8.9]` 而版本常量为 `0.8.8`，说明标注可**超前**。

> 结论：**v0.8.2 = `d302316fb8`**（主分析对象）。main 实为 **v0.8.8**（非 0.8.9）。

## 6. 备注

- 分析副本**只**用 Linux 产物（arm64 主、amd64 对照），与范围一致。
- 上游**无 tag/release**，`main` 随构建变动；**必须以提交 SHA 作锚点**才能复现。
- 二进制提交历史（arm64 产物）：
  `d302316fb8`(2026-09-09, **v0.8.2**) / `ae6bf91bb1`(2026-10-03, v0.8.8) / `4fa239b1fb`(10-04) /
  `16af7f4c75`(10-05) / `bbb5055769`(10-05) / `86c682c526`(10-07) / `b6331774f8`(10-15, v0.8.8)。
