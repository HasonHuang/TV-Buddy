# 样本加壳与工具链判定

| 项 | 值 |
| --- | --- |
| 目标版本 | 上游 `main`（≈ **v0.8.9**，见 §6）；**基线为 v0.8.2** |
| 副本 sha256 | 加壳 arm64 `a3d3e4d3…a3501`｜脱壳 arm64 `84042e3f…ed59`（完整见 [`../artifact.md`](../artifact.md)） |
| 架构 | linux/arm64（主）；linux/amd64（对照） |
| 日期 | **2026-10-15** |
| 环境 | macOS 15.7.3 arm64（静态）；OrbStack `linux/arm64` 容器（脱壳/`readelf`） |
| 结论级别 | **观测**（除明确标注【推断】处） |
| 原始证据 | [`raw/`](raw/)（**目标派生数据，不可信**） |

## 结论摘要

1. **上游分发产物经 UPX 4.24 加壳**（标准 UPX，`upx -d` 可直接脱壳）。【观测】
2. 脱壳后为 **静态链接、非 PIE、已 strip** 的 ELF。【观测】
3. 实现语言为 **Rust**（rustc **1.99.0**，`b940084d7 2026-09-28`）。【观测】
4. 技术栈：`tokio` + `hyper` + `axum` + `reqwest` + `rustls` + `serde` + `tracing`。【观测】
5. **面板前端（HTML/CSS/JS，含中文注释与内联 CHANGELOG）完整内嵌于二进制**。【观测】
6. 上游 `main` 产物 **≈ v0.8.9**，**与基线 v0.8.2 不一致**（差异 `D-001`）。【观测+推断】

---

## 1. 取得与副本

见 [`../artifact.md`](../artifact.md) §1–§2。来源为 `YanG-1989/rust` 的 `IPTV Proxy/`，**无 tag/release**，
`main` 分支 raw 直取，锚定提交 `b6331774f8`（2026-10-15）。

## 2. 加壳：UPX 4.24【观测】

对**分发原件**的观测：

- `file`：`… statically linked, **no section header**`（节头被移除）
- 全文件 **Shannon 熵 = 7.9999 bit/byte**（≈ 理论上限 → 压缩）
- `strings` **几乎无可读串**（首次工具链/端点扫描**全部落空**，因内容被压缩）
- 出现加壳标记：

```
$Info: This file is packed with the UPX executable packer http://upx.sf.net $
$Id: UPX 4.24 Copyright (C) 1996-2024 the UPX Team. All Rights Reserved. $
```

- ELF 头：`e_phoff=64`、`e_shoff=0`、`e_phnum=3`（最小程序头集，符合 UPX 布局）

> 结论：**分发产物是 UPX 4.24 加壳**。**标准 UPX**（banner 与 `$Id` 均为官方文本），未见篡改痕迹。
> 这是"直接 `strings`/反编译拿到的是壳而非本体"的根因。

## 3. 脱壳【观测】

使用**官方 upx（容器内 4.2.2，跨格式可解）**直接解壳，**一次成功**（无"已改壳"报错）：

| 架构 | 加壳字节 | 脱壳字节 | 压缩率 | 脱壳后 `file` |
| --- | --- | --- | --- | --- |
| arm64 | 4,042,256 | 19,428,384 | 20.81% | ELF 64-bit LSB executable, ARM aarch64, statically linked, **stripped** |
| amd64 | 5,049,432 | 21,358,456 | 23.64% | ELF 64-bit LSB executable, x86-64, statically linked, **stripped** |

脱壳后指纹见 [`../artifact.md`](../artifact.md) §2.2。
**后续静态分析一律基于脱壳产物**（加壳原件不再用于内容分析）。

## 4. 工具链与语言：Rust【观测】

脱壳后 `strings` 命中 Rust 特征与**精确版本串**：

