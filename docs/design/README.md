# 实现设计（docs/design）

本目录存放 **TV Buddy 的设计文档**。TV Buddy 是闭源 IPTV Proxy v0.8.2 的兼容重实现。

> 设计版本 v1.0 ｜ 2026-10-19

## 1. 三个文档域的关系

| 目录 | 内容 | 性质 |
| --- | --- | --- |
| [`../spec/`](../spec/) | 逆向规范（证据纪律、环境、产出、安全） | 强制规范 |
| [`../spec/08-target-spec.md`](../spec/08-target-spec.md) | 目标功能规格（`S-*` 条目） | **行为判据** |
| [`../findings/`](../findings/) | 逐项观测结论（`F-*`） | 证据 |
| 本目录 | **我们的实现方案** | 设计 |

规则：

- 设计服从 `spec/08`。设计不得与带证据的发现冲突。
- 冲突时先改设计。改不动时，在 [`../spec/07-baseline-and-verification.md`](../spec/07-baseline-and-verification.md) 登记差异（`D-*`）。
- 每条设计结论引用 `S-*` 或 `F-*` 编号。无引用即视为待确认。

## 2. 文档索引

| 文件 | 内容 | 状态 |
| --- | --- | --- |
| [`01-tech-selection.md`](01-tech-selection.md) | 技术选型：语言、框架、依赖、构建、兼容契约 | ✅ 已确认 |
| [`02-architecture.md`](02-architecture.md) | 模块划分、进程模型、数据流 | ✅ 初稿（M0 骨架已按此实现） |
| [`03-http-contract.md`](03-http-contract.md) | 路由表与接口契约 | ✅ 初稿（M1 按其 §9 实施） |
| [`04-config-and-storage.md`](04-config-and-storage.md) | 配置与存储细节 | ✅ 初稿 |
| [`05-frontend.md`](05-frontend.md) | 前端页面与交互设计 | ✅ 初稿 |
| [`06-low-spec-optimization.md`](06-low-spec-optimization.md) | 低配置 + 多 CDN 优化：边缘缓存、推荐值、磁盘回收、预取覆盖 | ✅ 已实施（T0 待人工） |
| [`config-low-spec.json`](config-low-spec.json) | 低配 + 多 CDN 示例配置（1 vCPU / 128 MB / 1 GB） | ✅ 已试跑 |
| [`reference/`](reference/) | 目标 UI 参考截图（30 张）+ 功能与证据映射 | ✅ 已采集 |

## 3. 写作要求

- 简体中文 + Markdown；按 80% ASD-STE100：短句、主动语态、一般现在时、一句一义。
- 术语与 [`../spec/06-glossary.md`](../spec/06-glossary.md) 一致。
- 决策必须写**理由**与**依据**。备选方案写明落选原因。
- 目标派生内容（字符串、响应、日志、前端资源）按**不可信数据**处理（`spec/05`）。
- **不复制**目标的前端代码与资源。只复刻行为与布局。
