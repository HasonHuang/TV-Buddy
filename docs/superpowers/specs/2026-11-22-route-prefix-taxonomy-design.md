# 路由前缀分类设计（CDN 可缓存 / 不可缓存）

> 状态：设计待批准 ｜ 日期：2026-11-22
> 目标版本：下一次主版本。具体版本号由发布计划决定。
> 相关：[`../../design/03-http-contract.md`](../../design/03-http-contract.md)、[`../../deploy/04-cdn.md`](../../deploy/04-cdn.md)、[`../../design/06-low-spec-optimization.md`](../../design/06-low-spec-optimization.md)

## 1. 意图

让 CDN 只用少量前缀规则区分「可缓存」与「不可缓存」。

现状的问题是路径按功能分，不按缓存性分：

- 公开接口散落在 `/health`、`/unban/*`、`/subs/*`、`/epg/translate/*`、`/subtitle/*`、`/api/share/pull`。
- 播放清单与切片共用 `/<gid>/<cid>/...`。清单不可缓存，切片要缓存。
- 路径令牌入口 `/l/<token>/...` 与签名切片 `/<gid>/<cid>/l/<exp>/<sig>/...` 没有被现有 CDN 规则稳定覆盖。

新的分类规则：

| 前缀 | 缓存性 | 内容 |
| --- | --- | --- |
| `/api/*` | 不可缓存 | 面板 API、公开 API、用户订阅、播放清单 |
| `/media/*` | 可缓存 | 播放切片、初始化段、时移切片、签名切片 |
| `/static/*` | 可缓存 | 台标、面板静态资源、本地 VOD 素材 |
| `/page/*` | 不可缓存 | 面板页面、解封页面 |
| `/` | 不可缓存 | 首页跳转 |

CDN 规则简化为：

```text
不缓存：/api/*  /page/*  /
缓存：  /media/*  /static/*
```

分类原则：

- API 端点一律在 `/api/*`。
- HTML 页面一律在 `/page/*`。
- `/` 只做首页跳转，不是页面。
- 旧路径别名是唯一例外。它们在本次主版本内保留，下一主版本移除。

## 2. 目标

1. 所有不可缓存的 API 与清单都在 `/api/*` 下。
2. 所有 API 端点都在 `/api/*`。
3. 所有 HTML 页面都在 `/page/*` 下。页面路径只返回 HTML。
4. 所有可缓存的播放切片都在 `/media/*` 下。
5. 所有可缓存的静态资源都在 `/static/*` 下。
6. CDN 不再依赖多段通配符与扩展名规则。
7. 旧路径在本次主版本内继续可用。下一主版本再移除。

## 3. 非目标

- 不改上游片源地址与协议。
- 不改播放令牌、签名、IP 管控的语义。
- 不改切片字节、缓存窗口、TTL 计算。
- 不引入 URL 版本段，例如 `/v2/`。
- 不要求 CDN 跟随源站缓存头。强制 TTL 仍然可用。

## 4. 反目标

- 不能出现「清单在 `/media/*` 下」或「切片在 `/api/*` 下」。
- 不能因为改名让旧订阅、旧播放器或旧 CDN 缓存立即失效。
- 不能让旧路径与新路径产生不同的响应内容。
- 不能把面板 API 的鉴权、功能开关或管理员检查漏掉。

## 5. 当前状态

| 路径族 | 当前路径 | 缓存性 | CDN 规则现状 |
| --- | --- | --- | --- |
| 面板 API | `/panel/api/*` | 不可缓存 | `/panel*` 已覆盖 |
| 用户订阅 | `/sub/<token>/*` | 不可缓存 | `/sub/*` 已覆盖 |
| 公开辅助接口 | `/health`、`/unban/*`、`/subs/*`、`/epg/translate/*`、`/subtitle/*` | 不可缓存 | 分散规则已覆盖 |
| 分享接口 | `/api/share/pull` | 不可缓存 | `/api/*` 已覆盖 |
| 播放入口清单 | `/<gid>/<cid>.<ext>` | 不可缓存 | `/*/*.m3u8` 等已覆盖 |
| 变体清单 | `/<gid>/<cid>/video/index.txt` | 不可缓存 | `/*/*/video/*` 已覆盖 |
| DASH 轨道清单 | `/<gid>/<cid>/<track>/index.txt` | 不可缓存 | 路径补丁已覆盖 |
| 时移清单 | `/<gid>/<cid>/dash-timeshift/<track>/playlist*` | 不可缓存 | 路径补丁已覆盖 |
| 代理切片 | `/<gid>/<cid>/<seg>` | 可缓存 | `/*/*/*` 已覆盖 |
| 回看切片 | `/<gid>/<cid>/timeshift/<file>` | 可缓存 | 路径补丁已覆盖 |
| 路径令牌入口 | `/l/<token>/<gid>/<cid>.<ext>` | 不可缓存 | **未覆盖** |
| 签名切片 | `/<gid>/<cid>/l/<exp>/<sig>/<name>` | 可缓存 | **未覆盖** |

