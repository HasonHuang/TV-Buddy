# 目标指纹与取得记录（artifact）

> 遵循 [`../spec/01-target-and-scope.md`](../spec/01-target-and-scope.md) §1、§2。
> **在登记指纹前，任何"对目标的分析"都必须标注所分析的具体副本来源。**

## 1. 取得

- 来源仓库：`YanG-1989/rust`，子目录 `IPTV Proxy/`（**源码未公开**，仓库仅托管预编译产物）
- 取得方式：**raw 直取 main 分支产物**（该仓库**无 tag / 无 release**）
  - 模板：`https://raw.githubusercontent.com/YanG-1989/rust/main/IPTV%20Proxy/iptv-proxy-linux-{arch}`
- 取得时间：**2026-10-15**（本地，Asia/Shanghai）
- 对应上游提交：`b6331774f8`（committer date `2026-10-15T06:58:22Z`，"Add files via upload"）
- 本地路径：`samples/`（**已 gitignore**，仅登记指纹）

> ⚠️ **不可复现风险**：因无 tag/release，`main` 会**随时变动**（该二进制 2026-10-15 当天刚更新）。
> 本记录以**提交 SHA `b6331774f8`**作为可复现锚点。

## 2. 指纹登记

### 2.1 发布产物（加壳态，即上游分发原件）

| 文件 | sha256 | 字节 | `file` 结果 |
| --- | --- | --- | --- |
| `iptv-proxy-linux-arm64` | `a3d3e4d31dd5107b11b6f8bfe9ef00388966ed0b67570d190dc7fdaf998a3501` | 4042256 | ELF 64-bit LSB executable, ARM aarch64, statically linked, no section header |
| `iptv-proxy-linux-amd64` | `6acdaf73f792dbcfe05b14845b641f6d6a16996e1aba6142328b62dbd3e86264` | 5049432 | ELF 64-bit LSB executable, x86-64, statically linked, no section header |

### 2.2 脱壳产物（UPX `-d` 后，静态分析工作副本）

| 文件 | sha256 | 字节 | `file` 结果 |
| --- | --- | --- | --- |
| `iptv-proxy-arm64` | `84042e3f48329f9240012c1d2e8f88e6cf3dabacc4fe3cb660c155d9906fed59` | 19428384 | ELF 64-bit LSB executable, ARM aarch64, statically linked, stripped |
| `iptv-proxy-amd64` | `9a39a691436a2fdfe04765ca94b877520c2b9fed0ed404351256e521fafe8cd1` | 21358456 | ELF 64-bit LSB executable, x86-64, statically linked, stripped |

> 脱壳过程与依据见 [`static/01-sample-packing-and-toolchain.md`](static/01-sample-packing-and-toolchain.md)。

## 3. 基础结构（脱壳后，`readelf`）

| 项 | arm64 | amd64 |
| --- | --- | --- |
| ELF 类型 | `ET_EXEC`（**非 PIE**） | `ET_EXEC`（**非 PIE**） |
| 基址 | `0x1000000` | `0x1000000` |
| 入口点 | `0x18315c0` | `0x1822880` |
| 解释器（INTERP） | 无（静态） | 无（静态） |
| 动态段 / NEEDED | 无（`There is no dynamic section`） | 无（静态） |
| `GNU_STACK` | `RW`（**NX 开**） | `RW`（NX 开） |
| `GNU_RELRO` | 有 | 有 |
| `TLS` | 有（0xd0/0x230） | 有（0xd0/0x230） |
| 节数（`readelf -S`） | 19 | 18 |
| 符号表 | 已剥离（`stripped`） | 已剥离 |
| 语言 / 工具链 | **Rust**（rustc 1.99.0） | **Rust**（rustc 1.99.0） |
| 加壳 | **UPX 4.24**（`-d` 可解） | **UPX 4.24**（`-d` 可解） |

## 4. 采集命令（可复现）

```bash
ARCH=arm64
BASE="https://raw.githubusercontent.com/YanG-1989/rust/b6331774f8/IPTV%20Proxy"
curl -fsSL "$BASE/iptv-proxy-linux-$ARCH" -o "samples/iptv-proxy-linux-$ARCH"

# 指纹
shasum -a 256 "samples/iptv-proxy-linux-$ARCH"
file "samples/iptv-proxy-linux-$ARCH"

# 脱壳（容器内使用官方 upx，跨格式可解）
docker run --rm -v "$PWD/samples:/s" --platform linux/arm64 -w /s ubuntu:24.04 \
  bash -c 'apt-get update -qq && apt-get install -y -qq upx-ucl >/dev/null \
           && cp iptv-proxy-linux-arm64 unpacked/iptv-proxy-arm64 \
           && upx -d unpacked/iptv-proxy-arm64'
```

## 5. 备注

- 分析副本**只**使用 arm64 与 amd64 两个 Linux 产物（与范围一致）。
- **版本告警**：上游 CHANGELOG 最新为 **v0.8.8（2026-10-03）**，但本二进制内嵌面板含 **`[v0.8.9]`** 标注，
  且构建于 2026-10-15 → 当前 `main` 产物 **≈ v0.8.9**，**与基线 v0.8.2 不一致**。
  差异登记见 [`../spec/07-baseline-and-verification.md`](../spec/07-baseline-and-verification.md) §3（条目 `D-001`）。
- 历史产物可用 `https://raw.githubusercontent.com/YanG-1989/rust/<sha>/IPTV%20Proxy/iptv-proxy-linux-arm64` 取得；
  二进制提交历史：`b6331774f8`(10-15) / `86c682c526`(10-07) / `bbb5055769`(10-05) / `16af7f4c75`(10-05) /
  `4fa239b1fb`(10-04) / `ae6bf91bb1`(10-03, =v0.8.8) / `d302316fb8`(09-09)。
