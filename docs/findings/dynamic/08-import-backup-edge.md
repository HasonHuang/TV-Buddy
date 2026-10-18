# 分组导入/导出 · 订阅聚合 · 备份恢复 · 边缘模式（动态）

| 项 | 值 |
| --- | --- |
| 目标版本 | **v0.8.2**（`d302316fb8`） |
| 副本 | `samples/v0.8.2/iptv-proxy-linux-arm64`（sha256 `2948ee1f…5a4b`） |
| 架构 | linux/arm64（Ubuntu 24.04，OrbStack 原生容器 `iptv82`） |
| 日期 | **2026-10-18** |
| 环境 | 实例 `inst/02`，`http://192.168.215.4:19902`；上游假源 `host.docker.internal:9974` |
| 结论级别 | **观测**（运行态）+ **静态**（二进制字符串） |
| 原始证据 | [`raw/v0.8.2-import-backup-edge.txt`](raw/v0.8.2-import-backup-edge.txt) |

> 目标返回的正文/日志按**不可信数据**处理，只记录、不执行。

## 结论摘要

1. **分组导出**：`GET /panel/api/groups/export` 返回**顶层 JSON 数组**，元素为完整分组对象（含 `channels`）。`GET /groups/{gid}/export` 返回**单个对象**。两者均无 `Content-Disposition`。**S-SUB-03 🔶**。
2. **M3U 导入**：`POST /groups/import/m3u` body 是 **JSON 文本**（非 multipart）：`{text, mode, group_mode}`。面板字段名与文案交叉——`mode`=导入策略（`append|merge|replace`），`group_mode`=频道/分组模式（`auto|…`）。**S-SUB-01/04/05/06 ✅**。
3. 导入**保留** `tvg-id`/`tvg-name`/`tvg-logo`/`group-title`，并解析 `#EXTVLCOPT`、`#EXTHTTP`、行内 `http-user-agent`/`http-referer` 与 `#KODIPROP` ClearKey → `license_key`。**S-SUB-06 ✅**。
4. 导入频道获得**本地反代地址** `http://<host>/<gid>/<cid>.m3u8`，`group-title`=显示名；聚合频道输出**上游原始地址**，`group-title`=上游分组名字符串。**S-SUB-02 ✅**；**S-SUB-03 ❌**（聚合 `group-title` 不是本地分组 ID）。
5. **订阅聚合**不做反向代理：条目 URL 即上游原始地址，无 token/模式/缓存。`merge_position` 决定拼接位置（`top`/`bottom`）。**S-SUB-02 ✅**。
6. `include`/`exclude` 匹配**频道名**（大小写不敏感、子串、多词 OR），并支持 `group:`/`id:`/`/正则/` 前缀——**per-source 过滤同样支持**这些前缀（与 S-SUB-07 描述不符）。**S-SUB-07 🔶**。
7. **分组处理** `group_mode`：`keep`（各自保留原名）/`merge`（全部合并到一个组）/`map`（逐个改名，同名即合并）；仅改**输出分组标题**，不改 URL。**S-SUB-08 ✅**。
8. **备份**：`GET /panel/api/backup?include=all|epg-scripts|resolver` → `application/gzip`，文件名带时间戳；包内 `iptv-proxy-backup/` 下为 `config.toml`、`epg/epg_map.json`、`epg/scripts/*.py`、`resolvers/*.py`、`logos/*`、`manifest.json`。**只识别这三个 scope，其它值回退 `all`**。**S-D25 ✅（新）**。
9. **恢复**：`POST /panel/api/restore`（`Content-Type: application/gzip`，body 为 tar.gz 原文）→ `{"ok":true,"config_reloaded":…,"report":{…}}`；原 `config.toml` 另存 `config.toml.bak`。实测可回滚配置。**S-D25 ✅**。
10. **边缘模式** `edge_mode`∈{`off`,`cloudflare`,`proxy`}。探测头名 **`X-IPP-Edge-Secret`**。`edge_reject=true` 时，**带转发头**（`X-Forwarded-For`/`X-Real-IP`/`CF-Connecting-IP`）而密钥不符的请求 → **403**；无转发头的直连请求放行。`edge_reject=false` 为观察模式。**S-D27 ✅（新）**。

## 1. 分组导出（S-SUB-03 🔶）