按 `*` 不跨 `/` 的常见语义：

- `/*/*.m3u8` 只匹配两段。`/l/<token>/<gid>.m3u8` 有三段，不会被命中。
- `/*/*/*` 只匹配三段。签名切片有六段，不会被命中。
- 不同 CDN 对 `*` 的解释不同。现有规则依赖控制台语义，不稳定。

## 6. 方案比较

| 方案 | 内容 | 代价 | 结论 |
| --- | --- | --- | --- |
| A 兼容优先 | 保留旧路径，只给公开接口加 `/api/*` 别名 | 旧播放路径仍需复杂规则 | 不能彻底解决 |
| B 清洁重排 | 新路径按缓存性分类；旧路径别名一个主版本 | 改动面大；需要迁移 | **已选** |
| C 只修规则 | 不改应用路由，只补 CDN 规则 | 未来每个新接口都要同步规则 | 不作为本次方案 |

选择 B。原因是它把「缓存性」变成路径契约。CDN 规则不再依赖路径深度、扩展名或供应商通配符语义。

## 7. 新路径映射

### 7.1 不可缓存：`/api/*`

| 旧路径 | 新路径 | 说明 |
| --- | --- | --- |
| `/panel/api/*` | `/api/panel/*` | 面板 API 全部迁移 |
| `/subs/status` | `/api/panel/subs/status` | 订阅源管理，面板功能 |
| `POST /subs/groups` | `POST /api/panel/subs/groups` | 同上 |
| `POST /subs/preview` | `POST /api/panel/subs/preview` | 同上 |
| `/epg/translate/*` | `/api/panel/epg/translate/*` | 与面板别名合并 |
| `/panel/api/epg/translate/*` | `/api/panel/epg/translate/*` | 去重 |
| `/subtitle/*` | `/api/panel/subtitle/*` | 与面板别名合并 |
| `/panel/api/subtitle/*` | `/api/panel/subtitle/*` | 去重 |
| `/health` | `/api/health` | 保留旧路径别名 |
| `/api/share/pull` | `/api/share/pull` | 不变 |
| `/unban/whoami` | `/api/unban/whoami` | |
| `POST /unban` | `POST /api/unban` | `GET /unban` 仍是页面，新路径 `/page/unban` |
| `/sub/<token>/playlist.m3u` | `/api/sub/<token>/playlist.m3u` | 用户订阅 |
| `/sub/<token>/<gid>/playlist.m3u` | `/api/sub/<token>/<gid>/playlist.m3u` | 单分组订阅 |
| `/sub/<token>/epg.xml[.gz]` | `/api/sub/<token>/epg.xml[.gz]` | 用户节目单 |

### 7.2 不可缓存：`/api/play/*`