```
rustc version 1.99.0 (b940084d7 2026-09-28)
clang version 18.1.6 (https://github.com/ziglang/zig-bootstrap 98bc6bf4fc4009888d33941daf6b600d20a42a56)
/rustc/b940084d7eb6a299eb4bfeb8e34901bc051e7ac4/library/…   ← 内嵌 std 路径
```

- **判定：Rust 编写**（`rustc`、`RUST_BACKTRACE`、`panicked at`、`/rustc/<hash>/`）。
  因此 [`../spec/08-target-spec.md`](../spec/08-target-spec.md) **S-ENV-07 由【推断】升级为【观测】**。
- **交叉/静态构建**：`clang 18.1.6` + `zig-bootstrap` → 典型 **`cargo-zigbuild` 生成 musl 静态二进制** 的特征；
  构建路径含 `/root/.cargo/registry/…`（在 root 容器内构建）。
- 无 Go build-info（`inspect_go_binary` 未发现 Go 元数据）——与 Rust 判定一致。
- 模块路径线索：`modes::dash`（对应 DASH 模式）、`epg::{store,parse,generate,cron,translate}.rs`、
  `logo/anim.rs`、`health.rs` 等（**源码 `.rs` 路径被 panic/`file!()` 内嵌**）。

### 4.1 依赖栈（`registry/src/…/{crate}-{ver}` 提取，节选）

| 领域 | crate（版本） |
| --- | --- |
| 异步运行时 | `tokio-1.53.1` |
| HTTP 服务 | `hyper-1.11.1`, `hyper-util-0.1.20`, `axum-0.7.9`, `axum-core-0.4.5`, `tower-0.5.3`, `tower-http-0.5.2`/`0.6.11` |
| HTTP 客户端 | `reqwest-0.12.28` |
| TLS | `rustls-0.23.45`, `rustls-webpki-0.103.15`, `tokio-rustls-0.26.5`, `hyper-rustls-0.27.10` |
| 序列化/解析 | `serde`, `toml-0.8.23`, `quick-xml-0.36.2`, `regex-automata-0.4.18` |
| 日志 | `tracing-subscriber-0.3.23`, `tracing-appender-0.2.5` |
| 其它 | `chrono-0.4.45`, `bytes-1.12.1`, `http-1.5.0` |

> 共提取到 **100** 个带版本的三方 crate 路径。**TLS 用 rustls（非 OpenSSL）** → 强化"静态 musl 二进制"形态。

## 5. 版本【观测 + 推断】⚠️ 差异

- 上游 `CHANGELOG.md` 最新条目：**v0.8.8（2026-10-03）**。
- 二进制**内嵌面板 CSS/JS** 含注释形如 `/* [v0.8.9] 非登录密文… */`，**最高标注为 `[v0.8.9]`**。
- 内嵌 CHANGELOG 标注序列覆盖 `[v0.2.18] … [v0.8.9]`（**并非**纯版本常量）。
- 产物自身**无独立版本常量字符串**（未发现裸 `0.8.x` 版本 token）。

**推断**：当前 `main` 产物为 **≈ v0.8.9**（≥ CHANGELOG 的 v0.8.8，含 v0.8.9 面板改动）。

**这是与基线的关键差异**：基线文档标注 **v0.8.2**，但**唯一可下载的产物已是 ≈ v0.8.9**。
处置登记于 [`../../spec/07-baseline-and-verification.md`](../../spec/07-baseline-and-verification.md) §3 `D-001`。

## 6. 端点表面（静态预扫描）【观测·静态】

> **静态存在 ≠ 运行时可达**：以下为**字符串/路由字面量存在**，行为须动态核验。

