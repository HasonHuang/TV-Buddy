# 07 · 基线与核对矩阵

> 规范版本 v1.0 ｜ 2026-10-15 ｜ 基线版本：**iptv-proxy v0.8.2**

## 1. 基线来源

基线是既有实测文档（写于 **2026-10-02 / 2026-10-03**）：

```
HasonHuang/sontv/docs/iptv-proxy/
├── markdown/README.md            # 导航 · 六模式速查 · 安全清单
├── markdown/iptv.md              # 订阅接入入门
├── markdown/fields-common.md     # 通用字段详解
├── markdown/mode-proxy.md        # 反向代理
├── markdown/mode-remux.md        # Remux
├── markdown/mode-dash.md         # DASH
├── markdown/mode-redirect.md     # 302 跳转
├── markdown/mode-rewrite.md      # URL 改写
├── markdown/mode-passthrough.md  # 直链
├── markdown/resource.md          # 资源占用实测
├── markdown/panel.md             # 面板 13 页 · 配置 · 运维
├── markdown/disguise-analysis.md # 切片伪装字节级验证
├── iptv-proxy-handbook.html      # 可视化上手页
└── iptv-proxy-reference.html     # 参考页
```

**基线是"引用"关系**：本仓库产出**引用**基线，不复制其内容。

> **规范正文**：基线的规范化整理见 [`08-target-spec.md`](08-target-spec.md)，
> 其条目以 `S-*` 规格 ID 标识。本文件的核对矩阵应引用这些 ID（第 2.1 节为节选）。

## 2. 核对矩阵（收尾必填）

对每条基线结论，标注本次核验结果：

| 状态 | 含义 |
| --- | --- |
| ✅ 一致 | 本次证据与基线一致 |
| ⚠️ 差异 | 与基线不一致（必须附新证据与差异说明） |
| ⏳ 未核 | 本次未覆盖（写明原因） |

### 2.1 待核项（节选，随进度扩充）

| # | 基线结论 | 级别 | 状态 | 证据 |
| --- | --- | --- | --- | --- |
| 1 | 面板提供**六种**工作模式（README 只写四种） | 观测 | ⏳ | |
| 2 | 切片伪装**不改载荷**，仅改后缀 + Content-Type | 观测 | ⏳ | |
| 3 | 伪装后缀由「频道 ID + 切片哈希」确定性推导 | 观测 | ⏳ | |
| 4 | 切片首字节 `0x47`，为原始 MPEG-TS（188 字节/包） | 观测 | ⏳ | |
| 5 | 每片含 SDT 描述符 `lumberjack` 水印 | 观测 | ⏳ | |
| 6 | 切片 `Cache-Control` 先 `public` 后 `private`（窗口） | 观测 | ⏳ | |
| 7 | 订阅聚合**不做**反向代理，保留上游原始地址 | 观测 | ⏳ | |
| 8 | 面板密码以 **SHA-256** 存储，明文不落盘 | 观测 | ⏳ | |
| 9 | v0.8.2 回源流量统计失效（恒为 0） | 观测 | ⏳ | |
| 10 | 三种开关需重启：DASH 预启动 / 热重载监视 / 登录失败清理 | 观测 | ⏳ | |

> 上表仅列关键项；完整项从基线各文件抽取，逐条回填。

### 2.2 静态核验（2026-10-15，来源 [`../findings/static/01-sample-packing-and-toolchain.md`](../findings/static/01-sample-packing-and-toolchain.md)）

| 规格 ID | 声明 | 本次静态观测 | 状态 |
| --- | --- | --- | --- |
| S-ENV-02 | 预编译二进制 `iptv-proxy-linux-{amd64,arm64}` + 安装脚本 | 仓库内同时存在 `arm64`/`amd64`/`armv7` 产物与 `iptv-proxy.sh` | ✅ |
| S-ENV-06 | 启动命令为 `./iptv-proxy config.toml` | 安装脚本 `ExecStart=${BIN} ${CONFIG}`；无子命令 | ✅ |
| S-ENV-07 | 语言/工具链为 Rust（基线为"推断"） | 命中 `rustc version 1.99.0`、`/rustc/<hash>/`、`panicked at` | ✅（升级为**观测**） |
| S-ENV-08 | （新增）分发产物经 **UPX 4.24** 加壳 | `$Id: UPX 4.24`、熵 7.9999、无节头 | ✅ |
| S-EP-09 | 多 VPS 分享端点（基线路径待确认） | 路由字面量 `/api/share/pull` | ✅（路径确认） |
| S-CFG-04 | 顶层配置键名 | 全部键命中字面量（§7） | ✅（键名） |

> **`✅` 仅表示"静态存在性"一致**（键名/路由/工具链）。**行为**（如 EPG 关闭时 404、伪装字节）一律待动态核验。

## 3. 差异登记

出现 `⚠️ 差异` 时，在 `docs/findings/` 下建独立小节，包含：

1. 基线的原文结论与出处；
2. 本次证据（命令 / Evidence ID / 环境）；
3. 差异范围（是否版本差异、架构差异、环境差异）；
4. 处置（更新基线建议 / 待复测 / 已澄清）。

### 3.1 差异清单

| ID | 差异 | 范围 | 状态 |
| --- | --- | --- | --- |
| `D-001` | 基线版本 **v0.8.2**，但已取得产物 **≈ v0.8.9** | **版本差异** | ⏳ 待处置 |

**`D-001` 详情**

- 基线原文：`07` 头部与 `08` 标题均标注目标为 **iptv-proxy v0.8.2**（基线实测于 2026-10-02/03）。
- 本次证据：上游 `CHANGELOG.md` 最新为 **v0.8.8（2026-10-03）**；
  已取得产物内嵌面板含 **`[v0.8.9]`** 标注，构建于 **2026-10-15**（提交 `b6331774f8`）。
  证据见 [`../findings/static/01-sample-packing-and-toolchain.md`](../findings/static/01-sample-packing-and-toolchain.md) §5。
- 差异范围：**版本差异**（非架构/环境）。上游**无 tag/release**，`main` 随时变动。
- 处置：**待用户决定**（二选一）——
  (a) 以当前 `main`（≈ v0.8.9）为分析对象，另建 `08b-target-spec-v0.8.9.md`，不复用 `08` 状态列；
  (b) 取历史提交产物贴近 v0.8.2（候选：`ae6bf91bb1`=v0.8.8、`d302316fb8`=09-09），逐条比对后再定基线。

## 4. 收敛条件

"本版本逆向完成"须同时满足：

- [ ] 目标指纹已登记；
- [ ] 静态画像（段/符号/字符串/加固）已出；
- [ ] 六种模式行为已（静态或动态）覆盖；
- [ ] 核对矩阵无遗留 `⏳`（或已写明未核原因）；
- [ ] 所有 `⚠️ 差异` 已登记并给出处置；
- [ ] 未决项与未知已单列；
- [ ] `docs/findings/README.md` 索引已更新。