| 旧路径 | 新路径 |
| --- | --- |
| `/<gid>/<cid>.<ext>` | `/api/play/<gid>/<cid>.<ext>` |
| `/<gid>/<cid>/index.m3u8` | `/api/play/<gid>/<cid>/index.m3u8` |
| `/<gid>/<cid>/video/index.txt` | `/api/play/<gid>/<cid>/video/index.txt` |
| `/<gid>/<cid>/<track>/index.txt` | `/api/play/<gid>/<cid>/<track>/index.txt` |
| `/<gid>/<cid>/<track>/index.m3u8` | `/api/play/<gid>/<cid>/<track>/index.m3u8` |
| `/<gid>/<cid>/dash-timeshift/<track>/playlist[.txt\|.m3u8]` | `/api/play/<gid>/<cid>/dash-timeshift/<track>/playlist[.txt\|.m3u8]` |
| `/<gid>/<cid>/l/<exp>/<sig>/index.m3u8` | `/api/play/<gid>/<cid>/l/<exp>/<sig>/index.m3u8` |
| `/<gid>/<cid>/l/<exp>/<sig>/video/index.txt` | `/api/play/<gid>/<cid>/l/<exp>/<sig>/video/index.txt` |
| `/<gid>/<cid>/l/<exp>/<sig>/<track>/index.txt` | `/api/play/<gid>/<cid>/l/<exp>/<sig>/<track>/index.txt` |
| `/<gid>/<cid>/l/<exp>/<sig>/<track>/index.m3u8` | `/api/play/<gid>/<cid>/l/<exp>/<sig>/<track>/index.m3u8` |
| `/<gid>/<cid>/l/<exp>/<sig>/dash-timeshift/<track>/playlist*` | `/api/play/<gid>/<cid>/l/<exp>/<sig>/dash-timeshift/<track>/playlist*` |
| `/l/<token>/<gid>/<cid>.<ext>` | `/api/play/token/<token>/<gid>/<cid>.<ext>` |
| `/l/<token>/<gid>/<cid>/index.m3u8` | `/api/play/token/<token>/<gid>/<cid>/index.m3u8` |

签名路径下的清单全部进 `/api/play/*`。只有切片与初始化段进 `/media/*`。

### 7.3 可缓存：`/media/*`

| 旧路径 | 新路径 |
| --- | --- |
| `/<gid>/<cid>/<seg>` | `/media/<gid>/<cid>/<seg>` |
| `/<gid>/<cid>/timeshift/<file>` | `/media/<gid>/<cid>/timeshift/<file>` |
| `/<gid>/<cid>/<track>/<file>` | `/media/<gid>/<cid>/<track>/<file>` |
| `/<gid>/<cid>/dash-timeshift/<track>/<file>` | `/media/<gid>/<cid>/dash-timeshift/<track>/<file>` |
| `/<gid>/<cid>/l/<exp>/<sig>/<name>` | `/media/<gid>/<cid>/l/<exp>/<sig>/<name>` |
| `/<gid>/<cid>/l/<exp>/<sig>/<track>/<file>` | `/media/<gid>/<cid>/l/<exp>/<sig>/<track>/<file>` |
| `/<gid>/<cid>/l/<exp>/<sig>/dash-timeshift/<track>/<file>` | `/media/<gid>/<cid>/l/<exp>/<sig>/dash-timeshift/<track>/<file>` |
| `/<gid>/<cid>/mpd/<token>/<base64>/...` | `/media/<gid>/<cid>/mpd/<token>/<base64>/...` |

签名前缀仍由 `sign::prefix` 生成，形状为 `l/<exp>/<sig>`。它现在插在 `/api/play/...` 或 `/media/...` 之后。

上表的 `<name>`、`<track>/<file>` 与 `dash-timeshift/<track>/<file>` 只指切片与初始化段。清单形状见 §7.2。`/media/*` 不返回清单。

### 7.4 可缓存：`/static/*`

| 旧路径 | 新路径 | 说明 |
| --- | --- | --- |
| `/logo/<file>` | `/static/logo/<file>` | 台标图片 |
| `/__assets/<path>` | `/static/panel/<path>` | Vite 构建产物 |
| `/assets/<path>` | `/static/assets/<path>` | 本地 VOD 素材与 HLS |
| `/assets/convert/<digest>/<file>` | `/static/assets/convert/<digest>/<file>` | 转换后的素材 |

`/static/assets/*` 是 VOD 命名空间。它包含本地 HLS 清单与切片。它们都按静态资源缓存。这是「清单不进缓存前缀」规则的唯一例外，原因是这些清单属于用户素材，不是直播清单。

### 7.5 页面与入口

| 旧路径 | 新路径 | 处理 |
| --- | --- | --- |
| `/panel`、`/panel/` | `/page/panel`、`/page/panel/` | 面板 HTML |
| `GET /unban` | `/page/unban` | 解封 HTML 页面。它不是 API |
| `POST /unban` | `POST /api/unban` | 解封接口的旧别名 |
| `/` | `/` | 首页跳转，不变 |
| `/playlist.m3u`、`/<gid>/playlist.m3u` | 不变 | 保留 `404` 指路 |

`/unban` 同时有页面与接口。GET 是页面，新路径是 `/page/unban`；POST 是接口，新路径是 `/api/unban`。新客户端只使用 `/page/unban`、`/api/unban` 与 `/api/unban/whoami`。

## 8. 兼容策略

