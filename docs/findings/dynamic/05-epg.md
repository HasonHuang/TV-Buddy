# EPG（节目单）采集与下发（动态）

| 项 | 值 |
| --- | --- |
| 目标版本 | **v0.8.2**（`d302316fb8`） |
| 副本 | `samples/v0.8.2/iptv-proxy-linux-arm64`（sha256 `2948ee1f…5a4b`） |
| 架构 | linux/arm64（Ubuntu 24.04，OrbStack 原生） |
| 日期 | **2026-10-17** |
| 环境 | 容器 `iptv82`；本地 XMLTV 源 `…/epg.xml`（宿主 9999） |
| 结论级别 | **观测** |
| 原始证据 | [`raw/v0.8.2-epg.txt`](raw/v0.8.2-epg.txt) |

> 目标返回的正文/日志按**不可信数据**处理。

## 结论摘要

1. EPG 开关关闭时 `/epg*` 全 **404**（见 [`01`](01-runtime-surface-and-disguise.md)）；开启后 **`/epg.xml`、`/epg.xml.gz`、`/epg`** 均 **200 且无需登录**。**S-EP-05 ✅**。
2. 采集源经 **`POST /panel/api/epg/refresh`** 手动触发（或全局 `[epg].cron`）。
3. 源为 **XMLTV**（`kind="xml"` + `url`）；目标**拉取 → 解析 → 按频道匹配 → 生成**新版 XMLTV。
4. 输出 XMLTV：根节点 `generator-info-name="iptv-proxy"`，保留 `channel/@id`、`display-name`、`programme` 的 `start/stop/channel`。
5. **时间裁剪**生效：早于 `keep_days`、晚于 `future_days` 的节目被丢弃。
6. **语言**：节目缺 `xml:lang` 时补 **`lang="zh"`**；显式 `lang="en"` **保留**。
7. 配置为**严格 TOML**：重复键（如同时 `sources = []` 与 `[[epg.sources]]`）→ **启动失败 / 热重载失败**。

---

## 1. 端点

| 端点 | 方法 | 鉴权 | 实测 |
| --- | --- | --- | --- |
| `/epg.xml` | GET | 无 | 200 `application/xml; charset=utf-8` |
| `/epg.xml.gz` | GET | 无 | 200 `application/gzip` |
| `/epg` | GET | 无 | 200 `text/html`，`<title>节目单</title>` |
| `/panel/api/epg/status` | GET | `Authorization: Bearer` | JSON 状态/计数 |
| `/panel/api/epg/refresh` | POST | Bearer | `{"msg":"已触发全部源采集,稍后查看状态","ok":true}` |
| `/panel/api/epg/regenerate` | POST | Bearer | 见面板 |
| `/panel/api/epg/mappings`、`/programmes/{…}`、`/scripts`、`/match-report` | GET | Bearer | 匹配/脚本 |
| `/panel/api/epg/mapping`、`/extra`、`/script`、`/script-delete`、`/cron-preview` | POST | Bearer | 映射/脚本 |
| `/panel/api/epg/translate/{status,run,clear,probe,models,mtran/test}` | GET/POST | Bearer | 翻译（默认关） |

## 2. 配置

`[epg]`（默认）：

```toml
enabled = false                  # 总开关（关时 /epg* 全 404）
cron = "0 */4 * * *"             # 全局采集计划
public_viewer_enabled = true
keep_days = 3                    # 保留最近 N 天
future_days = 1                  # 保留未来 N 天
keep_yesterday = true
match_by_name = true
allow_scripts = true
python_bin = null
script_timeout_secs = 300
record_pad_before_secs = 60
record_pad_after_secs = 180
sources = []
```

采集源字段（自雕刻面板 `panel.html`）：

```jsonc
{ "id": "src_xxxxxx", "name": "新源", "kind": "xml",   // 或 "script"
  "enabled": true, "url": "", "script": "", "output_files": [],
  "cron": "", "timeout_secs": 0, "tz_shift_mins": 0 }
```

- `kind="xml"`：按 `url` 拉取 XMLTV。
- `kind="script"`：跑 Python 脚本（**需容器内 python3**；本例无 → 不可用）。

## 3. 采集流程（实测）