| 端点 | 方法 | 鉴权 | 实测 |
| --- | --- | --- | --- |
| `/panel/api/groups/export` | GET | Bearer | 200 `application/json`，顶层数组，9 组 |
| `/panel/api/groups/{gid}/export` | GET | Bearer | 200 `application/json`，单对象 |

- 分组字段：`id,name,mode,access,format,remux_container,mpd_keep_seconds,proxy,user_agent,headers,epg_url,redirect_ttl,redirect_mode,channels`；频道字段：`id,name,url,tvg_id,tvg_name,logo,proxy,user_agent,remux_container,headers,license_key,selected_tracks,dash_mode,livestream,timeshift_*,dash_timeshift_*,resolver_script,resolver_arg,resolver_catchup,catchup_days,resolver_ttl,url_pool_size`。**无 `Content-Disposition`**（文件名由面板 JS 定为 `groups_export.json` / `group_<gid>.json`）；缺省值 `null`/`[]`/`0`/`false`，字段齐全；**不含 `segment_cache_max`、`retain_count`**（面板建组 payload 有，导出无）。

## 2. M3U 导入（S-SUB-01/04/05/06 ✅）

### 2.1 请求

- 请求：`POST /panel/api/groups/import/m3u`，body=`{"text":"#EXTM3U…","mode":"append","group_mode":"auto"}`。
- Body 是 **JSON 文本**（非 multipart；面板由 `File.text()` 读入后填入 `text`）；**字段名与文案交叉**：`#m3u-mode`→导入策略 `mode`、`#m3u-group-mode`→频道模式 `group_mode`。
- `mode`：`append`（默认，同名分组跳过）| `merge`（同名保留，仅新增）| `replace`（同名整体替换）；`group_mode`：`auto`（默认）| `redirect` | `rewrite` | `proxy` | `dash` | `passthrough`。
- 响应：`{"ok":true,"parsed_groups":N,"parsed_channels":N,"added_groups":N,"merged_channels":N,"replaced_groups":N,"skipped_groups":N,"group_mode":"…"}`。

### 2.2 落库规则

| 项 | 行为 |
| --- | --- |
| 分组 id | `group-title` 去掉非 ASCII 字符。`导入组A`→`A`、`导入组B`→`B`；纯中文→回退 `g-<6hex>` |
| 频道 id | 有 `tvg-id` 用 `tvg-id`；否则随机 `g-<6hex>`（**每次导入都不同，故无 tvg-id 的频道不幂等**） |
| 无 `group-title` | 落到分组 `Default`（id=`Default`） |
| `group_mode=auto` / 显式 `group_mode=proxy` 等 | `auto`：分组内 `.mpd` 源 → 该组 `mode=dash`、`format=mpd`，其余 → `mode=rewrite`、`format=hls`；显式值：新建分组采用该 `mode`，被跳过的同名分组保持原 `mode` |

### 2.3 元数据与头解析（S-SUB-06 ✅）

| 输入 | 落地字段 |
| --- | --- |
| `tvg-id` / `tvg-name` / `tvg-logo` | `tvg_id` / `tvg_name` / `logo` |
| `#EXTVLCOPT:http-user-agent=…` / 行内 `http-user-agent=` | `user_agent` |
| `#EXTHTTP:{"Referer":…,"Origin":…}` / 行内 `http-referer=` | `headers`（`[{name,value}]`，**按名排序**） |
| `#KODIPROP:…license_key=<kid:key>` | `license_key`（ClearKey） |

### 2.4 导入策略（S-SUB-05 ✅）

| `mode` | 行为 | 证据 |
| --- | --- | --- |
| `append` | 同名分组**跳过**，新分组追加 | `skipped_groups:1` |
| `merge` | 同名分组保留，仅**新增** id 不存在的频道；已存在 id **不更新**；幂等 | `merged_channels:1`，二次导入 `0` |
| `replace` | 同名分组**整体替换**（旧频道丢弃） | `replaced_groups:3` |

### 2.5 错误分支

无 `#EXTINF` → 400 `{"error":"M3U 中没有解析到任何频道(请检查格式)"}`；缺 `text` → 400 `{"error":"M3U 文本为空"}`。

## 3. 手工建组 / 删除