### 8.1 旧路径别名

旧路径在本次主版本内继续可用。实现方式：

1. 在路由最外层加路径改写中间件。它先于 IP 管控、功能开关、管理员检查与业务路由。
2. 中间件只改写旧路径。新前缀 `/api`、`/media`、`/static` 与页面路径不改写。
3. 改写后进入新路由。旧路径与新路径使用同一套处理器。
4. 不返回重定向。`POST`、`PUT`、`DELETE` 不能依赖客户端跟随重定向。

### 8.2 旧路径改写顺序

先匹配前缀，再匹配播放形状。命中即停。

| # | 旧路径 | 改写目标 |
| --- | --- | --- |
| 1 | `GET /health` | `/api/health` |
| 2 | `/panel/api/*` | `/api/panel/*` |
| 3 | `/subs/*` | `/api/panel/subs/*` |
| 4 | `/epg/translate/*` | `/api/panel/epg/translate/*` |
| 5 | `/subtitle/*` | `/api/panel/subtitle/*` |
| 6 | `/sub/*` | `/api/sub/*` |
| 7 | `/l/*` | `/api/play/token/*` |
| 8 | `/logo/*` | `/static/logo/*` |
| 9 | `/__assets/*` | `/static/panel/*` |
| 10 | `/assets/*` | `/static/assets/*` |
| 11 | `GET /unban/whoami` | `/api/unban/whoami` |
| 12 | `POST /unban` | `/api/unban` |
| 13 | `GET /unban` | `/page/unban` |
| 14 | `GET /panel`、`GET /panel/` | `/page/panel`、`/page/panel/` |
| 15 | 播放形状 | 见下表 |

规则 2、12、13、14 的顺序不能反。`/panel/api/*` 与 `POST /unban` 必须先于页面别名。否则它们会被 `/panel`、`/unban` 页面规则挡住。

播放形状按下表顺序判断：

| # | 旧路径形状 | 条件 | 改写目标 |
| --- | --- | --- | --- |
| 1 | `/<gid>/playlist.m3u` | 固定 404 指路 | 不改写 |
| 2 | `/<gid>/<file>` | 任意两段路径 | `/api/play/<gid>/<file>` |
| 3 | `/<gid>/<cid>/l/<exp>/<sig>/<rest>` | `<rest>` 是清单形状 | `/api/play/<gid>/<cid>/l/<exp>/<sig>/<rest>` |
| 4 | `/<gid>/<cid>/l/<exp>/<sig>/<rest>` | `<rest>` 不是清单形状 | `/media/<gid>/<cid>/l/<exp>/<sig>/<rest>` |
| 5 | `/<gid>/<cid>/<rest>` | `<rest>` 是清单形状 | `/api/play/<gid>/<cid>/<rest>` |
| 6 | `/<gid>/<cid>/<rest>` | `<rest>` 不是清单形状 | `/media/<gid>/<cid>/<rest>` |

`is_manifest_rest(rest)` 为真，当且仅当 `rest` 是以下形状：

| 形状 | 示例 |
| --- | --- |
| `index.m3u8` | `g/c/index.m3u8` |
| `video/index.txt` | `g/c/video/index.txt` |
| `<track>/index.txt` | `g/c/video_720p/index.txt` |
| `<track>/index.m3u8` | `g/c/video_720p/index.m3u8` |
| `dash-timeshift/<track>/playlist` | `g/c/dash-timeshift/video_720p/playlist` |
| `dash-timeshift/<track>/playlist.txt` | 同上 |
| `dash-timeshift/<track>/playlist.m3u8` | 同上 |

其余形状按媒体处理。`mpd/*`、`timeshift/*`、`dash-timeshift/<track>/<seg>` 都按媒体处理。

两段 `/<gid>/<file>` 一律改写，不限于入口文件名。旧 `/<gid>/<cid>.mp4` 返回占位 302。改写后处理器保留该响应。这是兼容性放宽。处理器的扩展名白名单不变。

保留的前缀不参与改写：`/api/`、`/media/`、`/static/`、`/page/`、`/`。

中间件先检查保留前缀。命中即返回，不再做播放形状判断。这样 `/api/...` 与 `/media/...` 不会被二次改写。

### 8.3 新 URL 的生成

新请求生成新路径：