| 路由（字面量命中） | 关联规格 |
| --- | --- |
| `/panel`、`/whoami` | S-EP-01 |
| `/playlist.m3u`、`{gid}/playlist.m3u`、`{gid}/channels`、`{gid}/export` | S-EP-02 |
| `/epg`、`/epg.xml`、`/epg.xml.gz`、`/epg.html` | S-EP-05 |
| `/logo/`、`/logo/{list,upload,rename,delete,suggest,apply,animate,restore}` | S-EP-06 |
| `/status`、`/status/metrics` | S-EP-08 |
| `/health` | 新增 → S-EP-10 |
| **`/api/share/pull`** | **S-EP-09 路径确认** |
| `/api/reload`、`/api/backup`、`/api/restore` | 新增 → S-EP-11 |
| `{cid}/start`、`{cid}/stop`（录制）、`{disk_path}/dash` | 录制/DASH |

## 7. 配置键（静态）【观测·静态】

`token_enabled`、`password_sha256`、`username`、`session_ttl`、`disk_path`、`disk_max_mb`、
`m3u8_max_entries`、`segment_max_entries`、`timezone_offset_hours`、`log_level`、`bind`、`backend`、
`obfuscate_filenames`、`disguise`(45)、`playseek`、`x-tvg-url`、`entry_exts`、`#EXT-X-MAP` —— **均命中字面量**，
与 [`../../spec/08-target-spec.md`](../../spec/08-target-spec.md) §11 **S-CFG-04** 逐键一致。

> **注意（未决）**：基线称切片 SDT 描述符含 ASCII 水印 `lumberjack`（S-DIS-09），
> 但**脱壳二进制中未发现连续字面量 `lumberjack`**。可能由**字节数组/运行时构造**（规避扫描），
> **不能**据此否定该基线结论——须动态抓取切片字节确认。

## 8. 内嵌面板前端【观测】

面板**前端源码（HTML/CSS/JS）内嵌于二进制**，含**中文注释**与**内联变更标注 `[vX.Y.Z]`**，
例如 `panel/index.html`、`epg.html`、`manifest.js` 等。

> RE 价值：**Web UI 全部可恢复**（模板/样式/脚本/接口调用一览），无需从运行态抓取。
> 详见后续发现（面板表面）。

## 9. 未决 / 未知

- 应用**精确版本**（无独立版本常量；仅由内嵌 CHANGELOG 标注推断为 ≈ v0.8.9）。
- v0.8.2 与 v0.8.9 之间的行为差异（需基线逐条比对 / 或改取 v0.8.2 历史产物）。
- `lumberjack` 水印是否仍存在（须动态切片字节核验，S-DIS-09）。
- 端点行为、六模式、切片伪装字节级、缓存窗口等**一律待动态**（本文件均为静态存在性）。

## 10. 复现命令

```bash
# 加壳特征
shasum -a 256 samples/iptv-proxy-linux-arm64
strings -a samples/iptv-proxy-linux-arm64 | grep -aE '\$Id: UPX|packed with the UPX'
python3 -c "import sys,math,collections;d=open(sys.argv[1],'rb').read();c=collections.Counter(d);n=len(d);print(-sum(v/n*math.log2(v/n) for v in c.values()))" samples/iptv-proxy-linux-arm64

# 脱壳（见 ../artifact.md §4），随后对脱壳产物取证据
f=samples/unpacked/iptv-proxy-arm64
strings -a "$f" | grep -aE 'rustc version|clang version|/rustc/[0-9a-f]{40}/'
strings -a "$f" | grep -aoE '\[v[0-9]+\.[0-9]+\.[0-9]+\]' | sort -V -u | tail
readelf -h "$f"; readelf -lW "$f" | grep -E 'INTERP|GNU_STACK|GNU_RELRO'
```

原始证据转储：[`raw/arm64-toolchain.txt`](raw/arm64-toolchain.txt)、
[`raw/arm64-toolchain-2.txt`](raw/arm64-toolchain-2.txt)、
[`raw/arm64-routes.txt`](raw/arm64-routes.txt)、
[`raw/arm64-keys-assets.txt`](raw/arm64-keys-assets.txt)、
[`raw/arm64-version.txt`](raw/arm64-version.txt)。