| 端点 | 方法 | 响应 |
| --- | --- | --- |
| `/panel/api/groups` | POST | `{"ok":true}`；同 id → **409** `{"error":"Group id already exists"}` |
| `/panel/api/groups/{gid}` | PUT | `{"ok":true}`；**局部更新**（只改传入字段） |
| `/panel/api/groups/{gid}` | DELETE | `{"ok":true}`；不存在 → 404 `Group not found` |
| `/panel/api/groups/{gid}/channels/{cid}` | DELETE | `{"ok":true}`；不存在 → 404 `Channel not found` |
| `/panel/api/groups/import` 与 `/panel/api/groups/import/force` | POST | `{"added":N,"skipped":N,"ok":true}` / `{"added":N,"replaced":N,"skipped":[],"ok":true}` |

- 以上建/删操作**立即重写**宿主 `config.toml`（`[[groups]]` 同步增删）；**`PUT /panel/api/config` 是全量替换**：用不含某分组的完整 config PUT 后，该分组消失。

## 4. 订阅聚合（S-SUB-02 ✅ / S-SUB-03 ❌ / S-SUB-08 ✅）

### 4.1 结构

- `playlist.sources[]` 字段：`id,name,url,enabled,ttl_secs,group_map,group_mode(keep|merge|map),merge_group,unlisted_groups(keep|drop),include[],exclude[]`；`playlist.merge_position="top|bottom|…"`（聚合条目插入位置）、`merge_ttl_secs`、`global_exclude[]`。
- `POST /panel/api/subs/groups {url,ttl_secs,force}`：**只读**读取上游分组，无落库副作用。`POST /panel/api/subs/preview {source}`：试算，返回 `kept`/`sample`/`trace`；**`source` 必填 `id`**（否则 422）。
- `GET /panel/api/subs/status`：字段 `id,name,enabled,error,channels,last_ok,last_try,ttl`（`ttl` 秒，未设时 3600）；未抓取前 `channels:0`、`last_ok:0`。

### 4.2 输出

```text
#EXTINF:-1 tvg-id="cn.bj" group-title="央视",CCTV-1 综合
http://upstream.example:9000/cctv1/index.m3u8

```

- **URL = 上游原始地址**（无 `/{gid}/{cid}` 反代、无 token、无模式/缓存）；**`group-title` = 上游 `group-title` 字符串**（`keep` 模式）。**S-SUB-02 ✅**。对照本地/导入频道：URL=`http://<host>/<gid>/<cid>.m3u8`，`group-title`=显示名 → **S-SUB-03 的「聚合频道 `group-title`=<分组ID>」与本观测不符**：本版输出上游分组名，不是本地分组 ID。

### 4.3 分组处理（S-SUB-08 ✅）

| `group_mode` | 输出分组标题 | 预览结果（7 频道） |
| --- | --- | --- |
| `keep` | 各自保留上游原名 | `out_groups=[央视(2),电影(2),上海(1),测试(1),购物(1)]` |
| `merge` + `merge_group=聚合` | 全部合并到一个组 | `out_groups=[聚合(7)]` |
| `map` + `group_map={央视:CN,电影:MOV}` | 逐个改名（同名=合并） | `out_groups=[CN(2),MOV(2)]` |

- `unlisted_groups=drop` + 空 `group_map` → `kept=0`（分组白名单语义）；`unlisted_groups=keep` → 全保留。任何 `group_mode` 下 URL 仍是上游原始地址 → **只改输出分组标题**。

## 5. include / exclude（S-SUB-07 🔶）

- 匹配**频道名**（不区分大小写、子串、多词 OR）；`include=["央视"]` 不命中（无频道名含「央视」）→ `kept=0`。
- 前缀 `group:<名>` / `id:<tvg-id>` / `/正则/` **在 per-source include 与 exclude 均有效**（`group:` 亦为子串）；`global_exclude`（config 级）同语法，全源生效。管道顺序：`total → after_group → after_global_exclude → after_include → after_exclude`。**与 S-SUB-07 的差异**：spec 称前缀**仅用于全局排除**；实测 per-source include/exclude 同样支持。

| 规则 | 结果（7 频道源） |
| --- | --- |
| `include ["CCTV"]` / `["cctv"]` | 2 |
| `include ["电影"]` | 2 |
| `include ["group:央视"]` / `["group:央"]` | 2 |
| `include ["id:mov.1"]` | 1 |
| `include ["/电影/"]` | 2 |
| `exclude ["/CCTV-./"]` | 5 |
| `global_exclude ["group:电影"]` | 5 |