- 面板「我的订阅」生成 `/api/sub/<token>/playlist.m3u`。
- 订阅条目生成 `/api/play/token/<token>/<gid>/<cid>.<ext>`。
- 清单内的变体清单地址使用 `/api/play/...`。
- 清单内的切片地址使用 `/media/...`。
- 台标引用使用 `/static/logo/<file>`。
- 面板静态资源使用 `/static/panel/<path>`。

旧路径仍可访问，但不再生成。

### 8.4 弃用与移除

- 本次主版本：旧路径别名可用。文档标注「已弃用」。
- 下一主版本：移除旧路径别名。
- 旧路径的响应可加 `Deprecation: true`。播放路径不加，避免影响播放器。
- 除旧别名外，任何 API 路径不得位于 `/api/*` 之外。

### 8.5 已持久化 URL

数据库与配置可能保存旧 URL：

- 频道 `logo` / `tvg_logo`：`/logo/<file>`。
- `[assets].map` 本地地址：`/assets/convert/<digest>/<file>`。

本次主版本不做一次性数据库改写。采用读时归一化：

1. 新增 `normalize_asset_url`。
2. `/logo/` 映射到 `/static/logo/`。
3. `/assets/` 映射到 `/static/assets/`。
4. 订阅渲染、台标匹配报告、素材列表、素材读取都先归一化。
5. 新写入的值一律用新前缀。
6. 旧值继续可用。下一主版本再考虑一次性迁移。

理由：读时归一化不在升级时改用户数据。旧值通过别名仍可访问。

## 9. CDN 规则

### 9.1 迁移期

迁移期保留现有规则 A/B。新增规则叠加，不替换。

| 规则 | 内容 |
| --- | --- |
| 现有规则 A | 全部保留。再补 `/l/*` 与 `/logo/api/*` |
| 现有规则 B | 全部保留。再补旧签名切片 `/*/*/l/*/*/*`（控制台支持多段通配符时） |
| 新增不缓存 | `/api/*`、`/page/*` |
| 新增缓存 | `/media/*`、`/static/panel/*`、`/static/logo/*`、`/static/assets/*` |
| 兜底 | `/*` 不缓存，放在最后 |

兜底很重要。未命中路径默认必须是不缓存。不能把默认设为缓存。
如果控制台不支持 `/*` 兜底，把控制台的默认行为设为「不缓存」。
如果控制台不支持多段通配符，旧签名切片在迁移期可能不缓存。记录该差异，不阻断上线。

现有规则 B 已包含 `/logo/*` 与 `/assets/*`。迁移期继续缓存。旧 `/__assets/*` 保持现有的不缓存行为。

上线顺序：

1. 先发布应用。旧路径继续可用。
2. 再加新 CDN 规则。现有规则 A/B 保留。
3. 刷新缓存。
4. 验证清单 BYPASS、切片 HIT。

### 9.2 迁移完成后

```text
不缓存：
/api/*
/page/*
/

缓存：
/media/*
/static/panel/*
/static/logo/*
/static/assets/*
```

不再需要：

- `/*/*.m3u8`、`/*/*.mpd`、`/*/*/index.txt` 等扩展名规则。
- `/*/*/video/*`、`/*/*/mpd/*`、`/*/*/dash-timeshift/*` 等深度规则。
- 伪装扩展名列表。

### 9.3 TTL 与缓存键

| 前缀 | TTL | 原因 |
| --- | --- | --- |
| `/media/*` | 切片 TTL（推荐值） | 直播切片窗口 |
| `/static/panel/*` | 长 TTL，例如 1 年 | Vite 产物带内容哈希 |
| `/static/logo/*` | 短 TTL，例如 1 小时 | 文件名可能不变，内容会变 |
| `/static/assets/*` | 中等 TTL，例如 1 天 | 用户素材，更新频率低 |
| `/api/*` | 不缓存 | 清单、API、订阅 |

缓存键：

- `/api/play/*` 不缓存。令牌与签名不进 CDN 缓存。
- `/media/*` 的签名切片在同一时间窗内全用户相同。缓存可共享。
- `/static/panel/*` 的文件名带哈希。长 TTL 安全。
- `/static/logo/*` 的文件名不带哈希。TTL 要短。

## 10. 实现设计

### 10.1 路由结构

```text
/api/panel/*    面板 API（需要会话与管理员检查）
/api/health     健康检查
/api/share/*    分享接口
/api/unban/*    自助解封接口
/api/sub/*      用户订阅与节目单
/api/play/*     播放清单
/media/*        播放切片
/static/*       静态资源
/page/panel     面板页面
/page/unban     解封页面
/               首页跳转
```

