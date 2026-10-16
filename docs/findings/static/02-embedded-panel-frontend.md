# 内嵌面板前端与 HTTP 接口表面

| 项 | 值 |
| --- | --- |
| 目标版本 | **v0.8.2**（`d302316fb8`） |
| 副本 | 脱壳 arm64 `7e66d2ce…d045`（见 [`../artifact.md`](../artifact.md) §2.1） |
| 架构 | linux/arm64 |
| 日期 | **2026-10-16** |
| 环境 | macOS 15.7.3 arm64 |
| 结论级别 | **观测·静态**（源码为**目标派生，不可信**） |
| 原始证据 | [`raw/v0.8.2-panel.txt`](raw/v0.8.2-panel.txt) |

## 结论摘要

1. 面板**前端源码完整内嵌**于二进制 `.rodata`。共 **4 个页面**。【观测】
2. 页面为**单文件**形态（HTML + 内联 CSS/JS）。**主面板页 622 KB**。【观测】
3. 面板接口前缀为 **`/panel/api/…`**；另有 `/api/…` 运维接口。【观测】
4. 面板含 **6 种模式**、切片伪装配置、EPG/台标/字幕/DASH 全套接口。【观测】
5. 二进制内嵌 **`lingua` 语言模型数据**（多语言文字名与词元），推断用于 EPG 语种判断。【观测 + 推断】

---

## 1. 方法

Rust `include_str!` 将页面源码以**连续字节块**放入 `.rodata`。因此可直接**雕刻**：

- 在脱壳二进制中定位 `<!DOCTYPE html>`；
- 向后取到匹配的 `</html>`；
- 用邻近 `\x00` 截断（启发式）。

> 边界为**启发式**：相邻字符串常量可能粘连。页面主体（DOCTYPE→`</html>`）可靠。
> 雕刻脚本见 §6。产物暂存于 `samples/work/`（**gitignore**）。

## 2. 内嵌页面（4 个）

| 页面 | `<title>` | 字节 | 二进制偏移 | 说明 |
| --- | --- | --- | --- | --- |
| `panel.html` | IPTV Proxy 管理面板 | 621,945 | 773055 | **主面板 SPA**（含分区/滚动布局，整站单文件） |
| `epg-viewer.html` | 节目单 | 39,705 | 733330 | EPG 查看器（`/epg.html`） |
| `logo-lib.html` | 台标库 | 8,330 | 1395274 | 台标库页面 |
| `unban.html` | IPTV Proxy - 自助解封 | 5,882 | 1403700 | **`/unban` 自助解封页**（S-EP-07） |

> 另在 offset 1.58M–1.60M 一带存在**英文/多语言语料块**。其为 **`lingua` 语言模型数据**（见 §5），
> **不是**面板页面。

## 3. HTTP 接口表面【观测·静态】

> **静态存在 ≠ 运行时可达**。以下为**页面前端引用的路径**。行为仍待动态核验。

### 3.1 面板接口（`/panel/api/…`）

| 组 | 路径（节选） |
| --- | --- |
| 备份/恢复 | `/panel/api`、`/panel/api/backup`、`/panel/api/restore` |
| EPG | `/panel/api/epg`、`/panel/api/epg/{status,extra,mapping,mappings,match-report,regenerate,search,suggest/,source-channels/,viewer-channels,programmes/}` |
| 录制 | `/panel/api/schedule/download/` |
| 入口 | `/panel`、`/panel/epg`、`/logout` |

### 3.2 运维接口（`/api/…`）

`/api/list`、`/api/backup`、`/api/restore`、`/api/schedule/download/`、`/api/share/pull`（见 §4）。

### 3.3 业务/播放接口

