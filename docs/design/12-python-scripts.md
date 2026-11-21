# 12. Python 脚本开发指南

> 设计版本 v1.0 ｜ 2026-11-15 ｜ 状态：✅ 已实施

TV Buddy 在两个位置运行 Python 脚本：

| 类型 | 用途 | 每个位置的数量 |
| --- | --- | --- |
| 上游解析脚本 | 把频道的上游地址解析成可播放的 HTTP(S) 地址 | 每个频道最多一个 |
| EPG 采集脚本 | 生成或抓取 XMLTV 节目单 | 每个 EPG 源最多一个 |

两类脚本**协议不同**。解析脚本读命令行参数，写标准输出。采集脚本不读参数，标准输出是 XMLTV。
两类脚本都不是 JSON 输入/输出协议。不要把其他项目的脚本直接搬进来。

---

## 1. 运行环境

### 1.1 安装 Python 3

服务端必须能执行 `python3`。运行时镜像**默认不安装** Python。
二进制安装与 Docker 安装都需要自行准备。

二进制安装（Debian / Ubuntu）：

```bash
sudo apt install -y python3
python3 --version
```

二进制安装（Alpine）：

```bash
sudo apk add --no-cache python3
```

Docker：推荐构建一个带 Python 的小镜像。

```dockerfile
FROM hason666/tv-buddy:latest
USER root
RUN apk add --no-cache python3
USER tvbuddy
```

Docker：临时安装只对当前容器有效。容器重建后丢失。

```bash
docker compose exec -u root tv-buddy apk add --no-cache python3
```

脚本只用标准库时，不需要安装 `pip`。需要第三方包时，在镜像构建阶段安装。
不要依赖交互式安装，服务进程没有终端。

### 1.2 解释器选择

| 脚本类型 | 解释器来源 | 能否修改 |
| --- | --- | --- |
| 解析脚本 | 固定命令名 `python3` | 不能 |
| EPG 采集脚本 | `[epg].python_bin`，留空时用 `python3` | 能 |

`python_bin` 可以是命令名，也可以是绝对路径。
路径必须对服务进程可见。Docker 里要写容器内路径，不是宿主机路径。

脚本的 shebang 不参与解释器选择。`#!/usr/bin/env python3` 只用于本地调试。

### 1.3 进程环境

- 工作目录：脚本所在目录。
- 标准输入：空（`/dev/null`）。
- 环境变量：继承服务进程。系统不注入自定义变量。
- 退出码：非 0 表示失败。
- 网络：可以访问外网。没有额外的网络隔离。

脚本可以读取服务进程的环境变量。不要在脚本里输出环境变量。
日志可能被其他管理员看到。

---

## 2. 脚本存放位置

### 2.1 目录

脚本只能放在缓存目录下的两个固定目录：

| 类型 | 目录 | Docker 中的典型路径 |
| --- | --- | --- |
| 解析脚本 | `<cache.disk_path>/resolvers/` | `/data/cache/resolvers/` |
| EPG 采集脚本 | `<cache.disk_path>/epg/scripts/` | `/data/cache/epg/scripts/` |

`cache.disk_path` 默认是 `./cache`，相对服务进程的工作目录解析。
二进制安装的默认工作目录是 `/opt/tv-buddy`。Docker 的工作目录是 `/data`。

两类脚本的工作目录都是自己的目录：

- 解析脚本的 cwd = `<cache>/resolvers/`。
- 采集脚本的 cwd = `<cache>/epg/scripts/`。

脚本里的相对路径按这个目录解析。

### 2.2 文件名规则

- 只接受 `.py` 文件。
- 文件名不能为空，不能以 `.` 开头，不能包含 `..`。
- 路径段会被剥离。`a/b.py` 保存为 `b.py`。
- 文字、数字、点、下划线、连字符以外的字符会替换成 `_`。
- 同名文件直接覆盖。覆盖后不能恢复旧内容。

解析脚本字段可以写 `foo` 或 `foo.py`。后端会补 `.py`。
EPG 源的 `script` 字段建议写完整文件名，例如 `foo.py`。

### 2.3 上传与管理入口

面板在选择脚本的位置提供「管理」按钮：

| 页面 | 位置 |
| --- | --- |
| 分组管理 → 频道 → 地址来源「解析脚本」 | 脚本下拉框旁的「管理」 |
| 节目指南 → EPG 源 → 类型「脚本源」 | 脚本下拉框旁的「管理」 |
| 节目指南 → EPG 源列表 | 脚本源行的「脚本」按钮 |
| 概览 → 系统状态 | 「解析脚本」卡片 |
| 节目指南 → 高级 | 「采集脚本」卡片 |

