# 03 · HTTP 契约

> 设计版本 v1.0 ｜ 2026-10-19 ｜ 状态：**初稿**
> 上游：[`02-architecture.md`](02-architecture.md) ｜ 判据：[`../spec/08-target-spec.md`](../spec/08-target-spec.md) §2–§4
> 证据：F-004、F-006、F-009、F-010、F-012 §3（路由表）、F-013（导入导出）、F-014

## 1. 范围

| 面 | 本文件覆盖 | 说明 |
| --- | --- | --- |
| 公开面 | 全部路由的**存在性** | P0 的播放与订阅给完整契约 |
| 播放面 | proxy 模式全链路、access、Token、缓存头 | 其余模式给状态码契约 |
| 面板面 | 已实现端点 + M1 端点 | 其余端点登记存在性，后续里程碑补契约 |
| 分享面 | `/api/share/pull` | 登记存在性（P2 实现） |

## 2. 通用约定

| 项 | 约定 | 依据 |
| --- | --- | --- |
| 会话鉴权 | `Authorization: Bearer <uuid>` 或 `X-Token: <uuid>` | S-AUTH-01、F-004 |
| 面板错误体 | `{"error":"<文本>"}` | F-013 §5 |
| 播放失败 | `302` 跳占位视频；**不用错误体** | S-DIS-15、F-004 |
| 面板兜底 | `/panel/api/*` 未匹配 → `404` JSON；其余未匹配 → `302` 占位 | S-EP-08 |
| 播放 Token 参数 | `?token=<48hex>`；`?t=` 无效 | S-URL-06 |
| 前端资源前缀 | `/__assets/*` | S-EP-16 |
| 系统时区 | 回看时间戳按 `server.timezone_offset_hours` 解析 | S-URL-05 |

状态码用量：

| 码 | 用途 |
| --- | --- |
| `200` | 成功；面板成功体多为 `{"ok":true}` |
| `302` | 占位跳转（未知频道、归档组、Token 失效、未匹配路径）、直连模式跳转 |
| `400` | 参数或请求体不合法 |
| `401` | 未登录、口令错误 |
| `403` | 边缘密钥不符、面板关闭 |
| `404` | 分组订阅（归档组）、未知面板接口、`/logo/<缺文件>` |
| `409` | 分组 id 已存在 |
| `422` | 请求体字段缺失（如 `subs/preview` 缺 `source.id`） |
| `500` | 内部错误 |
| `502` | 上游返回非预期内容 |
| `503` | `remux`/`dash` 预热；`Retry-After: 2`（remux） |

## 3. 公开面路由

| 方法 | 路径 | 鉴权 | 响应 | 阶段 | 依据 |
| --- | --- | --- | --- | --- | --- |
| GET | `/playlist.m3u` | 无 | `200` M3U | M1 | S-EP-02 |
| GET | `/{gid}/playlist.m3u` | 无 | `200` M3U / `404`（归档组） | M1 | S-EP-02、F-009 |
| GET | `/{gid}/{cid}.{ext}` | 按 token 开关 | `200` 清单 / `302` 占位 | M2 | S-EP-03 |
| GET | `/{gid}/{cid}/index.m3u8` | 同上 | 同上（兼容旧路径） | M2 | F-012 §3 |
| GET | `/{gid}/{cid}/video/index.txt` | 无 | `200` `text/plain` 媒体清单（仅主清单上游）；否则 `302 error_url` | M2 | F-004、F-018 |
| GET | `/{gid}/{cid}/<P>_<S>.<ext>` | 无 | `200` 切片 | M2 | S-DIS-04 |
| GET | `/{gid}/{cid}/timeshift/{filename}` | 按 token 开关 | `200` 本地回看 | P1 | F-014 |
| GET | `/{gid}/{cid}/dash-timeshift/{track}/{...}` | 按 token 开关 | `200` DASH 回看 | P2 | S-EP-16 |
| GET | `/epg` | 无 | `200` `text/html`；EPG 关 → `404` | ✅ P1 | S-EP-05 |
| GET | `/epg.xml`、`/epg.xml.gz` | 无 | `200` `application/xml` / `application/gzip`；EPG 关 → `404` | ✅ P1 | S-EP-05 |
| GET | `/epg/api/{channels,programmes/:channel,search}` | 无 | `200` JSON | P1 | S-EP-16 |
| GET | `/logo` | 无 | `200` `text/html` | ✅ P1 | S-EP-06 |
| GET | `/logo/<file>` | 无 | `200` 图片 + `public, max-age=86400`；缺文件、越界名、非白名单扩展 → `404` | ✅ P1 | F-011 §6 |
| GET | `/logo/api/list` | 无 | `{"allowed_exts","count","logos":[{"file","name","ext","bytes","mtime","url"}]}` | ✅ P1 | F-011 |
| GET | `/unban` | 无 | `200` `text/html` | P1 | S-EP-07 |
| GET | `/unban/whoami`、`/whoami` | 无 | `200` `{"ip":"<地址>"}`；取 `X-Forwarded-For` 首项或 `X-Real-IP`，都没有时 `unknown` | ✅ P1 | S-EP-15 |
| GET | `/health` | 无 | `200` `ok`（`text/plain`） | ✅ M0 | S-EP-10 |
| GET | `/panel`、`/panel/`、`/panel/epg` | 无 | `200` `text/html` | ✅ M0 | S-EP-01 |
| GET | `/__assets/*path` | 无 | `200` 静态资源 / `404` | ✅ M0 | S-EP-16 |

