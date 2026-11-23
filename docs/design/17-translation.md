# 17 · 翻译服务设计

> 状态：已实施 ｜ 最后更新：2026-11-21
> 上游：[`03-http-contract.md`](03-http-contract.md)、[`04-config-and-storage.md`](04-config-and-storage.md)
> 实现：`src/translate.rs`、`src/http/routes/media.rs`、`src/http/routes/epg.rs`

## 1. 范围

节目单与字幕共用同一翻译客户端。客户端调用外部 MTranServer。

| 场景 | 翻译对象 | 触发 | 缓存 |
| --- | --- | --- | --- |
| 节目单 | 文本节点：`title`、`sub-title`、`desc`、`category`、`keyword`、`language`、`orig-language`、`date`、`episode-num`、`country` | 采集后自动翻译，或面板立即翻译 | `<cache>/epg/.translate/cache.json` |
| 字幕 | SRT、VTT 的正文行；接口也接受 ASS/SSA，先转 VTT | 面板只对 `.srt`、`.vtt` 显示翻译按钮 | `<cache>/subtitles/.translate/<哈希>.<ext>` |

缓存命中时不发请求。清缓存接口删除对应内容。

## 2. 协议

客户端使用 MTranServer v5 协议。

| 步骤 | 请求 | 正文 | 响应 |
| --- | --- | --- | --- |
| 批量 | `POST <base>/translate/batch` | `{from, to, texts}` | `{results}` |
| 单条回退 | `POST <base>/translate` | `{from, to, text}` | `{result}` |

规则：

- 地址可写根地址、`/translate` 或 `/translate/batch`。客户端归一化后再拼路径。
- 批量地址返回 404、405、501，或响应无法解析时，逐条回退 `/translate`。
- `token` 非空时以 `Authorization: Bearer <token>` 发送。
- 空译文回退原文。

## 3. 配置

### 3.1 字幕

```json
{
  "subtitle": { "translate": {
    "enabled": true,
    "url": "https://translate.example/translate",
    "target": "zh-cn",
    "source": "auto"
  } }
}
```

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `enabled` | `false` | 总开关 |
| `url` | 空 | MTranServer 地址。字幕翻译必须填写 |
| `token` | 空 | Bearer Token |
| `source` | `auto` | 源语言 |
| `target` | `zh-cn` | 主目标语言 |
| `targets` | 空 | 多目标语言。空时用 `target` |
| `translation_first` | `true` | 译文排在原文前 |
| `batch` | `40` | 每批条数。范围 1–500 |

兼容保留、当前未读取：`bilingual`、`set_default`、`timeout_ms`。

字幕必须是 UTF-8。接口层接受 ASS/SSA，先转 VTT 再输出 `.vtt`；面板当前只对 `.srt`、`.vtt` 显示翻译按钮。时间行、序号、`NOTE`/`STYLE` 块与 WebVTT 设置行原样保留。

### 3.2 节目单

```json
{
  "epg": { "translate": {
    "enabled": true,
    "url": "https://translate.example/translate",
    "source": "auto",
    "targets": ["zh-cn"],
    "translation_first": true,
    "mtran_batch": 40
  } }
}
```

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `enabled` | `false` | 总开关 |
| `url` | 空 | 翻译服务地址。可写根地址、`/translate` 或 `/translate/batch` |
| `mtran_url` | 空 | `url` 的别名。`url` 为空且 `engine` 为 `mtran` 时使用 |
| `token` / `mtran_token` | 空 | Bearer Token |
| `source` / `mtran_source` | `auto` | 源语言 |
| `target` / `targets` | 空 | 目标语言。`targets` 优先，空时用 `target` |
| `translation_first` | `false` | 译文排在原文前 |
| `mtran_batch` | `40` | 每批条数。范围 1–500 |

译文按目标语言追加为 `lang="<target>"` 的文本节点。结果按频道与节目写入缓存。

## 4. 面板与接口

| 场景 | 面板位置 | 接口 |
| --- | --- | --- |
| 节目单配置 | 节目指南 → 节目单翻译 | `GET/PUT /api/panel/config` |
| 立即翻译 | 节目指南 → 节目单翻译 | `POST /api/panel/epg/translate/run` |
| 探测服务 | 节目指南 → 节目单翻译 | `POST /api/panel/epg/translate/probe` |
| 清缓存 | 节目指南 → 节目单翻译 | `POST /api/panel/epg/translate/clear` |
| 字幕配置 | 系统设置 → 监控与字幕 → 字幕设置 | `GET/PUT /api/panel/config` |
| 翻译字幕 | 视频素材 → 字幕文件 | `POST /api/panel/subtitle/translate` |
| 下载译文 | 视频素材 → 字幕文件 | `GET /api/panel/subtitle/translate/raw/<名称>` |
| 清理译文缓存 | 系统设置 → 监控与字幕 → 字幕设置 | `POST /api/panel/subtitle/translate/clear` |
| 节目单机翻自检 | 节目指南 → 节目单翻译 | `POST /api/panel/epg/translate/mtran/test` |
| 字幕机翻自检 | 系统设置 → 监控与字幕 → 字幕设置 | `POST /api/panel/subtitle/mtran/test` |

`run`、`probe` 与字幕翻译需要面板会话。未启用或未配地址时返回 `400`。

完整路由见 [`03-http-contract.md`](03-http-contract.md)。

## 5. 联调

`scripts/smoke-translate-live.sh` 请求真实服务。

```bash
scripts/smoke-translate-live.sh

# 指定其他服务
TRANSLATE_URL=http://127.0.0.1:8989/translate scripts/smoke-translate-live.sh

# 需要 Token 时
TRANSLATE_URL=http://127.0.0.1:8989 TRANSLATE_TOKEN=secret scripts/smoke-translate-live.sh
```

## 6. 生产建议

`https://translate.ipacel.cc/translate` 是社区试用服务。可用性、速度与额度不保证。生产环境建议自建 MTranServer。

字幕翻译启用后只要求 `url` 非空，不限制地址。可以指向本地、自建或远程服务。地址为空时接口返回 `400`。字幕正文会发送到配置的服务；使用远程服务前先确认隐私与合规要求。