## 6. 备份 / 恢复（S-D25 ✅ 新）

### 6.1 下载

| 端点 | 方法 | 响应头 |
| --- | --- | --- |
| `/panel/api/backup?include=all` | GET | 200 `application/gzip`，`Content-Disposition: attachment; filename="iptv-proxy-backup-YYYYMMDD-HHMM.tar.gz"` |
| `?include=epg-scripts` / `?include=resolver` | GET | `filename="iptv-proxy-epg-scripts-….tar.gz"` / `"iptv-proxy-resolvers-….tar.gz"` |

- 未鉴权 → **401**。**只识别 `all` / `epg-scripts` / `resolver`**；其它值（如 `epg`、`bogus`）**回退为 `all`**（文件名无 scope 后缀）。

### 6.2 包结构

| 路径（`iptv-proxy-backup/` 下） | scope |
| --- | --- |
| `config.toml` | all |
| `epg/epg_map.json` | all（EPG 映射） |
| `epg/scripts/<name>.py` | all + epg-scripts |
| `resolvers/<name>.py` | all + resolver |
| `logos/<file>` | all（台标库） |
| `manifest.json` | — |

```json

{ "kind":"iptv-proxy-backup", "version":1, "app_version":"0.8.2", "created_at":"2026-10-18 19:21:21", "scope":"all", "files":5 }

```

- `config.toml` 与实例 `config.toml` **sha1 完全一致**。**生成的 `epg.xml`/`epg.xml.gz` 不入包**；`epg_map.json` 入包。无内容时 `files` 相应减少（空脚本时 `files:0`）。

### 6.3 恢复

```text

POST /panel/api/restore    Content-Type: application/gzip    body = tar.gz 原始字节
→ 200 {"config_reloaded":true,"ok":true,
        "report":{"config":true,"epg_map":true,"epg_scripts":1,"logos":1,"resolvers":1,"skipped":[]}}

```

- 未鉴权 → 401 `{"error":"Missing token"}`；非 gzip body → **400** `{"error":"恢复失败: invalid gzip header"}`。恢复 `all` 时 `config_reloaded:true`（热载）；恢复脚本/台标包时 `config_reloaded:false`。
- **原 `config.toml` 自动另存为 `config.toml.bak`**（内容=恢复前配置，已验证含改前 marker 值）；改 `home_url`/`log_level` 后恢复 `all` 包 → 两值回到恢复前状态。

## 7. 边缘模式（S-D27 ✅ 新）

### 7.1 配置

| 键 | 取值 | 语义（实测/字符串） |
| --- | --- | --- |
| `edge_mode` | `off` / `cloudflare` / `proxy` | 来源校验模式；`off` 不校验 |
| `edge_secret` | 字符串 | 边缘密钥；CF Transform Rule 注入头 `X-IPP-Edge-Secret` 时必须与之一致 |
| `edge_reject` | bool | `true` 拒绝不可信来源；`false`=观察模式（放行 + 周期日志） |
| `edge_refresh` | bool | 启用 Cloudflare 网段刷新（启动时拉取 `https://www.cloudflare.com/ips-v4\|ips-v6`，实测 22 条；拉空则保留现有/内嵌列表） |

- 面板 v0.8.2 **无 edge 设置 UI**（`panel.html` 仅一处 CSS 注释含 “edge”），只能改文件/API；`[server]` 的 `edge_*` **热重载即时生效**（无需重启）。启动日志：`【启动】边缘来源校验模式: Off(拒绝非边缘来源=true)` / `… Cloudflare(…)`。

### 7.2 探测头名

**`X-IPP-Edge-Secret`**（二进制字符串 `x-ipp-edge-secret` / `X-IPP-Edge-Secret`）；转发头 `X-Forwarded-For`、`X-Real-IP`、`CF-Connecting-IP`。

### 7.3 `edge_mode="cloudflare"` + `edge_reject=true` 矩阵（GET `/playlist.m3u`）

| 请求 | 结果 |
| --- | --- |
| 无任何头 / 仅 `X-IPP-Edge-Secret: <正确>` / 仅 `wrong` | **200** |
| `X-Forwarded-For: 1.2.3.4` | **403** `text/plain` `Forbidden` |
| `X-Real-IP: 1.2.3.4` / `CF-Connecting-IP: 1.2.3.4` / `CF-Connecting-IP: 173.245.48.5`（CF 网段） | **403** |
| 转发头 + 正确密钥 / 错误密钥 | 200 / 403 |