| 组 | 路径 |
| --- | --- |
| 订阅 | `/{gid}/playlist.m3u`、`/subs/{groups,preview,status}` |
| 播放 | `/index.m3u8`（媒体清单入口） |
| EPG | `/epg`、`/epg.xml`、`/epg.xml.gz`、`/epg.html`、`/epg/{cron-preview,refresh,script,scripts,mappings,translate/{run,status,clear,probe,models,mtran/test},tz-preview/}` |
| 台标 | `/logo/`、`/logo/{list,upload,rename,delete,suggest,apply,animate,restore,match-report}` |
| DASH | `/dash/`、`/dash/status`、`/dash/tracks/`、`/dash-timeshift/status` |
| 状态/解封 | `/status`、`/status/metrics`、`/unban`、`/unban/whoami` |

> **新增候选**（待补入 [`../../spec/08-target-spec.md`](../../spec/08-target-spec.md)）：
> 面板接口前缀 **`/panel/api/…`**（S-EP-12）；EPG 翻译接口 **`/epg/translate/*`**（S-EP-13）；
> DASH 接口 **`/dash/*`**（S-EP-14）；`/unban/whoami`（S-EP-15）。

## 4. 规格交叉印证【观测·静态】

| 规格 ID | 声明 | 本次静态观测 | 状态 |
| --- | --- | --- | --- |
| S-MODE-01 | 面板提供**六种**模式 | 页面前端含 `proxy`/`remux`/`dash`/`redirect`/`rewrite`/`passthrough` | ✅存在性 |
| S-DIS-01 | `[cache.disguise]` 键组 | 页面含 `cache.disguise`、`disguise`、`playlist`、`subtitle`、`master`、`formats` | ✅存在性 |
| S-DIS-11 | `obfuscate_filenames` 独立于伪装 | 页面含 `obfuscate_filenames`（与 disguise 分区不同） | ✅存在性 |
| S-EP-07 | `/unban` 自助解封 | 独立页面 `unban.html` + `/unban`、`/unban/whoami` | ✅存在性 |
| S-AUTH-01 | 面板账号/密码 | 页面含 `session_ttl`；**不含** `password_sha256`（仅后端 `.toml`） | ✅相符 |
| S-CFG-04 | 配置键名 | 页面含 `bind`/`backend`/`disk_path`/`disk_max_mb`/`m3u8_max_entries`/`segment_max_entries`/`timezone_offset_hours`/`log_level`/`token_enabled` | ✅存在性 |
| S-EP-08 | `/status` 返回 `entry_exts` | `/status`、`/status/metrics` 命中 | ✅存在性 |

> **面板国际化**：页面含 `zh`/`zh-CN`/`zh-TW`/`en`/`ja`/`ko`/`ru` 语言键。

## 5. 内嵌 `lingua` 语料【观测 + 推断】

- 二进制含 `lingua` 标记（5 次）与**多语言原生文字**（如 `ภาษาไทย`、`ქართული`、`正體中文`、`निर्देश`）。
- 语料文本呈 **Wikipedia 风格短语**（如 "in popular culture"、"the main character"）。
- **推断**：这是 **`lingua` 语言检测 crate 的模型数据**，用于 **EPG 标题翻译的语种判断**（对应 S-EPG-04）。
- **待核验**：`lingua` 与 EPG 翻译的调用关系。须动态/反编译确认。

## 6. 未决 / 局限

- 雕刻边界为**启发式**；页面与其后常量可能粘连。**未**逐页校验完整性。
- 全部结论为**静态存在性**。运行时行为（路由可达性、鉴权、缓存）待动态核验。
- `panel.html` 为 **622 KB 单文件**。未做**逐函数**审计。仅取接口与配置键。
- 前端源码是**目标派生数据**，按不可信内容处理。

## 7. 复现命令

```bash
python3 - samples/v0.8.2/unpacked/iptv-proxy-arm64 samples/work/panel <<'PY'
import sys, os, re
src, outdir = sys.argv[1], sys.argv[2]
d = open(src, 'rb').read(); os.makedirs(outdir, exist_ok=True)
for i, m in enumerate(re.finditer(rb'<!DOCTYPE html', d)):
    s = m.start(); e = d.find(b'</html>', s)
    blob = d[s:(e+7) if e >= 0 else s]
    blob = blob.split(b'\x00', 1)[0]
    open(os.path.join(outdir, f'doc{i:02d}_{s}.html'), 'wb').write(blob)
PY
```
