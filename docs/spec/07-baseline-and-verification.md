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

### 2.3 动态核验（2026-10-16，来源 [`../findings/dynamic/01-runtime-surface-and-disguise.md`](../findings/dynamic/01-runtime-surface-and-disguise.md)）

| 规格 ID | 声明 | 本次运行实测 | 状态 |
| --- | --- | --- | --- |
| S-ENV-05 | 默认端口 `19899`，监听 `[::]` | 日志：`http://[::]:19899` | ✅ |
| S-EP-05 | EPG 关闭时 `/epg*` 全 404 | `/epg`、`/epg.xml`、`/epg.xml.gz` 均 404 | ✅ |
| S-EP-07 | `/unban` 无需登录 | `/unban` 200 HTML；无需 token | ✅ |
| S-EP-10 | `/health` | 200 `ok` | ✅ |
| S-DIS-01 | 伪装默认 `enabled=true`,`playlist=true` | `GET /panel/api/config` 默认值一致 | ✅ |
| S-DIS-02 | 不改载荷字节 | 切片字节与上游同长同内容 | ✅ |
| S-DIS-04 | 后缀由 频道ID+切片哈希 推导 | **切片哈希 = `sha256(上游切片绝对URL)[:32]`**；16-hex 频道前缀确定性但推导未解 | 🔶 部分 |
| S-DIS-05 | 后缀不参与校验 | 同 ID 换 8 种后缀，字节全同 | ✅ |
| S-DIS-11 | `obfuscate_filenames` 独立 | 位于 `[cache.dash]`，与 disguise 分区不同 | ✅ |
| S-CFG-01 | 面板改动落盘 `config.toml` | 建组后文件出现 `[[groups]]` | ✅ |
| S-CFG-02 | 配置热重载监视 | 启动日志：热重载监视已启动 | ✅ |
| S-CFG-04 | 顶层键与默认值 | `GET /panel/api/config` 全量（见 raw） | ✅ |
| S-AUTH-01 | 密码存 SHA-256，明文不落盘 | 登录 `admin` 成功；配置中 `password_sha256` 已屏蔽 | ✅ |
| S-AUTH-02 | Token 开启后播放需 token | 入口需 `?token=<48hex>`；无/错 token → 302 `token_failure_url` | ✅ |
| S-AUTH-03 | Token 有效期可配、失效跳转可配 | `token_ttl_hours=24`；日志「新 Token 有效期 24 小时」 | ✅ |
| S-DIS-16 | **切片不受 token 保护** | 无 token 取切片 → 200 | ✅ |
| S-URL-06 | 播放 token 参数名为 `token`，面板 Bearer 不适用 | `?t=` → 302；Bearer → 302 | ✅ |
| S-MODE-01 | 六种模式 | 面板 `<select id="mg-mode">` 六个 `<option>` | ✅ |
| S-MODE-02 | 模式属于分组 | `PUT /panel/api/groups/{id}` `{"mode":…}` 改变全组行为 | ✅ |
| S-MODE-08 | Redirect=302 直连；缓存上游地址 300s | 302 `location=上游`；`cache/m3u8/redirect_url_…` TTL=300 | ✅ |
| S-MODE-09 | Rewrite=取一次清单、相对改绝对 | 200 `text/plain`，切片=上游绝对 URL，客户端直连 | ✅ |
| S-MODE-11 | Passthrough 原样导出上游 URL | 302 `location=上游` | ✅ |
| S-MODE-05 | Remux 源容器自动识别 | 日志 `【remux】自动识别容器: Ts` | 🔶 部分 |
| S-MODE-06 | Dash MPD→HLS、片段级容错 | 解析 MPD 轨道 `video_360p`、取 `init.mp4`→404、失败清缓存 | 🔶 部分 |
| S-DIS-06 | 切片刚写入短暂 `public`，之后 `private`（均 `max-age=300`） | 人为延迟上游：首写 `public, max-age=300`；命中 `private, max-age=300`（3 次复现） | ✅ |
| S-DIS-07 | 切片忽略 `Range`，返回 200 全量，无 `Accept-Ranges`/`ETag` | `Range: bytes=100-199` → `200`、`content-length: 1880`、无 `Content-Range` | ✅ |
| S-DIS-10 | 媒体清单伪装为 `.../video/index.txt`；主清单入口仍 `.m3u8` | master 上游 → 主清单 `.m3u8`（`no-cache`），媒体清单 `video/index.txt`（`text/plain`） | ✅ |
| S-DIS-14 | 入口/媒体清单以 `text/plain` 下发 | 见上 | ✅ |
| S-EP-05 | EPG 关闭 404 / 开启 200 | 开启后 `/epg.xml`、`/epg.xml.gz`、`/epg` 均 200 且免登录 | ✅ |
| S-EP-12 | 面板 `/panel/api/…` EPG 全套 | `/panel/api/epg/status`、`/refresh` 可用（Bearer） | ✅ |
| S-EPG-01 | 采集源 XMLTV → 生成新版 XMLTV，根 `generator-info-name="iptv-proxy"` | 本地源 2 频道/2 节目；见 `05` | ✅ |
| S-EPG-02 | 时间裁剪：早于 `keep_days`、晚于 `future_days` 丢弃 | 10 天前/5 天后丢弃；今天未来保留 | ✅ |
| S-EPG-03 | 缺 `xml:lang` 补 `lang="zh"`；显式 lang 保留 | 缺→`zh`；`lang="en"` 保留 | ✅ |
| S-EP-06 | 台标 `GET /logo/<file>` 公开可读 | 200 `image/png`、`public, max-age=86400`；缺→404 | ✅ |
| S-AUTH-05 | 分组访问范围四档 | hidden→不进总订阅可直连；archived→分组订阅 404/播放 302、EPG 照常；open→免 token 且也不进总订阅 | ✅ |
| S-URL-03 | 订阅地址含分组 ID | `/{gid}/playlist.m3u` 200，`filename="<gid>.m3u"` | ✅ |
| S-URL-04 | Token 开启后导出 M3U 自动附加 token | public 条目 `?token=<48hex>`；open 组条目不带 | ✅ |
| S-URL-07 | `server.m3u_header_lines` 自定义 M3U 头 | 自定义行附加于 `#EXTM3U` 后，热重载+重启持久 | ✅ |
| S-CFG-02 | 配置热重载监视 | `[server]`/`[auth]` 文件直改即时生效；`[epg]` 例外；文件直改与面板保存有竞态 | ✅ |
| S-AUTH-09 | Token 轮换 + 旧 token 宽限 | `POST /security/token/rotate` 即生效；旧 token ≈1h 内仍 200 | ✅ |
| S-AUTH-10 | IP 封禁/解封 API 面 | ban `{"ips":[…]}`（默认 30 天）/unban `{"ip":…}`/stats；字段不对称 | ✅ |
| S-OPS-01 | 指标出口 | 仅 `/panel/api/{status,cache/stats,health}` | ✅ |
| S-OPS-02 | 公开指标端点 | `/status`、`/metrics`、`/version` 404；`/status/metrics` → 302 占位 | ✅ |