### 10.2 中间件顺序

1. 路径改写中间件（只处理旧路径）。
2. IP 管控。
3. 功能开关。
4. 管理员检查。
5. 业务处理器。

IP 管控豁免：

- `/page/*`
- `/static/panel/*`
- `/api/panel/*`
- `/api/unban/*`
- `/panel*`（迁移期旧页面别名）
- `/unban*`（迁移期旧页面别名）

`/static/panel/*` 必须豁免。否则被封 IP 能打开页面，却加载不到 JS 与 CSS。
`/api/health` 不豁免。它与现在的 `/health` 语义一致。

改写中间件在 IP 管控之前运行。守卫看到的是规范路径。旧 `/panel/api/*` 已改写为 `/api/panel/*`；旧 `POST /unban` 已改写为 `/api/unban`；旧页面已改写为 `/page/*`。

管理员检查按 `/api/panel/*` 判断。开放列表：

- `/api/panel/login`
- `/api/panel/logout`
- `/api/panel/me`
- `/api/panel/my*`（如存在）

以下面板功能需要管理员：

- `/api/panel/subs/*`
- `/api/panel/epg/translate/*`
- `/api/panel/subtitle/*`
- 其余 `/api/panel/*`

功能开关按 `/api/panel/*` 子路径判断。原 `feature_of` 的路径表同步改为新前缀。

### 10.3 需要修改的路径生成点

| 模块 | 修改 |
| --- | --- |
| `src/http.rs` | 拆分 `api`、`media`、`static`、`page`、`legacy` 路由；挂载改写中间件 |
| `src/http/guard.rs` | 前缀从 `/panel/api` 改为 `/api/panel`；IP 管控豁免 `/page/*`、`/static/panel/*` 与新接口前缀；更新单测 |
| `src/http/routes/stream.rs` | 拆分 `play_base` 与 `media_base`；清单与切片分别使用 |
| `src/http/routes/sub.rs` | 订阅路径改为 `/api/sub/*` |
| `src/http/routes/users.rs` | `playlist_url`、`epg_url` 改为新路径 |
| `src/playlist.rs` | 订阅条目改为 `/api/play/token/*`；EPG 路径改为 `/api/sub/*`；台标读时归一化 |
| `src/remux.rs` | 清单切片路径改为 `/media/*` |
| `src/proxy/media.rs` | 代理清单的伪装切片与初始化段改为 `/media/*` |
| `src/http/routes/admin.rs` | 请求统计从 `/media/*` 与 `/api/play/*` 解析分组与频道，旧形状继续可用 |
| `src/dash_hls.rs` | 轨道清单函数改用 `paths::play_url`；初始化段与切片函数改用 `paths::media_url` |
| `src/dash_timeshift.rs` | 清单用 `play_url`；切片用 `media_url` |
| `src/timeshift.rs` | 切片用 `media_url` |
| `src/sign.rs` | `prefix` 形状不变；调用方换基址 |
| `src/http/routes/logo.rs` | 台标 URL 改为 `/static/logo/*`；匹配报告读时归一化旧 `/logo/*` |
| `src/http/static_assets.rs` | 面板与解封页面改为 `/page/*`；面板静态资源改为 `/static/panel/*` |
| `src/http/routes/media.rs` | 本地素材改为 `/static/assets/*`；面板素材与字幕改为 `/api/panel/*` |
| `src/config/model.rs` | `asset_local` 读时归一化旧 `/assets/*` |
| `frontend/vite.config.ts` | `base` 改为 `/static/panel/` |
| `frontend/src/api.ts` | API 基址改为 `/api/panel` |
| `frontend/src/unban.tsx` | 页面链接改为 `/page/panel`；接口改为 `/api/unban` 与 `/api/unban/whoami` |
| `frontend/src/assets.tsx` | 接受新旧素材前缀；优先使用后端归一化后的 URL |
| `frontend/src/*.tsx` | 直接 `fetch('/panel/api...')` 改为新路径；直接 `fetch('/unban...')` 改为 `/api/unban...` |
| `Dockerfile` | 健康检查改为 `/api/health` |

全仓 URL 生成点以本表为准。新增路径生成点必须走统一的 `play_url`、`media_url`、`static_url` 帮助函数。不要在各模块手写前缀。

### 10.4 路径不变量

增加测试，保证：