## 4. 播放契约（proxy）

> 状态：**已实现，并经对拍确认**（`src/http/routes/stream.rs`、`src/proxy/`）。
> 验证：`scripts/smoke-m2.sh`、`scripts/compare-p0.sh`（22 项全一致，见 `reconciliation-report.md`）。

代理链路按**上游清单类型**分两支（F-018）：

```text
上游是媒体清单（单码率）
GET /{gid}/{cid}.m3u8
  → 200  text/plain; charset=utf-8
         cache-control: public, max-age=<TARGETDURATION>, s-maxage=<同上>
         正文 = 上游媒体清单；切片改写为绝对伪装地址
                http://<host>/{gid}/{cid}/<16hex>_<32hex>.<ext>
GET /{gid}/{cid}/video/index.txt
  → 302  跳 error_url（该路径只在主清单上游时存在）

上游是主清单（多码率）
GET /{gid}/{cid}.m3u8
  → 200  application/vnd.apple.mpegurl
         cache-control: no-cache, no-store
         正文 = #EXTM3U
                #EXT-X-VERSION:6
                #EXT-X-INDEPENDENT-SEGMENTS
                #EXT-X-STREAM-INF:<最佳变体原属性>
                http://<host>/{gid}/{cid}/video/index.txt
GET /{gid}/{cid}/video/index.txt
  → 200  text/plain; charset=utf-8
         cache-control: public, max-age=<变体 TARGETDURATION>, s-maxage=<同上>
         正文 = 最佳变体的媒体清单；切片改写为绝对伪装地址

GET /{gid}/{cid}/index.m3u8
  → 与入口同义（兼容别名）
GET /{gid}/{cid}/<P>_<S>.<ext>
  → 200  切片；忽略 Range（返回全量）
         首次写入 public, max-age=300；命中缓存 private, max-age=300
```