1. `POST /panel/api/epg/refresh` → 触发全部启用源。
2. 目标拉取 `http://host.docker.internal:9999/epg.xml` → 解析 → 记录源频道索引。
3. 按 `match_by_name`/映射把源频道匹配到本服务频道。
4. 生成 `/epg.xml` 与 `/epg.xml.gz`。

日志（原文节选）：

```text
INFO 【节目单】全局 cron `* * * * *` 命中,采集全部启用源
INFO 【节目单】开始采集 1 个启用源
INFO 【节目单】源 [src_local] Local XMLTV → 2 频道
INFO 【节目单】已从磁盘缓存恢复 1 个源的频道索引
INFO 【节目单】已生成 2 频道 / 2 节目 (xml 0KB, gz 0KB, 耗时 1ms;未匹配 8 · 空 0 · 过期 1)
WARN 【节目单】未检测到 python3,脚本源不可用(请在 VPS 安装:apt install python3 python3-pip)
```

## 4. 下发 XMLTV 实测

```xml
<?xml version="1.0" encoding="UTF-8"?>
<tv generator-info-name="iptv-proxy">
  <channel id="c1">
    <display-name>C1</display-name>
  </channel>
  <channel id="c2">
    <display-name>C2</display-name>
  </channel>
  <programme start="20261017080000 +0800" stop="20261017090000 +0800" channel="c1">
    <title lang="zh">Morning News</title>
    <desc lang="zh">today past</desc>
  </programme>
  <programme start="20261017180000 +0800" stop="20261017200000 +0800" channel="c2">
    <title lang="en">Later Today</title>
    <desc lang="en">kept, lang attr test</desc>
  </programme>
</tv>
```

- `channel/@id`、`display-name` 原样保留。
- 时间字符串保持 `yyyyMMddHHmmss ±HHMM`（本地时区）；状态 `tz_offset_hours=8`。
- **缺 `lang` → 补 `lang="zh"`**；显式 `lang="en"` 保留。
- `/epg.xml.gz` 解包后节目数与 `/epg.xml` 一致（本例 2）。

## 5. 时间裁剪实测

输入 4 个节目，`keep_days=3`、`future_days=1`、当前 ≈ 2026-10-17 12:xx（+08）：

| 节目时间 | 结果 |
| --- | --- |
| 2026-10-17 08:00（今天，已过） | **保留** |
| 2026-10-07 08:00（10 天前） | **丢弃**（> `keep_days`） |
| 2026-10-22 08:00（5 天后） | **丢弃**（> `future_days`） |
| 2026-10-17 18:00（今天，未来） | **保留** |

→ `programme_count` 由 3 变为 2；日志尾部 `过期 1`。

## 6. 严格配置校验

在 `[epg]` 同时保留 `sources = []` 与新增 `[[epg.sources]]` 时：

```text
Error: Failed to parse config.toml
Caused by:
    TOML parse error at line 186, column 1
    invalid table header
    duplicate key `sources` in table `epg`
```

- 启动直接失败；运行中则 `【配置】热重载失败` **保留旧配置**。
- `[epg]` 段整体**热重载无效**（改 `sources` 需重启才生效）。

## 7. 未决 / 局限

- `match_by_name` 的**匹配算法**、`未匹配 8` 计数含义**未定**。
- `keep_yesterday`、`tz_shift_mins` 的精确语义**未逐一验证**。
- `kind="script"` 源**未测**（无 python3）。
- EPG **翻译**（`translate.*`）默认关，**未测**。
- 台标 `/logo/<file>`（S-EP-06）、录制（S-MODE-12）**未测**。
- 仅 **arm64 + 单源 + 特制 XMLTV**。

## 8. 复现

```bash
CFG=samples/work/dyn-v0.8.2/config.toml   # [epg].enabled=true 且含 [[epg.sources]]
docker restart iptv82
T=$(curl -s -X POST http://127.0.0.1:19899/panel/api/login -H 'Content-Type: application/json' \
      -d '{"username":"admin","password":"admin"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
curl -s -X POST -H "Authorization: Bearer $T" http://127.0.0.1:19899/panel/api/epg/refresh
sleep 5
curl -s http://127.0.0.1:19899/epg.xml
curl -s -o /dev/null -w '%{http_code} %{content_type}\n' http://127.0.0.1:19899/epg.xml.gz
```