- `/api/*` 下不返回切片正文。健康检查、JSON、清单除外。
- `/media/*` 下不返回 `text/plain` HLS 清单或 `application/vnd.apple.mpegurl` 清单。
- `/media/*` 下不返回签名清单。
- `/static/assets/*` 是唯一可能包含 HLS 清单的缓存前缀。
- `is_manifest_rest` 对 `index.m3u8`、`video/index.txt`、`<track>/index.*`、`dash-timeshift/*/playlist*` 返回真。对 `mpd/*`、`timeshift/*`、普通切片返回假。
- `normalize_asset_url` 把旧 `/logo/*`、`/assets/*` 映射到新前缀。

可以用路由表测试。逐个注册路径断言分类，不必发真实请求。

### 10.5 旧路径与新路径一致性

对以下代表性路径做 A/B 测试：

| 类型 | 旧路径 | 新路径 |
| --- | --- | --- |
| 健康检查 | `/health` | `/api/health` |
| 面板页面 | `/panel` | `/page/panel` |
| 解封页面 | `/unban` | `/page/unban` |
| 面板状态 | `/panel/api/status` | `/api/panel/status` |
| 用户订阅 | `/sub/<token>/playlist.m3u` | `/api/sub/<token>/playlist.m3u` |
| 入口清单 | `/<gid>/<cid>.m3u8` | `/api/play/<gid>/<cid>.m3u8` |
| 切片 | `/<gid>/<cid>/<seg>` | `/media/<gid>/<cid>/<seg>` |
| 台标 | `/logo/<file>` | `/static/logo/<file>` |

要求：状态码、正文、缓存头一致。唯一允许的差异是清单内的绝对 URL 使用新前缀。

## 11. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 旧 CDN 缓存里的清单仍指向旧切片路径 | 旧路径别名保留一个主版本 |
| 旧订阅地址被播放器长期保存 | `/sub/*` 与 `/l/*` 别名保留一个主版本 |
| 浏览器书签指向 `/panel`、`/unban` | 旧页面别名保留一个主版本；新页面使用 `/page/*` |
| 面板前端有硬编码 `/panel/api` | 全局搜索并替换；`api.ts` 统一基址 |
| Vite 旧 HTML 引用 `/__assets/*` | 保留 `/__assets/*` 别名；新构建使用 `/static/panel/*` |
| DASH 清单与切片混合 | `play_base` 与 `media_base` 分离；用现有 DASH 冒烟测试验证 |
| 路径改写误伤播放路径 | 保留前缀白名单；只改写旧形状；加单测 |
| 静态台标 TTL 过长 | 台标与面板产物分开 TTL；台标使用短 TTL |
| CDN 仍按旧规则配置 | 文档给迁移期与迁移后两套规则 |

## 12. 验收

1. 新路径全部可用。
2. 旧路径全部可用，响应与新路径一致。
3. 页面使用 `/page/panel` 与 `/page/unban`。旧 `/panel` 与 `GET /unban` 可用。
4. 面板构建后使用 `/api/panel/*` 与 `/static/panel/*`。
5. 订阅导出使用 `/api/sub/*` 与 `/api/play/token/*`。
6. 清单内切片地址使用 `/media/*`。
7. Docker 健康检查使用 `/api/health`。
8. 迁移期 CDN 规则保留现有 A/B，并新增 `/api/*`、`/page/*`、`/media/*` 与三个 `/static/*` 子前缀。未命中路径默认不缓存。
9. 迁移后 CDN 规则只保留 `/api/*`、`/page/*`、`/`、`/media/*`、`/static/panel/*`、`/static/logo/*`、`/static/assets/*`。
10. `/static/panel/*`、`/static/logo/*`、`/static/assets/*` 使用不同 TTL。
11. 被 IP 管控封禁的客户端仍可访问 `/page/*`、`/static/panel/*`、`/api/panel/*` 与 `/api/unban/*`。
12. 旧 `logo` / `assets` 持久化值在订阅、台标报告与素材列表中正确显示。
13. remux 清单的切片地址使用 `/media/*`。
14. 签名清单在 `/api/play/*`，不在 `/media/*`。
15. `cargo test`、`cargo clippy -- -D warnings`、`cargo fmt --check` 通过。
16. 现有冒烟脚本通过；新增新路径冒烟脚本。
17. 除旧别名外，没有任何 API 路径在 `/api/*` 之外。