| 项 | 契约 | 依据 |
| --- | --- | --- |
| 清单缓存窗口 | `max-age` = 清单的 `#EXT-X-TARGETDURATION`；缺失时按 6 秒 | F-018 |
| 切片地址 | 绝对地址（`http://<Host>/…`），不是根相对路径 | F-018 |
| 扩展名 | `format=hls` → `.m3u8`；`format=mpd` → `.mpd` | S-URL-01 |
| `<S>` | `sha256(上游切片绝对 URL)` 前 32 hex | S-DIS-04 |
| `<P>` | 16 hex，仅依赖 `(gid,cid)`；错值 → `302` | S-DIS-17、F-017 |
| 扩展名轮换 | 取 `[cache.disguise].formats`，每频道独立序列 | S-DIS-04 |
| 切片类型 | 按伪装后缀给 `Content-Type` | S-DIS-13 |
| 载荷 | 不改字节 | S-DIS-02 |
| Token | 入口按开关校验；**切片与子路径不校验** | F-007、F-018 |
| 未知分组/频道（入口形状） | 无/错 Token → `token_failure_url`；Token 合法 → `not_found_url` | F-018 |
| 未知路径（其它形状） | `302 not_found_url`，不看 Token | F-018 |
| 归档组 | 入口 `302 not_found_url`（先于 Token 判定）；订阅 `404` | F-009、F-018 |
| `Range` | 忽略 | S-DIS-07 |
| 回看 `playseek` | 只把 `playseek` 原样追加到上游 URL；`start`/`end`/`utc`/`utcend` 丢弃 | S-EP-04 |
| 未知单段路径 | `404`（如 `/status`）；两段及以上按播放形状处理 | S-EP-08 |
| remux 预热 | `503` + `Retry-After: 2`，体 `remux warming up` | S-MODE-16 |
| dash 未就绪 | `503 timeout waiting for master playlist` | S-MODE-15 |

### 4.1 实现细节（内部，不是兼容契约）

| 项 | 取值 | 说明 |
| --- | --- | --- |
| `<P>` 算法 | `sha256("<gid>:<cid>")` 前 16 位十六进制 | 目标算法未解（F-017）；校验失败仍按契约 302 |
| 扩展名轮换起点 | `sha256("<gid>:<cid>")` 首字节 + 切片序号 | 目标轮换算法未解；只保证取自配置列表 |
| 入口 Host | 取请求 `Host` 头；有 `X-Forwarded-Proto` 时用其 scheme | 用于拼切片绝对地址 |
| 上游清单复用 | 进程内 TTL 5 秒 | 防突发请求打穿上游 |
| 切片索引 | `cache/seg_url/<sha256(gid/cid)[..16]>.json`（`S` → 上游 URL + 扩展名） | 内部实现 |
| 切片正文 | `cache/seg_bytes/<gid>_<cid>_<S>.<ext>` | 同上 |
| 伪装关闭 | 媒体清单直接给上游绝对地址，不建索引 | 目标行为未采证 |
| `#EXT-X-KEY` 等 | `URI="…"` 绝对化，不改名 | S-DIS-02（不改载荷） |
| 上游失败 | `502` + `Upstream Error` | F-012 §3 错误串 |

### 4.2 直连模式（M3）

| 模式 | 入口行为 | 细节 | 依据 |
| --- | --- | --- | --- |
| `redirect` | `302` 到上游 URL | `redirect_mode` = `解析最佳画质` 时先取主清单挑**最高码率**变体；地址按 `redirect_ttl`（0 → 300 s）缓存；失败不缓存，改跳 `error_url` | S-MODE-08/13、F-018 |
| `passthrough` | `302` 到上游 URL | **校验 Token**（与其它模式一致），不缓存上游清单 | S-MODE-11、F-018 |
| `rewrite` | `200 text/plain; charset=utf-8` + `no-cache, must-revalidate` | 相对地址改绝对，切片客户端直连；清单缓存 ≈5 s | S-MODE-09/14、F-018 |
| `remux` | `503` + `Retry-After: 2`，体 `remux warming up` | 转封装需 `ffmpeg`（P1）；首次请求抓上游并按 `hls`/`raw` 记日志（S-MODE-18：HLS 上游永不就绪） | S-MODE-16/18 |
| `dash` | 上游正文透传（`application/dash+xml`） | 就绪门 30 s，超时 → `503 timeout waiting for master playlist`；MPD→HLS 转封装属 P1 | S-MODE-15 |
| `proxy` | 代理链路（§4） | — | S-MODE-03 |

> 差异：`remux`/`dash` 的转封装未实现（需 `ffmpeg`，见 ADR-04）。`proxy` 模式下的 MPD 正文与 `dash` 一样**原样透传**，避免按行改写破坏 XML。

## 5. 订阅契约

### 5.1 响应头

