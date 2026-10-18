# 04 · 产出与文档规范

> 规范版本 v1.0 ｜ 2026-10-15

## 1. 语言与格式

- **主语言**：简体中文。技术术语保留英文（如 segment、master playlist、proxy）。
- **格式**：Markdown（`.md`）。图谱/参考页可另出 HTML。
- 风格对齐既有文档 `HasonHuang/sontv/docs/iptv-proxy/`：
  表格优先、短句、结论先行、每条结论附验证方法。

## 2. 目录约定

本仓库（`rea-iptv`）内：

```

docs/
├── spec/          # 本规范（只读参考，改前先读 README）
└── findings/      # 逆向发现（分析产出）
    ├── README.md          # 发现索引
    ├── artifact.md        # 目标指纹与取得记录
    ├── static/            # 静态分析
    └── dynamic/           # 动态分析

```

> 基线文档位于**另一个仓库** `HasonHuang/sontv/docs/iptv-proxy/`；
> 本仓库产出**引用**它，不复制（避免双源不一致）。

## 3. 发现文档模板（强制）

每篇发现文档顶部必须包含：

```markdown
# <标题>

> 目标版本：vX.Y.Z ｜ 目标 SHA-256：<sha256> ｜ 架构：arm64
> 采集日期：YYYY-MM-DD ｜ 环境：<platform/arch/isolation>
> 结论级别：观测 / 推断（逐条标注）

```

正文要求：

1. **结论先行**：开头给出可判定的结论（并标注级别）。
2. **逐条附证据**：命令、步骤或 REA Evidence ID。
3. **局限与未知**：单列一节。
4. **术语表**：涉及领域术语时附中英对照表。
5. **相关文档**：链接基线或相邻发现。

## 4. 结论标注法

- 观测：`【观测】`；推断：`【推断】`；未知：`【未知】`。
- 差异（相对基线）：`【差异】`，并给出基线与新证据链接。

## 5. 命名约定

- 文件：小写、短横线分隔，按主题命名，例如
  `dynamic/segment-disguise.md`、`static/mitigations.md`。
- REA Evidence ID 原样引用 `ev_<64hex>`。
- Commit message：`<scope>: <简述>`，scope ∈
  `spec` / `target` / `static` / `dynamic` / `report` / `chore`。

## 6. 数字与引用规范

- 哈希写全 64 位；地址写 `0x` 十六进制；文件偏移注明"文件偏移"而非"地址"。
- 引用公开来源给 URL 与访问日期；引用内部发现给仓库相对路径。
- 时间统一 `YYYY-MM-DD`。

## 7. 禁止事项

- 禁止无证据的断言、把推断当事实、把否定当绝对。
- 禁止复制粘贴基线结论冒充本次实测；引用基线必须标"引用基线"。
- 禁止在文档中写入真实凭据（token、密码明文、代理账密）。
