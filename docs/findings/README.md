# 逆向发现（docs/findings）

本目录存放 **IPTV Proxy 逆向工程的分析产出**。所有内容遵循 [`../spec/`](../spec/) 的规范。

> 目标版本：**v0.8.2**（主，`d302316fb8`）｜ 对照 **v0.8.8**（= `main`）
> ｜ 目标指纹：见 [`artifact.md`](artifact.md)（**已登记**）

---

## 目录结构

```
docs/findings/
├── README.md      ← 本文件。发现索引
├── artifact.md    ← 目标指纹与取得记录（先填这个）
├── static/        ← 静态分析（段/符号/字符串/加固/反编译）
└── dynamic/       ← 动态分析（运行/行为/网络/缓存/伪装）
```

## 发现索引

| 编号 | 标题 | 域 | 结论级别 | 关联规格 ID | 日期 | 文件 |
| --- | --- | --- | --- | --- | --- | --- |
| F-001 | 样本加壳与工具链判定（UPX 4.24 / Rust 1.98.1，v0.8.2） | 静态 | 观测 | S-ENV-02, S-ENV-07, S-ENV-08, S-EP-09, S-CFG-04 | 2026-10-15 | [`static/01-sample-packing-and-toolchain.md`](static/01-sample-packing-and-toolchain.md) |
| F-002 | 版本判定：v0.8.2 = `d302316fb8`；main = v0.8.8 | 静态 | 观测 | S-ENV-01 | 2026-10-15 | [`static/01-sample-packing-and-toolchain.md`](static/01-sample-packing-and-toolchain.md) §5 |
| F-003 | 内嵌面板前端与 HTTP 接口表面 | 静态 | 观测 | S-MODE-01, S-DIS-01, S-DIS-11, S-EP-07/08, S-AUTH-01, S-CFG-04 | 2026-10-16 | [`static/02-embedded-panel-frontend.md`](static/02-embedded-panel-frontend.md) |
| F-004 | 运行时表面与切片伪装（后缀=sha256(上游切片URL)[:32]） | 动态 | 观测 | S-DIS-02/04/05, S-EP-05/07/10, S-CFG-01/02/04, S-ENV-05 | 2026-10-16 | [`dynamic/01-runtime-surface-and-disguise.md`](dynamic/01-runtime-surface-and-disguise.md) |
| F-005 | 六种工作模式与看门狗（redirect/passthrough=302，rewrite=绝对化，dash/remux 失败路径） | 动态 | 观测 | S-MODE-01/02/03/08/09 | 2026-10-17 | [`dynamic/02-modes-and-watchdog.md`](dynamic/02-modes-and-watchdog.md) |
| F-006 | 缓存布局与切片缓存窗口（首写 public→命中 private，max-age=300） | 动态 | 观测 | S-DIS-06/07, S-MODE-13/14 | 2026-10-17 | [`dynamic/03-cache-layout-and-window.md`](dynamic/03-cache-layout-and-window.md) |
| F-007 | 播放 Token 与安全护栏（入口需 `?token=`，切片不受保护） | 动态 | 观测 | S-AUTH-01/02/03, S-DIS-16, S-URL-06 | 2026-10-17 | [`dynamic/04-auth-token-and-guards.md`](dynamic/04-auth-token-and-guards.md) |
| F-008 | EPG 采集与下发（`/epg.xml[.gz]`、`/epg`；时间裁剪、`lang` 处理） | 动态 | 观测 | S-EP-05/06/12/13 | 2026-10-17 | [`dynamic/05-epg.md`](dynamic/05-epg.md) |

> 每完成一篇发现，在此登记一行；`关联规格 ID` 指向
> [`../spec/08-target-spec.md`](../spec/08-target-spec.md) 的 `S-*` 条目。

## 核验状态回填

核对矩阵见 [`../spec/07-baseline-and-verification.md`](../spec/07-baseline-and-verification.md)。
本目录的发现用于把 `07` 中 `⏳` 的条目更新为 ✅ / ⚠️。

## 写作要求（摘要）

- 顶部模板（版本/SHA-256/架构/日期/环境/结论级别）见规范 `04`。
- 每条结论标注【观测】/【推断】/【未知】，并附证据（命令或 `ev_...`）。
- **静态 ≠ 运行**；否定须有界。
- 目标派生内容一律按不可信数据处理（规范 `05`）。