管理面板支持：查看列表、从本机读取 `.py`、新建、同名覆盖、删除。
管理面板不读取旧正文。覆盖前请先备份原文件。
管理弹窗内嵌一份简明开发指南。本文件是完整版。

也可以用 API 上传。见 §8。

### 2.4 备份

备份包包含两类脚本：

- `epg/scripts/*.py`
- `resolvers/*.py`

备份范围用 `GET /panel/api/backup?include=all|epg-scripts|resolver`。
恢复脚本时不会执行脚本。恢复后按恢复的配置运行。

---

## 3. 上游解析脚本

### 3.1 用途

频道可以配置一个解析脚本。播放请求到来时，服务先运行脚本，再用脚本输出的地址抓流。

适合这些场景：

- 上游地址带短期 token。
- 地址需要登录或签名。
- 地址来自 API，不是固定 URL。

### 3.2 调用契约

服务执行这条命令：

```text
python3 <cache>/resolvers/<name>.py <resolver_arg>
```

| 项 | 规则 |
| --- | --- |
| 参数 | `sys.argv[1]` 是频道表单里的「脚本参数」。参数可以为空。 |
| 工作目录 | `<cache>/resolvers/` |
| 标准输入 | 空 |
| 标准输出 | 上游地址。取最后一行非空内容。 |
| 标准错误 | 冷启动失败时记录到日志。最多保留 200 个字符。 |
| 超时 | 固定 30 秒 |
| 退出码 | 0 = 成功。非 0 = 失败。 |
| 缓存 | 按频道的「缓存秒数」复用结果。 |

输出地址必须满足两个条件：

1. 以 `http://` 或 `https://` 开头。
2. 是标准输出的最后一行。

标准输出可以有其他行。服务只取最后一行。
建议只输出一行地址，避免歧义。

### 3.3 缓存规则

解析脚本使用 stale-while-revalidate 缓存。

| 「缓存秒数」 | 行为 |
| --- | --- |
| 大于 0 | TTL 内不运行脚本。TTL 到期后，当前请求先拿旧地址，后台运行脚本刷新。 |
| 0 | 每次请求都触发后台刷新。当前请求仍用上一次成功的地址。 |
| 首次请求 | 没有旧地址。服务同步运行脚本，必须成功。 |

TTL 只影响刷新频率。它不保证每次请求都拿到新地址。

### 3.4 失败语义

| 场景 | 行为 |
| --- | --- |
| 脚本非 0 退出，且有旧地址 | 继续用旧地址。后台刷新失败不写日志。 |
| 脚本非 0 退出，且没有旧地址 | 跳转到 `[server].error_url` |
| 超过 30 秒 | 同失败。日志写超时。 |
| 输出不是 HTTP(S) 地址 | 同失败。日志写输出错误。 |
| 脚本文件不存在 | 同失败。 |
| 服务端没有 `python3` | 同失败。 |

### 3.5 最小示例

```python
#!/usr/bin/env python3
import sys

arg = sys.argv[1] if len(sys.argv) > 1 else ""
print("https://example.com/live.m3u8")
```

保存为 `resolvers/simple.py`。频道选择这个脚本，脚本参数留空。

### 3.6 带参数与鉴权的示例

```python
#!/usr/bin/env python3
import json
import os
import sys
import urllib.parse
import urllib.request

arg = sys.argv[1] if len(sys.argv) > 1 else ""
token = os.environ.get("MY_IPTV_TOKEN", "")
if not token:
    print("缺少 MY_IPTV_TOKEN", file=sys.stderr)
    sys.exit(2)

url = f"https://api.example.com/play/{urllib.parse.quote(arg)}"
request = urllib.request.Request(
    url,
    headers={"Authorization": f"Bearer {token}"},
)

try:
    with urllib.request.urlopen(request, timeout=10) as response:
        body = json.load(response)
except Exception as error:
    print(f"请求失败: {error}", file=sys.stderr)
    sys.exit(1)

print(body["stream_url"])
```

要点：

- 把错误写到标准错误。服务会把标准错误写进失败日志。
- 用非 0 退出码表示失败。
- 网络请求设置超时。脚本总超时只有 30 秒。
- 只输出一行地址。

### 3.7 排查

| 症状 | 检查 |
| --- | --- |
| 日志写「执行 python3 失败」 | 服务端是否有 `python3`，服务用户是否有执行权限 |
| 日志写「脚本不存在」 | 文件名是否正确，文件是否在 `resolvers/` 下 |
| 日志写「输出不是 HTTP(S) 地址」 | 标准输出最后一行是否是完整地址 |
| 日志写「超时（30s）」 | 脚本是否被网络请求阻塞 |
| 播放跳错误页 | 是否为首次请求失败，或没有旧地址 |
| 地址长期不更新 | 「缓存秒数」是否过大 |

