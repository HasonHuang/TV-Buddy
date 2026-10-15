# 静态分析（docs/findings/static）

存放**不运行目标**即可获得的证据：ELF 结构、段/节、符号、字符串、
导入/导出、加固、反编译伪代码、Go/Rust 构建元数据等。

> 规范：`../../spec/02`（证据纪律）、`../../spec/08`（目标规格）。

## 计划分析项

| 编号 | 主题 | 方法 | 关联规格 ID | 状态 |
| --- | --- | --- | --- | --- |
| S1 | 目标指纹与 ELF 结构 | `file` + 容器内 `readelf` | S-ENV-* | ✅ [`01`](01-sample-packing-and-toolchain.md) |
| S2 | 加壳与脱壳 | UPX 特征 + `upx -d` | S-ENV-08 | ✅ [`01`](01-sample-packing-and-toolchain.md) |
| S3 | 语言/工具链判定（Rust） | `strings` 符号/元数据 | S-ENV-07 | ✅ [`01`](01-sample-packing-and-toolchain.md) |
| S4 | 字符串与端点定位 | `strings` 字面量（静态） | S-EP-* | 🔶 部分（存在性） |
| S5 | 配置键与默认值 | 字符串/数据段 | S-CFG-* | 🔶 键名✅/默认值⏳ |
| S6 | 关键流程反编译 | REA + Hopper 伪代码 | S-MODE-* / S-DIS-* | ⏳ |
| S7 | 内嵌面板前端恢复 | 提取 HTML/CSS/JS | S-EP-* | ⏳ |

> REA `inspect_binary_layout` 本机**不可用**（provider `pwntools-elf` 仅支持 Linux x64 且未安装），
> 故 ELF 结构改用容器内 `readelf`。`inspect_go_binary` 已用（未发现 Go 元数据）。
> Hopper 反编译（S6）待启动。

> 分析副本、`sha256`、环境写入每篇发现文档头部。