| 路径 | 头 |
| --- | --- |
| `/playlist.m3u` | `content-type: application/x-mpegurl; charset=utf-8`、`content-disposition: attachment; filename="playlist.m3u"` |
| `/{gid}/playlist.m3u` | 同上，`filename="<gid>.m3u"` |

### 5.2 正文形状

```text
#EXTM3U x-tvg-url="http://<host>/epg.xml.gz"      ← EPG 开启才有
<server.m3u_header_lines 逐行原文>
#EXTINF:-1 tvg-id="<tvg_id||id>" tvg-name="<tvg_name||name>" tvg-logo="<logo>" group-title="<分组显示名>",<频道名>
http://<host>/<gid>/<cid>.<ext>[?token=<48hex>]
```

| 项 | 规则 | 依据 |
| --- | --- | --- |
| `tvg-id` | 缺省用频道 id | S-MODEL-04 |
| `tvg-name` | 缺省用频道名 | S-MODEL-04 |
| `group-title` | 分组**显示名** | F-013 §3 |
| Token 附加 | `token_enabled` 且 `access != open` 时附加 | S-URL-04、F-009 |
| `m3u_header_lines` | 追加在 `#EXTM3U` 后；热重载生效 | S-URL-07 |

### 5.3 `access` 矩阵

| access | 总订阅 | 分组订阅 | 直播入口 | EPG |
| --- | --- | --- | --- | --- |
| `public` | ✅ 含 | ✅ | 按 Token 开关 | ✅ |
| `hidden` | ❌ | ✅ `200` | ✅ | ✅ |
| `archived` | ❌ | ❌ `404` | ❌ `302` 占位 | ✅ 仍含 |
| `open` | ❌ | ✅（条目无 token） | ✅ 免 Token | ✅ |

> `open` 同时**排除出总订阅**——与 `hidden` 同规则（F-009）。

### 5.4 订阅聚合（P1 已实现）

`GET /playlist.m3u` 可合并外部 M3U 源（F-013 §5）。分组订阅不聚合。

| 配置 | 含义 |
| --- | --- |
| `[playlist].merge_ttl_secs` | 单个源的抓取缓存时长（秒，默认 300） |
| `[playlist].merge_position` | `top` 或 `bottom`（默认 `bottom`） |
| `[playlist].global_exclude` | 对所有源生效的过滤词 |
| `[[playlist.sources]].enabled` | `false` 时跳过 |
| `[[playlist.sources]].include` / `.exclude` | 源级过滤词；`include` 为空表示全收 |

过滤词规则：

| 写法 | 匹配对象 |
| --- | --- |
| `group:<文本>` | `group-title` 包含文本 |
| `id:<文本>` | `tvg-id` 完全相等 |
| 其它 | 频道名包含文本 |

规则与限制：

| 项 | 规则 |
| --- | --- |
| 大小写 | 不区分 |
| 优先级 | `exclude` 与 `global_exclude` 优先于 `include` |
| 地址 | 保留上游原始 URL；**不改写**，也不加本服务 Token |
| 失败 | 单个源超时或非 2xx → 跳过并记日志；本地订阅照常返回 `200` |
| 正则 | `/…/` 形式未实现（P2） |

## 6. 面板接口契约

### 6.1 已实现（M0）

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| POST | `/panel/api/login` | `{"username","password"}` | `{"token":"<uuid>"}`；错 → `401` |
| POST | `/panel/api/logout` | — | `{"ok":true}` |
| GET | `/panel/api/status` | — | 状态 JSON（含 `version`、`entry_exts`） |
| GET | `/panel/api/config` | — | 全量配置 JSON |
| PUT | `/panel/api/config` | 全量配置 | `{"ok":true}`；非法 → `400` |
| POST | `/panel/api/reload` | — | `{"ok":true}` |

### 6.2 M1 落地