本地测试：

```bash
cd <cache>/resolvers
python3 simple.py my-arg
echo "exit=$?"
```

本地测试的工作目录必须与服务一致。用相对路径读文件时，这一点很重要。

---

## 4. EPG 采集脚本

### 4.1 用途

EPG 源可以是 XMLTV 地址，也可以是脚本。脚本源适合这些场景：

- 节目单需要登录后获取。
- 节目单来自 JSON API，需要转换。
- 节目单需要合并多个上游。

### 4.2 调用契约

服务执行这条命令：

```text
<python_bin> <cache>/epg/scripts/<name>.py
```

| 项 | 规则 |
| --- | --- |
| 参数 | 无 |
| 工作目录 | `<cache>/epg/scripts/` |
| 标准输入 | 空 |
| 标准输出 | XMLTV 正文 |
| 标准错误 | 失败时记录到日志。最多保留 200 个字符。 |
| 超时 | 源的「超时（秒）」。0 表示用 `[epg].script_timeout_secs`。 |
| 退出码 | 0 = 成功。非 0 = 失败。 |
| 输出文件 | 标准输出为空时，按「产物文件」顺序读取同名文件。 |

超时会被限制在 5 到 3600 秒。全局默认值是 300 秒。

### 4.3 输出规则

服务按这个顺序取节目单：

1. 标准输出不为空。把标准输出当作 XMLTV。
2. 标准输出为空。按「产物文件」列表顺序查找文件。
3. 找到第一个存在的文件。把它当作 XMLTV。
4. 两者都没有。报错「脚本无输出」。

「产物文件」相对于 `epg/scripts/` 解析。
文件名同样经过消毒。不能写到目录外。

输出必须是 UTF-8 文本。推荐不加 BOM。

### 4.4 数据要求

- 根元素必须是 XMLTV 的 `<tv>`。
- 至少有一个 `<channel>`。没有频道的源会被判为失败。
- `<programme>` 的 `channel` 必须对应源里的频道 ID。
- 时间格式使用 XMLTV 标准格式，例如 `20261115080000 +0800`。
- 面板的「时区偏移」在解析后统一平移节目时间。

源频道 ID 不需要等于本地频道 ID。面板用「频道绑定」把两者对应起来。
`match_by_name` 打开时，同名频道可以自动绑定。

### 4.5 配置与开关

EPG 源的关键字段：

| 字段 | 说明 |
| --- | --- |
| `kind` | 脚本源填 `script` |
| `script` | 脚本文件名，例如 `foo.py` |
| `output_files` | 标准输出为空时读取的文件名列表 |
| `timeout_secs` | 本源的超时。0 表示用全局值 |
| `tz_shift_mins` | 节目时间平移，单位分钟 |
| `refresh_secs` | 采集节奏。0 表示随全局采集 |
| `enabled` | 源开关 |

全局开关：

| 配置 | 默认 | 作用 |
| --- | --- | --- |
| `epg.enabled` | false | EPG 总开关 |
| `epg.allow_scripts` | true | 允许脚本源 |
| `epg.python_bin` | null | 解释器命令 |
| `epg.script_timeout_secs` | 300 | 全局脚本超时 |

`allow_scripts` 关闭时，脚本源报错「已禁止脚本源」。

### 4.6 失败与缓存

- 脚本失败时，源状态记为失败，并保存错误文本。
- 上一次采集的 XMLTV 与 raw 缓存继续可用。
- 下一轮采集会重试。
- 输出不是合法 XMLTV 时，报错「解析失败」。
- XMLTV 没有频道时，报错「源没有频道」。
- 标准输出为空且没有产物文件时，报错「脚本无输出」。

旧数据不会因为一次失败被清空。修复脚本后，下一轮采集恢复。

### 4.7 最小示例

```python
#!/usr/bin/env python3

print('<?xml version="1.0" encoding="UTF-8"?>')
print('<tv generator-info-name="my-epg">')
print('  <channel id="cctv1">')
print('    <display-name>CCTV-1</display-name>')
print('  </channel>')
print('  <programme start="20261115080000 +0800" stop="20261115090000 +0800" channel="cctv1">')
print('    <title lang="zh">早间新闻</title>')
print('  </programme>')
print('</tv>')
```

保存为 `epg/scripts/my-epg.py`。添加脚本源，选择这个文件。

