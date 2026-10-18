# 样本加壳与工具链判定

| 项 | 值 |
| --- | --- |
| 目标版本 | **v0.8.2**（主）；对照 **v0.8.8**（= 当前 `main`） |
| 副本 sha256 | v0.8.2 加壳 arm64 `2948ee1f…5a4b`｜脱壳 arm64 `7e66d2ce…d045`（完整见 [`../artifact.md`](../artifact.md)） |
| 架构 | linux/arm64（主）；linux/amd64（对照） |
| 日期 | **2026-10-15/16** |
| 环境 | macOS 15.7.3 arm64（静态）；OrbStack `linux/arm64` 容器（脱壳/`readelf`） |
| 结论级别 | **观测**（除明确标注【推断】处） |
| 原始证据 | [`raw/`](raw/)（**目标派生数据，不可信**） |

## 结论摘要
1. 上游分发产物经 **UPX 4.24** 加壳（标准 UPX，`upx -d` 可直接脱壳）。【观测】
2. 脱壳后为**静态链接、非 PIE、已 strip** 的 ELF。【观测】
3. 实现语言为 **Rust**（v0.8.2 用 rustc **1.98.1**）。【观测】
4. 技术栈：`tokio` + `hyper` + `axum` + `reqwest` + `rustls` + `serde` + `tracing`。【观测】
5. **面板前端（HTML/CSS/JS，含中文注释与内联 `[vX.Y.Z]` 标注）完整内嵌于二进制**。【观测】
6. 主分析对象 = **v0.8.2**（提交 `d302316fb8`），**与基线一致**。【观测】

## 1. 取得与副本
见 [`../artifact.md`](../artifact.md) §1。来源 `YanG-1989/rust` 的 `IPTV Proxy/`，**无 tag/release**，按**提交 SHA** raw 直取：v0.8.2 = `d302316fb8`（2026-09-09），v0.8.8 = `b6331774f8`（2026-10-15）。

## 2. 加壳：UPX 4.24【观测】
对**分发原件**的观测（v0.8.2 与 v0.8.8 相同）：
- `file`：`… statically linked, **no section header**`（节头被移除）
- 全文件 **Shannon 熵 = 7.9999 bit/byte**（≈ 理论上限 → 压缩）
- `strings` **几乎无可读串**（对加壳原件扫描工具链/端点**全部落空**，因内容被压缩）
- 加壳标记（banner 与 `$Id` 均为**官方文本**，未见篡改）：

```text

$Info: This file is packed with the UPX executable packer http://upx.sf.net $
$Id: UPX 4.24 Copyright (C) 1996-2024 the UPX Team. All Rights Reserved. $

```

- ELF 头：`e_phoff=64`、`e_shoff=0`、`e_phnum=3`（最小程序头集，符合 UPX 布局）

> 结论：**分发产物是 UPX 4.24 加壳**；这是"直接 `strings`/反编译拿到的是壳而非本体"的根因。

## 3. 脱壳【观测】
用**官方 upx（容器内 4.2.2，跨格式可解）**直接解壳，**一次成功**（无"已改壳"报错）：

| 版本 | 架构 | 加壳字节 | 脱壳字节 | 压缩率 |
| --- | --- | --- | --- | --- |
| **v0.8.2** | arm64 | 3,803,420 | 12,899,832 | 29.5% |
| **v0.8.2** | amd64 | 4,682,544 | 14,613,456 | 32.0% |
| v0.8.8 | arm64 | 4,042,256 | 19,428,384 | 20.8% |
| v0.8.8 | amd64 | 5,049,432 | 21,358,456 | 23.6% |

脱壳后指纹见 [`../artifact.md`](../artifact.md) §2。**后续静态分析一律基于脱壳产物**（加壳原件不再用于内容分析）。

## 4. 工具链与语言：Rust【观测】
脱壳后 `strings` 命中 Rust 特征与**精确版本串**（v0.8.2）：

