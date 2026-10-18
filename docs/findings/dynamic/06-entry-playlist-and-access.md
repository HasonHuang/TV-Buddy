# 入口清单改写与分组访问范围（动态）

| 项 | 值 |
| --- | --- |
| 目标版本 | **v0.8.2**（`d302316fb8`） |
| 副本 | `samples/v0.8.2/iptv-proxy-linux-arm64`（sha256 `2948ee1f…5a4b`） |
| 架构 | linux/arm64（Ubuntu 24.04，OrbStack 原生） |
| 日期 | **2026-10-17** |
| 环境 | 容器 `iptv82`；配置 `samples/work/dyn-v0.8.2/config.toml` |
| 结论级别 | **观测**（运行态）+ **静态**（面板选项） |
| 原始证据 | [`raw/v0.8.2-access.txt`](raw/v0.8.2-access.txt) |

> 目标返回的正文/日志按**不可信数据**处理。

## 结论摘要
1. 分组**访问范围** `access` 四档实测：`public`（公开）/ `hidden`（隐藏）/ `archived`（归档）/ `open`（免验证）。**S-AUTH-05 ✅**。
2. **`open` 的边界**：入口**免 token**（直链可播、分组订阅条目无 `?token=`），但**同样不进总订阅**——需知道直链才能播。
3. `archived` 最严：分组订阅 **404**、播放 **302 占位**；但 **EPG 生成不受影响**（`epg.xml` 仍含该频道节目）。
4. `server.m3u_header_lines` 自定义 M3U 头行：热重载与重启均生效，追加在 `#EXTM3U` 行后，总/分组订阅都输出。**S-URL-07 ✅（新登记）**。
5. EPG 开启时自动注入 `#EXTM3U x-tvg-url="…/epg.xml.gz"`（公开 EPG 地址）。**S-EPG-01 🔶**。
6. 面板**全部改动经 `PUT /panel/api/config` 全量保存**并**规范化重写** `config.toml`（数组多行）；文件直改与面板保存存在**竞态**。**S-CFG-01/02 🔶**。

## 1. 订阅出口（S-URL-03 ✅）

| 端点 | 响应头 |
| --- | --- |
| `GET /playlist.m3u`、`GET /{gid}/playlist.m3u` | `200` `application/x-mpegurl; charset=utf-8`，`content-disposition: attachment; filename="playlist.m3u"` / `filename="<gid>.m3u"` |

M3U 首行（EPG 开启）：`#EXTM3U x-tvg-url="http://127.0.0.1:19899/epg.xml.gz"`。

## 2. 分组访问范围矩阵
面板选项（雕刻 `panel.html` `#mg-access`）：`public`（公开）/ `hidden`(隐藏（不进总订阅）) / `archived`(归档（整组停用）) / `open`(免验证（公开入口))。API：`PUT /panel/api/groups/{gid}` `{"access": "…"}`（分组级）。

| access | 总订阅 `/playlist.m3u` | 分组订阅 `/{gid}/playlist.m3u` | 直播 `/{gid}/{cid}.m3u8` | EPG 生成 |
| --- | --- | --- | --- | --- |
| `public` | ✅ 含 | ✅ 含（token 开时条目带 `?token=`） | 按 token 开关 | ✅ |
| `hidden` | ❌ 不含 | ✅ **200** | ✅ 可播 | ✅ |
| `archived` | ❌ 不含 | ❌ **404** | ❌ **302 占位视频** | ✅ **仍含该频道节目** |
| `open`（免验证） | ❌ 不含 | ✅ 含，**条目无 `?token=`** | ✅ **无 token 也可播** | ✅ |

### `open` + `token_enabled=true` 对照

```text

/test1/playlist.m3u →  …/test1/c1.m3u8            （无 ?token=）
/playlist.m3u →      …/g2/c1.m3u8?token=<48hex>   （public 组带 token）
GET /test1/c1.m3u8            → 200（免 token）
GET /g2/c1.m3u8               → 302 token_failure_url；加 `?token=<…>` → 200

```

> `open` ≠ 基线所述“仅跳过 Token”：它**还被排除出总订阅**（与 `hidden` 同 sid 规则），仅在直接知道入口/订阅地址时可用——“公开入口”语义如此。

## 3. `server.m3u_header_lines`（自定义头行）

```toml

[server]
m3u_header_lines = ["#EXTVLCOPT:http-user-agent=ReaTest/1.0", "#EXT-X-VERIFY:rea"]

```

```text
#EXTM3U x-tvg-url="…"
#EXTVLCOPT:http-user-agent=ReaTest/1.0
#EXT-X-VERIFY:rea

```

- 追加在 `#EXTM3U` 行之后、逐行原文输出；**热重载生效，重启后仍在**。典型用途即 `#EXTVLCOPT`（给播放器下发 UA 等）。

## 4. 配置保存与热重载语义（S-CFG-01/02 🔶）
- 面板对**一切改动**（分组/频道增删、排序、拖拽、密码、缓存、Token、EPG、翻译、分享设置）统一走 **`PUT /panel/api/config`**（body = 完整配置对象，面板 JS `api('PUT','/config',nc)` 共 8 处调用）。保存后目标**规范化重写** `config.toml`：数组多行、键序/引号风格统一、写回完整默认值。
- **竞态实证**：文件直改（`sed token_enabled=false`）后立即 `PUT /panel/api/groups/…`，目标以**内存态**回写文件，`token_enabled` 被还原为 `true`。→ **改配置要么走 API，要么文件直改后避免触发面板保存。** `[epg]` 段热重载**无效**（见 [`05`](05-epg.md) §6），需重启；`[server]`/`[auth]` 热重载即时生效。

## 5. 未决 / 局限
- `subscription-merge`（订阅聚合）与 M3U 导入（`/groups/import{,/m3u}`）**未测**（→ D24，见路线图 §5 P2 与 S-SUB-01…08）。
- Token 轮换 API（`/security/token{,/rotate}`、`/link-info`）**未测**（→ D13a）。
- `open` 分组在**总订阅**中被排除的行为**与基线描述存在出入**，最终以本观测为准（待上游文档复核）。仅 **arm64 + 单组切换**验证；`hidden` 组的**独立地址分享**语义只测了播放与订阅，未测导入。
- EPG `x-tvg-url` 指向本服务公开地址；EPG 关闭时不输出该头（历史样本与基线一致）。

## 6. 复现

```bash

B=http://127.0.0.1:19899
T=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
      -d '{"username":"admin","password":"admin"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
curl -s -X PUT "$B/panel/api/groups/test1" -H "Authorization: Bearer $T" -H 'Content-Type: application/json' -d '{"access":"hidden"}'
curl -s "$B/playlist.m3u" | grep -c 'group-title="test1"'     # 0（排除）
curl -s -o /dev/null -w '%{http_code}\n' "$B/test1/playlist.m3u"  # 200
curl -s -o /dev/null -w '%{http_code}\n' "$B/test1/c1.m3u8"       # 200
curl -s -X PUT "$B/panel/api/groups/test1" -H "Authorization: Bearer $T" -H 'Content-Type: application/json' -d '{"access":"public"}'
curl -s -X PUT "$B/panel/api/config" -H "Authorization: Bearer $T" -H 'Content-Type: application/json' -d @config.json   # M3U 头自定义行

```