### 4.8 写产物文件的示例

脚本需要分步写文件时，可以不用标准输出。

```python
#!/usr/bin/env python3
from pathlib import Path

xml = """<?xml version="1.0" encoding="UTF-8"?>
<tv generator-info-name="my-epg">
  <channel id="cctv1"><display-name>CCTV-1</display-name></channel>
</tv>
"""

Path("epg.xml").write_text(xml, encoding="utf-8")
```

源配置：

- `script` = `my-epg.py`
- `output_files` = `epg.xml`

工作目录是 `epg/scripts/`，所以文件写在该目录。

### 4.9 抓取并转换上游 JSON 的示例

```python
#!/usr/bin/env python3
import json
import sys
import urllib.request
from xml.sax.saxutils import escape

try:
    with urllib.request.urlopen("https://api.example.com/epg", timeout=20) as response:
        rows = json.load(response)
except Exception as error:
    print(f"抓取失败: {error}", file=sys.stderr)
    sys.exit(1)

channels = sorted({row["channel_id"] for row in rows})

print('<?xml version="1.0" encoding="UTF-8"?>')
print('<tv generator-info-name="my-epg">')
for channel in channels:
    print(f'  <channel id="{escape(channel)}">')
    print(f"    <display-name>{escape(channel)}</display-name>")
    print('  </channel>')
for row in rows:
    print(
        '  <programme start="{start}" stop="{stop}" channel="{channel}">'.format(
            start=row["start"],
            stop=row["stop"],
            channel=escape(row["channel_id"]),
        )
    )
    print(f'    <title lang="zh">{escape(row["title"])}</title>')
    print('  </programme>')
print('</tv>')
```

要点：

- 转义文本。节目名可能包含 `&`、`<`、`>`。
- 所有节目必须在超时内输出完毕。
- 大源要流式处理。不要一次把所有内容放在内存里。

### 4.10 排查

| 症状 | 检查 |
| --- | --- |
| 源状态「已禁止脚本源」 | 打开 `epg.allow_scripts` |
| 「脚本不存在」 | 文件名是否带 `.py`，文件是否在 `epg/scripts/` |
| 「脚本无输出」 | 标准输出是否为空，产物文件是否存在 |
| 「解析失败」 | XML 是否合法，是否使用 UTF-8 |
| 「源没有频道」 | XML 是否至少有一个 `<channel>` |
| 节目不显示 | 源频道是否已绑定到本地频道 |
| 时间偏移 | 检查源的「时区偏移」与 XMLTV 的时区 |

本地测试：

```bash
cd <cache>/epg/scripts
python3 my-epg.py > /tmp/epg-test.xml
python3 -c 'import xml.etree.ElementTree as ET; ET.parse("/tmp/epg-test.xml")'
```

面板「节目指南 → 立即采集」可以手动触发一次采集。

---

## 5. 两类脚本对比

| 项 | 解析脚本 | EPG 采集脚本 |
| --- | --- | --- |
| 位置 | `<cache>/resolvers/` | `<cache>/epg/scripts/` |
| 输入 | `sys.argv[1]` | 无 |
| 成功输出 | 最后一行为 HTTP(S) 地址 | XMLTV 正文或产物文件 |
| 解释器 | 固定 `python3` | `epg.python_bin` |
| 超时 | 固定 30 秒 | 源配置或全局值，5–3600 秒 |
| 缓存 | SWR，按频道 TTL | raw 缓存，失败保留旧数据 |
| 失败影响 | 单个频道的播放 | 单个 EPG 源的采集 |
| 是否需要开关 | 否 | `epg.allow_scripts` |

---

## 6. 调试方法

### 6.1 查看日志

二进制安装：

```bash
journalctl -u tv-buddy -f
```

Docker：

```bash
docker compose logs -f tv-buddy
```

日志关键词：

- `解析脚本`：解析脚本错误与超时。
- `EPG 单源采集失败`：EPG 源采集失败。
- `节目单`：EPG 采集与生成错误。
- `脚本`：脚本缺失、超时或无输出。

### 6.2 手工运行

两类脚本都可以在服务端本地运行。工作目录必须与运行时一致。
见 §3.7 与 §4.10。

### 6.3 检查文件

```bash
docker compose exec tv-buddy ls -l /data/cache/resolvers/
docker compose exec tv-buddy ls -l /data/cache/epg/scripts/
```

### 6.4 检查接口