| 方法 | 路径 | 请求 | 响应 | 依据 |
| --- | --- | --- | --- | --- |
| POST | `/panel/api/groups` | `{"id":"<gid>", …}` | `{"ok":true}`；重复 → `409` `Group id already exists` | F-013 §5 |
| PUT | `/panel/api/groups/{gid}` | **局部**字段 | `{"ok":true}`；缺组 → `404` `Group not found` | F-005、F-013 §5 |
| DELETE | `/panel/api/groups/{gid}` | — | `{"ok":true}`；缺组 → `404` | F-013 §5 |
| POST | `/panel/api/groups/{gid}/channels` | `{"id","name","url", …}` | `{"ok":true}` | F-004 |
| PUT | `/panel/api/groups/{gid}/channels/{cid}` | 局部字段 | `{"ok":true}` | F-004 |
| DELETE | `/panel/api/groups/{gid}/channels/{cid}` | — | `{"ok":true}`；缺 → `404` `Channel not found` | F-013 §5 |
| GET | `/panel/api/groups/export` | — | 顶层**数组**（完整分组对象） | F-013 §1 |
| POST | `/panel/api/groups/import` | — | 体为分组数组；id 已存在则跳过；`{"ok":true,"imported":n,"skipped":m}` | F-013 |
| POST | `/panel/api/groups/import/force` | — | 同上，但同 id 覆盖 | F-013 |
| POST | `/panel/api/groups/import/m3u` | — | 体 `{text,mode,group_mode}`；解析后落库 | F-013 §2 |
| GET | `/panel/api/logo/list` | — | 同 `/logo/api/list` | F-012 |
| POST | `/panel/api/logo/upload` | — | 体 `{"items":[{"name","ext","data"}]}`（`data` 可带 `data:image/…;base64,` 前缀）；返回 `{"ok":true,"added":[…],"failed":[…]}` | F-012 |
| POST | `/panel/api/logo/delete` | — | 体 `{"file"}`；成功 `{"ok":true,"removed":1}`；缺文件 → `404` | F-012 |
| GET | `/panel/api/groups/{gid}/export` | — | 单个分组对象 | F-013 §1 |

写入语义：分组与频道改动**立即重写** `config.toml`；`PUT /config` 是**全量替换**（F-013 §6）。

### 6.2.1 M3U 导入语义

请求体是 JSON（不是 multipart）：`{"text":"#EXTM3U…","mode":"append","group_mode":"auto"}`。

| 项 | 规则 |
| --- | --- |
| `mode` | `append`（默认，同名分组跳过）｜`merge`（同名分组补新频道）｜`replace`（同名分组整体替换）。其它值 → `400` |
| `group_mode` | `auto`（默认）或 `proxy`/`remux`/`dash`/`redirect`/`rewrite`/`passthrough`。其它值 → `400` |
| `auto` 判定 | 分组内有 `.mpd` 地址 → `mode=dash`、`format=mpd`；否则 `mode=rewrite`、`format=hls`（与 F-013 观测一致） |
| 分组 id | `group-title` 去掉非 ASCII 字母数字与 `_`/`-`；结果为空 → `g-<6hex>` |
| 频道 id | 有 `tvg-id` 用 `tvg-id`；否则 `g-<6hex>`（不幂等，与目标一致）。组内重复时追加 `-2`、`-3` |
| 无 `group-title` | 落到分组 `Default` |
| 属性保留 | `tvg_id`/`tvg_name`/`tvg_logo`/`group_title` 存入频道 `extra`；`#EXTVLCOPT`、`#EXTHTTP` 与行内 `http-user-agent`/`http-referer` 转为 `channel.headers` 字符串 |
| 响应 | `{"ok":true,"parsed_groups","parsed_channels","added_groups","merged_channels","replaced_groups","skipped_groups","group_mode"}` |

> `channel.headers` 在**代理/改写/串流（dash、remux）取上游清单与切片**时附加。302 模式（redirect、passthrough）由客户端直连上游，无法附加。空 `text` 或未解析到频道 → `400`。

### 6.3 后续里程碑（登记存在性）

