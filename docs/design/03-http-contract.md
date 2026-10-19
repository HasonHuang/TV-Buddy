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
| GET | `/{gid}/{cid}/video/index.txt` | 无 | `200` `text/plain` 媒体清单 | M2 | F-004 |
| GET | `/{gid}/{cid}/<P>_<S>.<ext>` | 无 | `200` 切片 | M2 | S-DIS-04 |
| GET | `/{gid}/{cid}/timeshift/{filename}` | 按 token 开关 | `200` 本地回看 | P1 | F-014 |
| GET | `/{gid}/{cid}/dash-timeshift/{track}/{...}` | 按 token 开关 | `200` DASH 回看 | P2 | S-EP-16 |
| GET | `/epg` | 无 | `200` `text/html`；EPG 关 → `404` | P1 | S-EP-05 |
| GET | `/epg.xml`、`/epg.xml.gz` | 无 | `200` `application/xml` / `application/gzip` | P1 | S-EP-05 |
| GET | `/epg/api/{channels,programmes/:channel,search}` | 无 | `200` JSON | P1 | S-EP-16 |
| GET | `/logo` | 无 | `200` `text/html` | P2 | S-EP-06 |
| GET | `/logo/<file>` | 无 | `200` 图片 + `public, max-age=86400` / `404` | P2 | F-011 §6 |
| GET | `/logo/api/list` | 无 | `200` JSON | P2 | F-011 |
| GET | `/unban` | 无 | `200` `text/html` | P1 | S-EP-07 |
| GET | `/unban/whoami`、`/whoami` | 无 | `200` JSON（客户端 IP） | P1 | S-EP-15 |
| GET | `/health` | 无 | `200` `ok`（`text/plain`） | ✅ M0 | S-EP-10 |
| GET | `/panel`、`/panel/`、`/panel/epg` | 无 | `200` `text/html` | ✅ M0 | S-EP-01 |
| GET | `/__assets/*path` | 无 | `200` 静态资源 / `404` | ✅ M0 | S-EP-16 |

## 4. 播放契约（proxy）

链路（F-004、F-006）：

```text
GET /{gid}/{cid}.m3u8
  → 200 清单；cache-control: public, max-age=5, s-maxage=5
     内容含媒体清单地址 /{gid}/{cid}/video/index.txt
GET /{gid}/{cid}/video/index.txt
  → 200 text/plain；切片地址 /{gid}/{cid}/<16hex>_<32hex>.<ext>
GET /{gid}/{cid}/<P>_<S>.<ext>
  → 200 切片；忽略 Range（返回全量）
     首次写入：public, max-age=300；命中缓存：private, max-age=300
```

| 项 | 契约 | 依据 |
| --- | --- | --- |
| 扩展名 | `format=hls` → `.m3u8`；`format=mpd` → `.mpd` | S-URL-01 |
| `<S>` | `sha256(上游切片绝对 URL)` 前 32 hex | S-DIS-04 |
| `<P>` | 16 hex，仅依赖 `(gid,cid)`；错值 → `302` | S-DIS-17、F-017 |
| 扩展名轮换 | 取 `[cache.disguise].formats`，每频道独立序列 | S-DIS-04 |
| 切片类型 | 按伪装后缀给 `Content-Type` | S-DIS-13 |
| 载荷 | 不改字节 | S-DIS-02 |
| Token | 入口按开关校验；**切片不校验** | F-007 |
| `Range` | 忽略 | S-DIS-07 |
| remux 预热 | `503` + `Retry-After: 2`，体 `remux warming up` | S-MODE-16 |
| dash 未就绪 | `503 timeout waiting for master playlist` | S-MODE-15 |

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
| GET | `/panel/api/groups/{gid}/export` | — | 单个分组对象 | F-013 §1 |

写入语义：分组与频道改动**立即重写** `config.toml`；`PUT /config` 是**全量替换**（F-013 §6）。

### 6.3 后续里程碑（登记存在性）

| 组 | 路径 | 阶段 |
| --- | --- | --- |
| 缓存 | `POST /panel/api/cache/flush`、`GET /panel/api/cache/stats` | M2 |
| 素材 | `/panel/api/assets/{list,convert,remove,ffmpeg-status}` | P2 |
| 录制 | `/panel/api/schedule`、`/schedule/from-epg`、`/schedule/{idx}`、`/schedule/{idx}/stop`、`/schedule/download/{filename}` | P1 |
| 推流 | `/panel/api/livestream`、`/livestream/{gid}/{cid}/{start,stop}` | P1 |
| DASH | `/panel/api/dash/{status,scan-url,tracks/:gid/:cid}`、`/dash-timeshift/status` | P2 |
| EPG | `/panel/api/epg/{status,refresh,regenerate,mappings,programmes,scripts,match-report,mapping,extra,script,script-delete,cron-preview}`、`/epg/{suggest,search,viewer-channels,source-channels,tz-preview}` | P1 |
| 安全 | `/panel/api/security/{token,token/rotate,ip/stats,ip/ban,ip/unban,ip/unban-batch}` | P1 |
| 指标 | `/panel/api/{stats,logs,health}` | M3 |
| 备份 | `/panel/api/{backup,restore}` | P2 |
| 导入 | `POST /panel/api/groups/import/m3u`、`/groups/{import,import/force}` | P2 |
| 订阅聚合 | `POST /panel/api/subs/{groups,preview}` | P2 |
| 翻译 | `/panel/api/epg/translate/{run,status,clear,probe,models,mtran/test}`、`/subtitle/translate/clear` | P2 |
| 分享 | `GET /api/share/pull?key=`（仅 GET/HEAD） | P2 |
| 解析脚本 | `/panel/api/resolver/{scripts,script-delete}` | P2 |

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