```bash
# 登录后取得 token
# 口令见面板账号或首次安装文件 admin_initial_credentials.txt
T=$(curl -s -X POST http://127.0.0.1:9900/panel/api/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"<你的口令>"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')

curl -s -H "Authorization: Bearer $T" http://127.0.0.1:9900/panel/api/resolver/scripts
curl -s -H "Authorization: Bearer $T" http://127.0.0.1:9900/panel/api/epg/scripts
```

`/panel/api/epg/scripts` 同时返回 Python 版本。值为 `null` 表示服务端没有 `python3`。

---

## 7. 安全与限制

脚本以服务进程身份运行。Docker 中的用户是 `tvbuddy`（UID 10001）。

| 项 | 规则 |
| --- | --- |
| 沙箱 | 没有。脚本可以读写服务用户可访问的文件，也可以访问网络。 |
| 权限 | 接口要求已登录。面板入口只对管理员显示。普通账号仍可直接调用接口。 |
| 文件名 | 只允许 `.py`。路径穿越被阻止。 |
| 资源限制 | 只有超时。没有内存与 CPU 配额。 |
| 环境变量 | 继承服务进程。脚本可以读到服务端环境变量。 |
| 日志 | 失败时只记录标准错误的前 200 个字符。 |
| 覆盖 | 同名上传会替换整个文件，不能恢复。 |
| 备份 | 备份包包含脚本源码。备份包按敏感文件保管。 |

只上传自己编写或审计过的脚本。不要上传来源不明的脚本。
不要依赖交互式输入。服务进程没有终端。

---

## 8. 面板与 API

### 8.1 解析脚本

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/panel/api/resolver/scripts` | 列表 |
| POST | `/panel/api/resolver/script` | 保存或覆盖 |
| POST | `/panel/api/resolver/remove` | 删除 |

列表响应：

```json
{
  "count": 1,
  "scripts": [{ "name": "simple.py", "bytes": 120 }]
}
```

保存请求：

```json
{ "name": "simple.py", "content": "#!/usr/bin/env python3\nprint('https://example.com/live.m3u8')" }
```

`content` 不能为空。文件不存在时，删除返回 404。

### 8.2 EPG 采集脚本

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/panel/api/epg/scripts` | 列表与 Python 版本 |
| POST | `/panel/api/epg/script` | 保存或覆盖 |
| POST | `/panel/api/epg/script-delete` | 删除 |

列表响应：

```json
{
  "python": "Python 3.11.2",
  "scripts": [{ "name": "my-epg.py", "size": 320 }]
}
```

保存请求：

```json
{ "name": "my-epg.py", "content": "#!/usr/bin/env python3\nprint('<tv></tv>')" }
```

`name` 必须以 `.py` 结尾。不要上传空正文。空正文会生成空文件。

### 8.3 脚本字段

解析脚本的频道字段：

| 字段 | 位置 | 说明 |
| --- | --- | --- |
| `resolver_script` | 频道扩展字段 | 脚本文件名 |
| `resolver_arg` | 频道扩展字段 | 传给脚本的参数 |
| `resolver_ttl` | 频道扩展字段 | 缓存秒数，默认 30 |

EPG 脚本的源字段见 §4.5。

---

## 9. 与其他脚本体系的区别

本项目不兼容 CharmingStreamer（`mpd-hsl`）的脚本协议。主要区别：

| 项 | TV Buddy | CharmingStreamer |
| --- | --- | --- |
| 输入 | 命令行参数 | stdin 的 JSON `ScriptContext` |
| 输出 | 地址或 XMLTV | 完整的 JSON `ctx` |
| 钩子 | 地址解析、EPG 采集 | MPD source、DRM key |
| 多脚本串联 | 不支持 | 同阶段按顺序执行 |
| DRM / CDM | 不支持 | 内置 Widevine / PlayReady |
| UA / Headers 覆盖 | 不支持 | source 脚本可以覆盖 |

不要照搬其他项目的示例。协议不同，脚本无法运行。

---

## 10. 上线检查清单

- [ ] 服务进程能执行 `python3`。
- [ ] 脚本位于 `resolvers/` 或 `epg/scripts/`。
- [ ] 文件名以 `.py` 结尾，不含路径。
- [ ] 脚本在本地用相同工作目录运行成功。
- [ ] 退出码为 0。
- [ ] 标准错误只写调试信息，不写敏感数据。
- [ ] 解析脚本只输出一行 HTTP(S) 地址。
- [ ] EPG 输出是合法 XMLTV，至少有一个频道。
- [ ] EPG 输出为 UTF-8。
- [ ] 运行时间低于超时。
- [ ] EPG 的「允许脚本源」已打开。
- [ ] 覆盖旧脚本前已备份。
- [ ] 备份包中的脚本按敏感文件保管。