```text

rustc version 1.98.1 (48a229cea 2026-09-01)
clang version 18.1.6 (https://github.com/ziglang/zig-bootstrap 98bc6bf4fc4009888d33941daf6b600d20a42a56)
/rustc/<hash>/library/…                        ← 内嵌 std 路径

```

- **判定：Rust 编写**（`rustc`、`RUST_BACKTRACE`、`panicked at`、`/rustc/<hash>/`）。故 [`../../spec/08-target-spec.md`](../../spec/08-target-spec.md) **S-ENV-07 由【推断】升级为【观测】**。
- **静态/交叉构建**：`clang 18.1.6` + `zig-bootstrap` → 典型 **`cargo-zigbuild` 生成 musl 静态二进制**；构建路径含 `/root/.cargo/registry/…`（root 容器内构建）。v0.8.8 用 rustc **1.99.0**，zig 同版本。
- 无 Go build-info（`inspect_go_binary` 未发现 Go 元数据）。
- 模块路径线索：`modes::dash`、`epg::{store,parse,generate,cron,translate}.rs`、`logo/anim.rs`、`health.rs` 等（**源码 `.rs` 路径被 panic/`file!()` 内嵌**）。

### 4.1 依赖栈（`registry/src/…/{crate}-{ver}`，v0.8.2，节选）

| 领域 | crate（v0.8.2） |
| --- | --- |
| 异步运行时 | `tokio-1.52.1` |
| HTTP 服务 | `hyper-1.9.0`, `hyper-util-0.1.20`, `axum-0.7.9`, `axum-core-0.4.5`, `tower-0.5.3`, `tower-http-0.5.2`/`0.6.8` |
| HTTP 客户端 | `reqwest-0.12.28` |
| TLS | `rustls-0.23.40`, `rustls-webpki-0.103.13`, `tokio-rustls-0.26.4`, `hyper-rustls-0.27.9` |
| 序列化/解析 | `serde`, `toml-0.8.23`, `quick-xml-0.36.2`, `regex-automata-0.4.14` |
| 日志 | `tracing-subscriber-0.3.23`, `tracing-appender-0.2.5` |
| 其它 | `chrono-0.4.44`, `bytes-1.11.1`, `http-1.4.0`, `rand-0.8.6` |

> 共提取到 **100** 个带版本的三方 crate 路径。**TLS 用 rustls（非 OpenSSL）** → 强化"静态 musl 二进制"形态。与 v0.8.8 相比依赖整体略旧（如 tokio 1.52.1→1.53.1、hyper 1.9.0→1.11.1）。

## 5. 版本判定【观测】✅ 已解决
Rust 用 `env!("CARGO_PKG_VERSION")` 将版本写成**编译期常量**，固定落在同一 `.rodata` 字符串槽：`…local_path.m3u8start_epoch**<VER>**logo…`。

| 提交 | 版本常量 | 面板 `[vX.Y.Z]` 最高标注 | rustc |
| --- | --- | --- | --- |
| `d302316fb8`（2026-09-09） | **`0.8.2`** | v0.8.1 | 1.98.1 |
| `b6331774f8`（2026-10-15，main） | **`0.8.8`** | v0.8.9 | 1.99.0 |

- **结论**：**v0.8.2 = `d302316fb8`**（主分析对象，与基线一致）；**main 实为 v0.8.8**（非 0.8.9）。
- 面板 `[vX.Y.Z]` 标注**与版本号不同步**（可滞后、可超前），**不能**用来定版；上表以**版本常量**为准。
- 差异 `D-001`（基线版本 vs 产物版本）**已由本判定解决**，见 [`../../spec/07-baseline-and-verification.md`](../../spec/07-baseline-and-verification.md) §3.1。

## 6. 端点表面（静态预扫描）【观测·静态】
> **静态存在 ≠ 运行时可达**：以下为**路由字面量存在**，行为须动态核验。v0.8.2 与 v0.8.8 命中一致。