判定模型（实测）：① 带转发头才进入边缘校验；② 仅当 `X-IPP-Edge-Secret` 等于 `edge_secret` 才放行，否则 403（`edge_reject=true`）；③ **无转发头**的直连请求一律放行（CF 网段命中与否不改变结论）；④ `edge_reject=false` 放行不可信请求，按周期写 `【边缘·观察模式】…` 日志。

### 7.4 `edge_mode="proxy"`

`trust_proxy=[…]` 命中时采信转发头确定客户端 IP。实测各请求**均 200**（不拦截；`edge_reject` 无效）。`trust_proxy` 为空时日志警告：管控退回按 TCP 源 IP 记账。

### 7.5 `edge_mode="off"`

不校验：任意转发头 / 密钥组合均 200。

## 8. 未决 / 局限

- **聚合 `group-title` 语义**（S-SUB-03）与 spec 冲突，以本观测为准（上游分组名，非本地分组 ID）；上游分组名与本地组名同名时的行为**未测**。
- 订阅聚合的**缓存**（`sub_url` 目录、`ttl`）命中/过期行为**未逐一验证**；`POST /subs/groups` 的分组顺序（同名计数排序）**不稳定**，排序规则未定；**导入 id 生成**中纯中文组名/无 tvg-id 频道的 `g-<6hex>` 已生成，但**保留 ID 冲突/预置 ID**（二进制含「跳过使用了保留 ID 的分组」）**未测**。
- 备份包**不含**录像、DASH 缓存、EPG 生成产物；`logos/*.orig`（动画原图）是否入包**未测**。
- 恢复的**合并/覆盖策略**（同名覆盖，未知多余文件是否清理）**未验证**；仅验证同名覆盖与回滚。
- **`edge_refresh` 的刷新周期**未定（仅见启动拉取一次 CF 网段）；`edge_mode` 判定中「无转发头一律放行」仅在**私有网段对端**下观测，公网对端行为**未测**。仅 **arm64 + 单实例 + 单上游源**验证。

## 9. 复现

```bash

B=http://192.168.215.4:19902
AUTH="Authorization: Bearer $(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')"
# D24 导出 / 导入
curl -s "$B/panel/api/groups/export" -H "$AUTH" | python3 -m json.tool
curl -s -X POST "$B/panel/api/groups/import/m3u" -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"text":"#EXTM3U\n#EXTINF:-1 tvg-id=\"x\" group-title=\"G\",CH\nhttp://o/x.m3u8","mode":"append","group_mode":"auto"}'
# D24 订阅聚合（宿主先起上游源端口 9974）
curl -s -X POST "$B/panel/api/subs/groups" -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"url":"http://host.docker.internal:9974/playlist.m3u","ttl_secs":0,"force":true}'
# 落库: GET /config -> 改 playlist.sources[] -> PUT /config；校验: curl -s "$B/playlist.m3u"
# D25 备份 / 恢复
curl -s -D - "$B/panel/api/backup?include=all" -H "$AUTH" -o bk.tar.gz && tar tzvf bk.tar.gz
curl -s -X POST "$B/panel/api/restore" -H "$AUTH" -H 'Content-Type: application/gzip' --data-binary @bk.tar.gz
# D27 边缘模式（config.toml 或 PUT /config）：edge_mode="cloudflare"; edge_secret="<secret>"; edge_reject=true
curl -s -o /dev/null -w '%{http_code}\n' "$B/playlist.m3u"                               # 200（无转发头）
curl -s -o /dev/null -w '%{http_code}\n' -H 'X-Forwarded-For: 1.2.3.4' "$B/playlist.m3u" # 403
curl -s -o /dev/null -w '%{http_code}\n' -H 'X-Forwarded-For: 1.2.3.4' \
  -H 'X-IPP-Edge-Secret: <secret>' "$B/playlist.m3u"                                      # 200
docker exec iptv82 sh -c 'grep -a 边缘 /app/inst/02/run.log | tail'

```

> 注：容器 `iptv82` 的实例由外部 harness 分批拉起（`inst/00…06`）。**不要随意 `docker restart`**——重启后 `inst/02` 需等待 harness 重新拉起（本次为手工补启动）。
