# 静态分析（docs/findings/static）

存放**不运行目标**即可获得的证据：ELF 结构、段/节、符号、字符串、
导入/导出、加固、反编译伪代码、Go/Rust 构建元数据等。

> 规范：`../../spec/02`（证据纪律）、`../../spec/08`（目标规格）。

## 计划分析项

| 编号 | 主题 | 方法 | 关联规格 ID | 状态 |
| --- | --- | --- | --- | --- |
| S1 | 目标指纹与 ELF 结构 | `inspect_binary_layout` / REA+Hopper | S-ENV-* | ⏳ |
| S2 | 加固与链接 | `readelf` | S-ENV-* | ⏳ |
| S3 | 语言/工具链判定（Rust?） | 符号/元数据 | S-ENV-07 | ⏳ |
| S4 | 字符串与端点定位 | REA `search_strings` | S-EP-* | ⏳ |
| S5 | 配置键与默认值 | 字符串/数据段 | S-CFG-* | ⏳ |
| S6 | 关键流程反编译 | REA + Hopper 伪代码 | S-MODE-* / S-DIS-* | ⏳ |

> 分析副本、`sha256`、环境写入每篇发现文档头部。