| 组 | 路径 | 阶段 |
| --- | --- | --- |
| 缓存 | `POST /panel/api/cache/flush`、`GET /panel/api/cache/stats` | ✅ M4 |
| 素材 | `/panel/api/assets/{list,convert,remove,ffmpeg-status}` | P2 |
| 录制 | `/panel/api/schedule`、`/schedule/from-epg`、`/schedule/{idx}`、`/schedule/{idx}/stop`、`/schedule/download/{filename}` | P1 |
| 推流 | `/panel/api/livestream`、`/livestream/{gid}/{cid}/{start,stop}` | P1 |
| DASH | `/panel/api/dash/{status,scan-url,tracks/:gid/:cid}`、`/dash-timeshift/status` | P2 |
| EPG | `/panel/api/epg/{status,refresh,regenerate,mappings,programmes,scripts,match-report,mapping,extra,script,script-delete,cron-preview}`、`/epg/{suggest,search,viewer-channels,source-channels,tz-preview}` | P1 |
| 安全 | `/panel/api/security/{token,token/rotate,ip/stats,ip/ban,ip/unban}` | ✅ P1（见 §6.7）；`ip/unban-batch` 未实现 |
| 指标 | `/panel/api/{stats,logs,health}` | ✅ M4（`health` 在 M0） |
| 备份 | `GET /panel/api/backup?include=all\|epg-scripts\|resolver`、`POST /panel/api/restore` | ✅ P1（见 §6.6） |
| 导入 | `POST /panel/api/groups/import/m3u` | ✅ P1（见 §6.2） |
| 导入 | `/groups/{import,import/force}`（JSON 分组数组） | ✅ P1（`/groups/import/m3u` 相邻） |
| 订阅聚合 | `POST /panel/api/subs/{groups,preview}` | P2 |
| 翻译 | `/panel/api/epg/translate/{run,status,clear,probe,models,mtran/test}`、`/subtitle/translate/clear` | P2 |
| 分享 | `GET /api/share/pull?key=`（仅 GET/HEAD） | P2 |
| 解析脚本 | `/panel/api/resolver/{scripts,script-delete}` | P2 |

### 6.4 运维接口（M4 已实现）

响应形状为本实现自定；目标只登记了存在性与用途（F-010），未逐字段采证。

| 方法 | 路径 | 响应 |
| --- | --- | --- |
| `GET` | `/panel/api/cache/stats` | `{"backend","disk_path","entries":{"m3u8","seg_url","seg_bytes","sub_url","dash"},"bytes","playlist_cache_entries"}` |
| `POST` | `/panel/api/cache/flush` | `{"ok":true,"removed":<文件数>}`；清 `m3u8`/`seg_url`/`seg_bytes`/`sub_url`，**保留 `dash`**（S-CACHE-06）；同时丢弃进程内索引与清单缓存 |
| `GET` | `/panel/api/stats` | `{"version","uptime_secs","requests","playlists","segments","segment_hits","upstream_failures","online_window_secs","groups":[{"id","requests"}]}` |
| `GET` | `/panel/api/logs?limit=n` | `{"total","lines":[…]}`；`limit` 上限 500，默认 200；来源为进程内环形缓冲（500 行） |
| `POST` | `/panel/api/reload` | 立即重读配置；返回 `{"ok":true}` |

热重载：只监视配置文件本身；解析失败保留旧快照并记 `error` 日志（`design/04` §2）。冷却中的频道在配置重载后立即解除冷却（S-MODE-17）。

### 6.5 EPG（P1 已实现）

证据：F-008（S-EP-05、S-EP-12）。目标共 12 个 `/panel/api/epg/*` 端点；本实现先做公开三件与状态/刷新。