| 路由（字面量命中） | 关联规格 |
| --- | --- |
| `/panel`、`/whoami` | S-EP-01 |
| `/playlist.m3u`、`{gid}/playlist.m3u`、`{gid}/channels`、`{gid}/export` | S-EP-02 |
| `/epg`、`/epg.xml`、`/epg.xml.gz`、`/epg.html` | S-EP-05 |
| `/logo/`、`/logo/{list,upload,rename,delete,suggest,apply,animate,restore}` | S-EP-06 |
| `/status`、`/status/metrics` | S-EP-08 |
| `/health` | 新增 → S-EP-10 |
| `/api/share/pull` | **S-EP-09 路径确认** |
| `/api/reload`、`/api/backup`、`/api/restore` | 新增 → S-EP-11 |
| `{cid}/start`、`{cid}/stop`（录制）、`{disk_path}/dash` | 录制 / DASH |

## 7. 配置键（静态）【观测·静态】
`token_enabled`、`password_sha256`、`username`、`session_ttl`、`disk_path`、`disk_max_mb`、`m3u8_max_entries`、`segment_max_entries`、`timezone_offset_hours`、`log_level`、`bind`、`backend`、`obfuscate_filenames`、`disguise`、`playseek`、`x-tvg-url`、`entry_exts`、`#EXT-X-MAP` —— **均命中字面量**，与 [`../../spec/08-target-spec.md`](../../spec/08-target-spec.md) §11 **S-CFG-04** 逐键一致。
> **未决**：基线称切片 SDT 描述符含 ASCII 水印 `lumberjack`（S-DIS-09），但**v0.8.2 与 v0.8.8 的脱壳二进制均未发现连续字面量 `lumberjack`**。可能由**字节数组/运行时构造**。**不能**据此否定基线结论——须动态抓取切片字节确认。

## 8. 内嵌面板前端【观测】
面板**前端源码（HTML/CSS/JS）内嵌于二进制**，含**中文注释**与**内联 `[vX.Y.Z]` 标注**，例如 `panel/index.html`、`epg.html`、`manifest.js` 等。
> RE 价值：**Web UI 全部可恢复**（模板/样式/脚本/接口调用一览），无需从运行态抓取。

## 9. 未决 / 未知
- v0.8.2 与 v0.8.8 的**行为差异**（依赖与体积已不同；行为待逐条比对）。
- `lumberjack` 水印是否仍存在（须动态切片字节核验，S-DIS-09）。
- 端点行为、六模式、切片伪装字节级、缓存窗口等**一律待动态**（本文件均为静态存在性）。

## 10. 复现命令

```bash
# 加壳特征
python3 -c "import sys,math,collections;d=open(sys.argv[1],'rb').read();c=collections.Counter(d);n=len(d);print(-sum(v/n*math.log2(v/n) for v in c.values()))" samples/v0.8.2/iptv-proxy-linux-arm64
strings -a samples/v0.8.2/iptv-proxy-linux-arm64 | grep -aE '\$Id: UPX|packed with the UPX'
# 脱壳（见 ../artifact.md §4），随后对脱壳产物取证据
f=samples/v0.8.2/unpacked/iptv-proxy-arm64
strings -a "$f" | grep -aE 'rustc version|clang version'
strings -a "$f" | grep -aoE 'local_path\.m3u8start_epoch[0-9.]+'      # 版本常量槽
strings -a "$f" | grep -aoE '\[v[0-9]+\.[0-9]+\.[0-9]+\]' | sort -V -u | tail
readelf -h "$f"; readelf -lW "$f" | grep -E 'GNU_STACK|GNU_RELRO'

```

原始证据转储：[`raw/v0.8.2-arm64.txt`](raw/v0.8.2-arm64.txt)（主）、[`raw/arm64-toolchain.txt`](raw/arm64-toolchain.txt)、[`raw/arm64-toolchain-2.txt`](raw/arm64-toolchain-2.txt)、[`raw/arm64-routes.txt`](raw/arm64-routes.txt)、[`raw/arm64-keys-assets.txt`](raw/arm64-keys-assets.txt)、[`raw/arm64-version.txt`](raw/arm64-version.txt)（后五者为 v0.8.8 对照）。