## 3. 差异登记

出现 `⚠️ 差异` 时，在 `docs/findings/` 下建独立小节，包含：

1. 基线的原文结论与出处；
2. 本次证据（命令 / Evidence ID / 环境）；
3. 差异范围（是否版本差异、架构差异、环境差异）；
4. 处置（更新基线建议 / 待复测 / 已澄清）。

### 3.1 差异清单

| ID | 差异 | 范围 | 状态 |
| --- | --- | --- | --- |
| `D-001` | 基线 **v0.8.2** 与上游 `main`（**v0.8.8**）不同 | **版本差异** | ✅ 已解决（取 v0.8.2 产物） |

**`D-001` 详情**

- 基线原文：`07` 头部与 `08` 标题均标注目标为 **iptv-proxy v0.8.2**（基线实测于 2026-10-02/03）。
- 现象：上游 `main` 随构建变动，当前 `main` 实为 **v0.8.8**（非 v0.8.2）。
- 证据：Rust 版本常量 `env!("CARGO_PKG_VERSION")` 落在固定 `.rodata` 槽
  `…local_path.m3u8start_epoch<VER>logo…`：`d302316fb8`=**0.8.2**、`b6331774f8`(main)=**0.8.8**。
  见 [`../findings/static/01-sample-packing-and-toolchain.md`](../findings/static/01-sample-packing-and-toolchain.md) §5。
- 差异范围：**版本差异**（非架构/环境）。上游**无 tag/release**，须以**提交 SHA**锚定。
- 处置：**已解决**——主分析对象改用 **v0.8.2 = `d302316fb8`**（与基线一致），登记于
  [`../findings/artifact.md`](../findings/artifact.md) §2.1。`main`(v0.8.8) 仅作**差异对照**，不作主分析对象。

## 4. 收敛条件

"本版本逆向完成"须同时满足：

- [ ] 目标指纹已登记；
- [ ] 静态画像（段/符号/字符串/加固）已出；
- [ ] 六种模式行为已（静态或动态）覆盖；
- [ ] 核对矩阵无遗留 `⏳`（或已写明未核原因）；
- [ ] 所有 `⚠️ 差异` 已登记并给出处置；
- [ ] 未决项与未知已单列；
- [ ] `docs/findings/README.md` 索引已更新。