| 方法 | 路径 | 鉴权 | 行为 |
| --- | --- | --- | --- |
| `GET` | `/epg.xml` | 无 | `200 application/xml; charset=utf-8`；`[epg].enabled = false` 或尚未生成 → `404` |
| `GET` | `/epg.xml.gz` | 无 | `200 application/gzip`（gzip 魔数 `1f 8b`）；同上 `404` |
| `GET` | `/epg` | 无 | `200 text/html`（内嵌浏览页）；关闭时 `404` |
| `GET` | `/panel/api/epg/status` | Bearer | `{"enabled","running","sources","channels","programmes","kept","keep_days","future_days","last_success","last_error"}` |
| `POST` | `/panel/api/epg/refresh` | Bearer | 同步采集一次；`{"ok":true,"msg":"采集完成","channels","programmes"}`；失败 → `500` + 原因 |
| `GET` | `/epg/api/channels` | 无 | `{"count","channels":[{"id","names":[{"lang","name"}]}]}` |
| `GET` | `/epg/api/programmes/{channel}` | 无 | `{"channel","count","programmes":[{"channel","start","stop","title","desc","categories"}]}` |
| `GET` | `/epg/api/search?q=` | 无 | `{"q","count","programmes":[…]}`；最多 200 条；空 `q` 返回 0 |

采集规则（F-008 观测值）：

| 项 | 规则 |
| --- | --- |
| 源类型 | 只实现 `kind = "xml"`（按 `url` 拉 XMLTV）。`kind = "script"` 需 `python3`，跳过并记日志 |
| 时间窗 | 保留 `stop >= 现在 - keep_days` 且 `start <= 现在 + future_days`；`keep_yesterday = true` 时下界不晚于昨天 0 点 |
| 频道匹配 | `match_by_name = true` 时按频道名匹配；同时接受 `tvg_id`/`tvg-id`/`epg_id` 显式键。未匹配的频道与节目丢弃 |
| 输出 | `<cache>/epg/epg.xml` 与 `epg.xml.gz`；根节点 `generator-info-name="iptv-proxy"`；保留 `channel/@id` 与 `programme` 的 `start`/`stop`/`channel` |
| 语言 | 文本节点缺 `xml:lang` 时补 `lang="zh"`；显式语言保留 |
| 原样保留 | `icon`、`credits` 等未建模子节点按原片段输出 |
| 调度 | 启动即采集一次；之后按 `[epg].cron`，只识别 `0 */N * * *`（N 小时），其它写法回退 4 小时 |

> 三个 `/epg/api/*` 的响应形状为本实现自定（目标只登记了路由存在性）。
> 实现：按请求读取 `epg.xml` 并解析；节目单较大时再改为内存快照。
>
> 未实现：翻译接口、脚本源、节目单映射与录制联动（P2）。

### 6.6 备份与恢复（P1 已实现）

| 项 | 规则 |
| --- | --- |
| 备份请求 | `GET /panel/api/backup?include=all|epg-scripts|resolver`；其它值回退 `all` |
| 备份响应 | `200 application/gzip`；`Content-Disposition: attachment; filename="iptv-proxy-backup-YYYYmmdd-HHMMSS.tar.gz"`（UTC） |
| 包内布局 | `iptv-proxy-backup/config.toml`、`logos/*`、`epg/scripts/*.py`、`epg/epg_map.json`、`resolvers/*.py`、`manifest.json`（`generator`/`version`/`created`/`scope`） |
| 恢复请求 | `POST /panel/api/restore`，`Content-Type: application/gzip`，体为 tar.gz 原文；上限 64 MiB |
| 恢复步骤 | 解包 → 解析并**校验** `config.toml` → 旧文件另存 `config.toml.bak` → 原子写入 → 恢复台标 → 重载快照（立即解除看门狗冷却） |
| 恢复响应 | `{"ok":true,"config_reloaded":true,"report":{"files":[…],"skipped":[…]},"groups":N}` |
| 失败 | 无 `config.toml`、配置不合法、非 gzip → `400`；未登录 → `401` |

> 台标以外的脚本类文件（`epg/scripts`、`resolvers`）只备份，不在恢复时执行。

### 6.7 安全面（P1 已实现）

响应形状为本实现自定（目标只登记了路由存在性，F-010）。

| 方法 | 路径 | 响应 |
| --- | --- | --- |
| `GET` | `/panel/api/security/token` | `{"enabled","ttl_hours","current","old"}`；`old` 为宽限期内的旧值 |
| `POST` | `/panel/api/security/token/rotate` | `{"ok":true,"token":"<48hex>"}`；旧值保留 1 小时宽限（S-AUTH-09） |
| `GET` | `/panel/api/security/ip/stats` | `{"count","banned","entries":[{"ip","requests","failures","banned","remaining_secs"}]}`，按请求数降序 |
| `POST` | `/panel/api/security/ip/ban` | `{"ip","seconds"}`；`seconds = 0` 用默认 3600，上限 30 天；返回实际时长 |
| `POST` | `/panel/api/security/ip/unban` | `{"ip"}` 或 `{"ips":[…]}`；`{"ok":true,"removed":n,"requested":n}` |

执行规则：

| 项 | 规则 |
| --- | --- |
| 记账 | 非 `/panel` 路径按 `X-Forwarded-For` 首项或 `X-Real-IP` 记账；都没有时记 `unknown` 并忽略 |
| 拦截 | 封禁期内 → `403` 纯文本 `banned`；面板路径不拦截（避免管理员自锁） |
| 容量 | 最多保留 10 000 个地址，超出后忽略新地址 |
| 未实现 | `ip/unban-batch`、按 IP 的会话上限、自动限流与 `/unban` 自助解封（P2） |

### 6.8 面板分区（前端）

| 分区 | 数据来源 | 状态 |
| --- | --- | --- |
| 概览 | `GET /panel/api/status`、`GET/PUT /panel/api/config` | ✅ |
| 分组管理 | `/panel/api/{groups,channels}` 与导入 | ✅ |
| 安全管控 | `/panel/api/security/{token,token/rotate,ip/stats,ip/ban,ip/unban}` | ✅ |
| 运行日志 | `GET /panel/api/logs?limit=`（5 秒轮询） | ✅ |
| 备份恢复 | `GET /panel/api/backup`、`POST /panel/api/restore`（浏览器内下载/上传） | ✅ |
| 其它（缓存、录制、热度、素材等） | — | 待实现，侧栏置灰 |

## 7. 错误串对照

面板错误体保留目标原文，便于前端与外部脚本匹配（F-012 §3、F-013 §5）：

| 串 | 场景 |
| --- | --- |
| `Group not found` | 分组不存在 |
| `Group id already exists` | 建组 id 重复 |
| `Group archived` | 归档组播放 |
| `Channel not found` | 频道不存在 |
| `Invalid channel` | 频道 id 不合法 |
| `Segment not found` | 切片未知 |
| `Rate Limited` | 限流 |
| `Upstream Error` | 上游失败 |
| `Master M3U8` | 主清单相关 |

> 面板提示文案（登录失败、保存成功）与目标**不逐字对齐**：目标面板是闭源前端，我们自研 UI（`spec/05` 纪律：不复制目标前端代码与文案）。

## 8. 已知差异（登记在案，不重复实现）

| 差异 | 处理 | 依据 |
| --- | --- | --- |
| `<P>` 生成算法未解 | 自研 `sha256("{gid}:{cid}")[:16]` + 校验；客户端从清单取值 | F-017 |
| `rewrite` + MPD 正文损坏 | 不复刻该缺陷 | D-005 |
| `/epg/api/*` 仅 v0.8.8 | P0 不实现 | S-GAP-02 |
| 缓存上限未生效（目标缺陷） | P0 不实现上限 | F-015 |
| `/status`、`/status/metrics` 非路由 | 保持 `302` 假路由 | S-EP-08 |

## 9. M1 实施清单

> 状态：**已实现**（`src/http/routes/groups.rs`、`src/playlist.rs`、`src/auth/play_token.rs`、`frontend/src/groups.tsx`；验证见 `scripts/smoke-m1.sh` 与 `PROGRESS.md` §3c）。差异：无 `x-tvg-url`（EPG 属 P1）。

1. 分组 CRUD + 频道 CRUD（§6.2），改后原子写盘并替换内存快照。
2. 订阅渲染（§5）：总订阅与分组订阅、`access` 过滤、Token 附加、`m3u_header_lines`、`x-tvg-url` 占位（EPG 在 P1 前恒不输出）。
3. 分组导出（§6.2 两个 `export`）。
4. 冒烟脚本扩展：分组/频道增删改、订阅形状断言、`access` 矩阵断言。
