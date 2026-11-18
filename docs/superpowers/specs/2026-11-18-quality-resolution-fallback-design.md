# 质量校验：兜底速率、规则页重构与 ffprobe 分辨率识别

> 设计日期：2026-11-18 ｜ 状态：已实现
> 关联：`docs/design/13-availability-and-speedtest.md`、`docs/design/15-quality-admission-function.md`

## 1. 目的

质量校验需要解决三个问题：

1. 无法识别分辨率时，没有独立的速率兜底。当前只能回退到 `min_speed_mib`。
2. 规则页的“分辨率速率要求”表格会溢出。删除按钮压住“Host 缓存”开关。
3. 清单没有 `RESOLUTION` 时，质量探测没有分辨率。需要补充可靠的识别方案。

## 2. 目标与非目标

### 2.1 目标

- 增加 `fallback_speed_mib`。无法识别分辨率，或分辨率表未命中时，使用它。
- 保持旧配置行为。`fallback_speed_mib=0` 时回退到 `min_speed_mib`。
- 重构规则页。修复错位，降低信息密度，补齐窄屏布局。
- 在 `full` 档增加可选的分辨率识别。用已下载切片的字节运行 `ffprobe`。
- 清单已有 `RESOLUTION` 时，不运行 `ffprobe`。
- `ffprobe` 失败时保留“分辨率未知”，使用兜底速率。不判 `probe_error`。
- 补单元测试、前端构建检查与手工验收。
- 规则变化后按 `rule_hash` 重新判定到期频道。
- 面板安装同时提供 `ffmpeg` 与 `ffprobe`。

### 2.2 非目标

- 不识别帧率、编码、音轨数量。`min_fps`、`require_video`、`require_audio` 仍不实现。
- 不推断广告。
- 不扩展 DASH 探测。DASH 在非连通档仍判 `probe_error`。
- 不从 `BANDWIDTH` 猜分辨率。
- 不新增数据库列。分辨率来源只写调试日志。
- 不把 `ffprobe` 配置开放到频道、分组、来源覆盖。进程成本是全局资源。
- 不支持需要初始化段的 fMP4 HLS。第一版只处理 MPEG-TS 切片。

### 2.3 反目标

- 不默认运行 `ffprobe`。它默认关闭。
- 不在 `speed` 档运行 `ffprobe`。
- 不把 `ffprobe` 失败当成频道失败。
- 不改变清单已有 `RESOLUTION` 的判定路径。

## 3. 现状

### 3.1 分辨率来源

质量探测只解析 HLS 主清单：

```rust
"RESOLUTION" => {
    if let Some((w, h)) = value.split_once('x') {
        width = w.trim().parse().ok();
        height = h.trim().parse().ok();
    }
}
```

位置：`src/quality/probe.rs:100-121`。

没有 `RESOLUTION` 时，`width` 与 `height` 都是 `None`。

### 3.2 判定

位置：`src/quality/rules.rs:261-284`。

- 分辨率上下限只在宽高都存在时生效。
- 分辨率速率表按像素面积取不超过它的最高档。
- 表未命中或宽高未知时，使用 `min_speed_mib`。

### 3.3 档位

`probe_mode="full"` 当前映射为测速：

```rust
"speed" | "full" => ProbeMode::Speed,
```

位置：`src/quality/schedule.rs:44-49`。

质量探测没有 `ffprobe` 调用。`ffprobe` 只用于台标转换。

### 3.4 规则页

`frontend/src/quality-page.tsx:720-840` 的判定阈值卡使用 `.fields` 网格：

```css
.fields {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
}
```

宽屏会生成 5 列。“分辨率速率要求”是普通字段。内部行固定为 132 px 与 108 px，加上删除按钮后超过 240 px。内容溢出到下一列，压住“Host 缓存”。

## 4. 已选方案

用户已选择：

- 兜底：单字段 `fallback_speed_mib`。
- UI：结构化重构。
- `ffprobe`：只在 `full` 档按需运行。

### 4.1 兜底语义

设：

- `fallback = fallback_speed_mib > 0 ? fallback_speed_mib : min_speed_mib`
- `px = width * height`

判定顺序：

1. 宽高已知，表中有 `key <= px` 的最高档 → 用该档。
2. 宽高已知，表中没有匹配档 → 用 `fallback`。
3. 宽高未知 → 用 `fallback`。
4. `fallback_speed_mib=0` → `fallback` 等于 `min_speed_mib`。

“匹配档”按像素面积向下取整。例如 1280x720 档匹配 1920x1080。只有不存在 `key <= px` 的档时才算未命中。

分辨率上下限不参与兜底。宽高未知时，上下限仍跳过。

### 4.2 配置字段

`QualityCfg` 新增：

| 字段 | 类型 | 默认 | 范围 | 说明 |
| --- | --- | --- | --- | --- |
| `ffprobe` | bool | false | — | 启用分辨率识别 |
| `ffprobe_timeout_secs` | u64 | 5 | 1–30 | 单次进程超时 |
| `probe_concurrency` | u32 | 1 | 1–4 | `ffprobe` 并发与采集上限 |
| `max_ffprobe_per_run` | u64 | 0 | 0– | 单轮上限。0 = 不限 |
| `ffprobe_min_speed_mib` | f64 | 0.0 | 0– | 达到该速率才运行 |
| `ffprobe_sample_kb` | u64 | 512 | 64–2048 | 送给 `ffprobe` 的最大字节数 |

`QualityRules` 新增：

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `fallback_speed_mib` | f64 | 0.0 | 0 = 使用 `min_speed_mib` |

`QualityOverride` 新增：

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `fallback_speed_mib` | Option\<f64\> | None | 未写时继承上一层 |

`EffectiveRules` 新增：

```rust
pub fallback_speed_mib: f64,
```

`effective_rules` 按频道 > 分组 > 来源 > 全局解析。`QualityRules` 的默认值保持旧行为。

### 4.3 判定实现

`effective_rules` 增加：

```rust
fallback_speed_mib: pick(channel_q, group_q, source_q, |quality| {
    quality.fallback_speed_mib
})
.unwrap_or(rules.fallback_speed_mib),
```

`evaluate` 改为：

```rust
let fallback = if rules.fallback_speed_mib > 0.0 {
    rules.fallback_speed_mib
} else {
    rules.min_speed_mib
};
let threshold = match (measurement.width, measurement.height) {
    (Some(width), Some(height)) => {
        let px = u32::from(width) * u32::from(height);
        rules
            .resolution_speed_map
            .iter()
            .rev()
            .find(|(key, _)| **key <= px)
            .map(|(_, speed)| *speed)
            .unwrap_or(fallback)
    }
    _ => fallback,
};
```

`required_probe_mode` 增加：

```rust
if rules.fallback_speed_mib > 0.0 {
    mode = mode_at_least(mode, ProbeMode::Speed);
}
```

否则只设置兜底速率的配置不会触发测速。

### 4.4 规则哈希与重新判定

`rule_hash` 当前只写不读。本次补上读取与比较。

哈希输入分两部分：

1. `EffectiveRules`，含 `fallback_speed_mib`。
2. `ProbeSignature`，记录会改变测量结果的全局参数：
   - `probe_mode`
   - `ffprobe`
   - `ffprobe_ready`（`ffprobe=true` 且 `-version` 探测成功）
   - `ffprobe_sample_kb`
   - `ffprobe_min_speed_mib`
   - `sample_segments`
   - `sample_bytes_kb`
   - `sample_secs`

超时、并发、预算不进入哈希。它们只影响运行成本，不改变判定语义。修改后等正常周期生效。

`ffprobe_ready` 进入哈希，用于覆盖“先开启但未安装，后安装”的情况。安装或删除成功后，清除可用性缓存。

`ffprobe_ready` 由 `state.ffprobe_ready().await` 提供。缓存为空时先运行 `ffprobe -version` 并写入缓存。`engine::run`、`filter_plan` 与状态接口共用这个入口。

`rule_hash` 签名改为：

```rust
pub fn rule_hash(rules: &EffectiveRules, probe: &ProbeSignature) -> String
```

前缀升为 `tvb-quality-rules-v2`。

现有 `due_targets` 守卫是：

```rust
if !include_not_due && next_check_at > now {
    continue;
}
```

改为：

```rust
let next_due = stored.as_ref().map_or(true, |result| result.next_check_at <= now);
let stale_hash = stored.as_ref().is_some_and(|result| {
    result.rule_hash.is_empty() || result.rule_hash != current_hash
});
let due = next_due || stale_hash;
if !include_not_due && !due {
    continue;
}
```

规则：

- 周期运行：`include_not_due=false`。哈希变化或到期都探测。
- 手动运行：`include_not_due=true`。保持现有“校验范围内全部频道”的语义，不因哈希跳过。手动运行不用于验证哈希重判。
- 质量关闭（`measure_only`）的手动运行也探测全部。
- 同步门控（`filter_plan`）只处理新增与 URL 变化的候选。它不检查哈希。现有频道的规则变更由周期运行覆盖。
- `rule_hash` 为空的历史结果视为 stale。
- 单轮上限：周期运行使用 `max_channels_per_run`。同步门控使用 `run_limit`。未处理完的频道保持到期，下一轮继续。
- `rule_hash` 调用点：`gate::decide`、`gate::paused_outcome`、`engine::decide`、两处 `result_for`。它们都从同一个 `ProbeSignature` 取值。

测试：

- 哈希变化后，`next_check_at` 未到也视为到期。
- 哈希相同且未到期时不重复探测。
- 空哈希视为到期一次。
- 手动运行即使哈希相同也探测全部。
- `max_channels_per_run` 截断后，剩余频道下一轮仍到期。

### 4.5 API

`GET /panel/api/quality/effective` 的 `rules` 增加 `fallback_speed_mib`。

`GET /panel/api/quality/status` 增加：

```json
"ffprobe": {
  "installed": true,
  "enabled": false,
  "ready": false,
  "note": ""
}
```

`installed` 由 `ffprobe -version` 探测得出，不检查相对路径是否存在。结果缓存。安装或删除任务成功后清空缓存。

## 5. ffprobe 设计

### 5.1 为什么不在播放列表 URL 上运行

实测 `ffprobe 9.0.2`：

```text
$ ffprobe -f hls https://cdn6.cc.cd/163189/fct
URL https://cdn6.cc.cd/api/fct/35550 is not in allowed_segment_extensions
Invalid data found when processing input
```

该源的切片路径是 `/api/fct/35550`，没有 `.ts` 扩展名。`ffprobe` 的 HLS 解复用器拒绝它。`-allowed_extensions ALL` 也没有绕过。

直接探测一个切片则成功：

```text
$ ffprobe -f mpegts -select_streams v:0 \
    -show_entries stream=codec_type,width,height -of csv=p=0 \
    https://cdn6.cc.cd/api/fct/35550
video,1920,1080
```

前 512 KiB 已经足够：

```text
$ head -c 524288 segment.ts | ffprobe -f mpegts ... -
video,1920,1080
```

结论：不要重新访问播放列表。复用测速阶段已下载的切片字节，通过标准输入交给 `ffprobe`。这更快、更安全，也不受切片扩展名限制。

### 5.2 采集切片

`read_counted` 增加可选采集：

```rust
async fn read_counted(
    response: Response,
    max_bytes: usize,
    max_secs: f64,
    capture: Option<&mut Vec<u8>>,
    capture_limit: usize,
) -> Result<(u64, Duration), String>
```

当前实现只计数。改动后：

- 从切片开头读取。
- 最多保留 `capture_limit` 字节。
- `bytes` 计数保持不变。
- 只对第一个成功响应的采样切片采集。
- 只在 `ProbeMode::Full`、`ffprobe=true` 且宽高未知时采集。

内存上限：`ffprobe_sample_kb` 默认 512 KiB。并发 2 时约 1 MiB。

### 5.3 运行条件

`ffprobe` 只在以下条件全部满足时运行：

1. 档位是 `ProbeMode::Full`。
2. `quality.ffprobe=true`。
3. 清单解析后 `width` 或 `height` 为空。
4. 已测得 `speed_mib`。
5. `speed_mib >= ffprobe_min_speed_mib`。
6. 采集字节非空。
7. 单轮 `max_ffprobe_per_run` 未达到。
8. `ffprobe -version` 探测成功（使用缓存的 `ffprobe_ready`）。

可选的提前跳过：若 `speed_mib` 低于所有可能阈值的下界，则无需识别分辨率。下界是 `min_speed_mib`、`fallback_speed_mib`（大于 0 时）与速率表值的最小值。低于下界的频道必定失败。

### 5.4 进程调用

使用 `tokio::process::Command`。参数：

```text
ffprobe -v error
  -f mpegts
  -probesize 512K
  -analyzeduration 1000000
  -select_streams v:0
  -show_entries stream=codec_type,width,height
  -of json
  -
```

规则：

- `-f mpegts` 仅当采集字节以 TS 同步字节 `0x47` 开头且每 188 字节重复时使用。
- 其他容器不加 `-f`，让 `ffprobe` 自动探测。探测失败则放弃。
- 通过 `stdin` 写入采集字节。不访问网络。
- `kill_on_drop(true)`。
- `tokio::time::timeout(ffprobe_timeout_secs)`。
- 标准输出上限 64 KiB。标准错误上限 4 KiB，只写调试日志。
- 不经过 shell。字节只通过管道传递。

### 5.5 解析

输出示例：

```json
{
  "streams": [
    { "codec_type": "video", "width": 1920, "height": 1080 }
  ]
}
```

规则：

- 取第一个 `codec_type="video"` 且 `width>0`、`height>0` 的流。
- 成功：写入 `measurement.width`、`measurement.height`。日志记 `source=ffprobe`。
- 失败：保持 `None`。日志记 `source=unknown` 与错误摘要。`probe_error` 保持 false。
- 不覆盖清单已有的分辨率。

### 5.6 并发、上限与采集内存

有两条准入入口：

- `engine::run`：周期运行与手动运行。
- `subscription::filter_plan`：同步源新增或 URL 变化的门控。

两条入口各自创建两个句柄：

- `probe_semaphore`：许可数为 `probe_concurrency`。同时限制采集缓冲与 `ffprobe` 进程。
- `ffprobe_counter`：本操作已尝试的 `ffprobe` 次数。

两条入口共用 `admission_lock`，不会并发运行。因此“每次准入操作一份”等价于“同一时间只有一份”。

上限检查顺序：

1. 若 `max_ffprobe_per_run > 0`，执行 `fetch_add(1)`。
2. 返回的旧值 `>= max_ffprobe_per_run` 时，执行 `fetch_sub(1)` 并跳过。
3. 获取 `probe_semaphore` 许可。
4. 采集、运行 `ffprobe`，然后释放许可。

`fetch_add` 必须先于信号量。否则多个并发探测会同时看到计数未到上限，然后都运行。计数器统计“尝试次数”，失败也消耗一次。

采集内存：

- 需要采集时先尝试获取 `probe_semaphore` 许可。
- 获取失败时，本次探测不采集，不运行 `ffprobe`，分辨率保持未知。测速继续。
- 许可覆盖“采集 + ffprobe”整段。缓冲释放后再释放许可。
- 峰值采集内存不超过 `probe_concurrency * ffprobe_sample_kb`。
- 默认 1 × 512 KiB。最大 4 × 2048 KiB = 8 MiB。
- 只使用一个信号量，固定“先许可、后采集、再 ffprobe”的顺序，不会死锁。

`max_ffprobe_per_run` 的口径是“每次准入操作”。同步与周期不共享同一个计数。

### 5.7 Host 缓存

Host 缓存只缓存测速值。分辨率与 URL 绑定，不能按 Host 共享。

规则：

- `full` 档且 `ffprobe=true` 时，一律不使用 Host 缓存捷径。探测前无法知道宽高；先浅探测再补第二次会增加一次请求。直接跳过只影响这一种组合。
- `full` 档但 `ffprobe=false` 时，按现有 Host 缓存逻辑运行。
- 仍可读取 URL 级 memo 缓存。memo 指纹加入 `ProbeMode::Full` 的等级 3。

### 5.8 档位

新增：

```rust
pub enum ProbeMode {
    Reachability,
    Playlist,
    Speed,
    Full,
}
```

`probe_mode` 映射：

```rust
"full" => ProbeMode::Full,
```

`probe_rank` 增加 `Full => 3`。

`required_probe_mode` 在规则要求测速时至少升到 `Speed`。配置本身是 `full` 时保持 `Full`。

`full` 但 `ffprobe=false` 时，`probe` 按 `Speed` 执行。日志记一次“ffprobe 未启用”。

### 5.9 失败处理

| 情况 | 结果 |
| --- | --- |
| `ffprobe` 未安装 | 跳过。日志记 unknown。使用兜底。 |
| 进程超时 | 终止进程。保持未知。使用兜底。 |
| 解析失败 | 保持未知。使用兜底。 |
| 没有视频流 | 保持未知。使用兜底。 |
| 单轮上限达到 | 保持未知。使用兜底。 |
| 清单已有分辨率 | 不运行。使用清单值。 |

任何失败都不改变可达性、延迟与速率。任何失败都不产生 `probe_error`。

### 5.10 安装器扩展

`Artifact` 增加 `name` 字段。`artifact_for` 返回固定顺序的 `Vec<Artifact>`：`ffprobe`、`ffmpeg`。

安装流程：

1. 计算两个制品总大小。
2. 下载两个制品到 `<name>.part`。
3. 校验两个 sha256。
4. 解压两个到 `<name>.unpack`。
5. 两个都成功后，再原子改名为 `<name>`，设置 `0755`。
6. 进度回调使用累计下载量与总大小。

删除流程：删除两个正式文件与两类临时文件。返回是否删除了任一正式文件。

状态接口：分别探测两个文件。`installed` 表示 `ffmpeg` 可用；`ffprobe_installed` 表示 `ffprobe` 可用。两者独立。

失败规则：下载、校验或解压任一步失败，删除本次临时文件。两个正式文件都不改变。改名阶段例外：第二个改名失败时，第一个已启用的文件保留。暂存后统一改名，避免下载或校验阶段出现半成品。

现有安装器在启用前要求 stdout 包含 `"ffmpeg version"`。多制品版本改为按 `Artifact.name` 检查：

- `ffmpeg` → `"ffmpeg version"`
- `ffprobe` → `"ffprobe version"`

临时文件与目标文件名也必须来自 `Artifact.name`：

- `<name>.part`
- `<name>.unpack`
- `<name>`

测试替身 `Installer.artifact: Option<Artifact>` 改为 `Vec<Artifact>`。测试覆盖两个制品、版本前缀、删除与总进度。

4 个 `ffprobe` 制品的大小与 sha256 已于 2026-11-18 通过 GitHub API 核对。安装前仍应由发布流程复核。

## 6. 规则页重构

### 6.1 信息架构

规则页保留现有四个卡片：

1. 运行参数。
2. 判定阈值。
3. 安全阀。
4. 覆盖规则。

“判定阈值”内部拆为三块：

- 速率与延迟：最低速率、兜底速率、最大延迟。
- 分辨率：最低分辨率、最高分辨率、Host 缓存、分辨率速率表。
- 其他：手工下线宽限。

新增“分辨率探测”卡片，放在“判定阈值”与“安全阀”之间。

### 6.2 布局

规则卡片使用容器查询，不再使用视口断点：

```css
.quality-rules-card {
  container-type: inline-size;
}

.quality-rules-grid {
  display: grid;
  grid-template-columns: 1fr;
  gap: 12px 16px;
}

@container (min-width: 640px) {
  .quality-rules-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@container (min-width: 1000px) {
  .quality-rules-grid {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
}

.quality-field-wide {
  grid-column: 1 / -1;
}
```

要点：

- 最多 3 列。宽屏不再出现 5 列。
- 速率表使用 `quality-field-wide`。
- 所有 `input` 使用 `min-width: 0; width: 100%`。
- 移除固定宽度 132 px 与 108 px。

### 6.3 速率表

使用原生 `table`，保留语义：

```text
| 分辨率     | 最低速率 (MiB/s) | 操作 |
| 1280x720   | 0.5              | 删除 |
| 1920x1080  | 1000             | 删除 |
```

规则：

- 表头固定显示。
- 输入框带 `aria-label`。
- 删除按钮保留可见文字“删除”。
- 表格下方有“添加分辨率”按钮。
- 窄屏（容器宽度 < 560 px）隐藏表头，每个输入显示自己的小标签。
- 不使用横向滚动。

### 6.4 兜底字段

“兜底速率”放在“最低速率”右侧：

- 标签：兜底速率
- 单位：MiB/s
- 提示：`无法识别分辨率，或速率表未命中时使用；0 = 用最低速率`
- 控件：`NumberInput`，最小 0，步长 0.1。

### 6.5 分辨率探测卡

字段：

| 字段 | 控件 | 提示 |
| --- | --- | --- |
| 启用分辨率识别 | Switch | 仅“完整”档生效。 |
| 单次超时 | 数字，秒 | 1–30。 |
| 并发 | 数字 | 1–4。 |
| 单轮上限 | 数字 | 0 = 不限制。 |
| 最低测速门槛 | 数字，MiB/s | 0 = 不限制。 |
| 采样大小 | 数字，KiB | 读取切片开头。64–2048。 |

状态提示：

- `ffprobe` 未安装时显示警告：“未安装 ffprobe。分辨率识别不可用，规则会使用兜底速率。”
- `ffprobe` 已安装但未启用时显示提示：“开启后，完整档会读取切片开头识别分辨率。”
- `probe_mode=full` 但 `ffprobe=false` 时，运行参数卡显示“实际档位：测速（ffprobe 未启用）”。

### 6.6 其他文案

- 探测档位“完整”的说明从“第一版按测速执行”改为“测速，并按需识别分辨率”。
- `PROBE_LABEL.full` 从“测速”改为“完整”。
- 覆盖规则标签增加 `fallback_speed_mib`：兜底速率。
- 结果详情抽屉没有规则快照，不增加兜底速率展示（不在本设计范围内）。

## 7. 兼容性

- 旧配置缺少新字段时使用默认值。
- `fallback_speed_mib=0`、`ffprobe=false` 时，判定结果与现在一致。
- 规则哈希升为 v2。`rule_hash` 不一致的频道在下一轮周期运行中重新判定。
- 规则变更可能让一轮探测量增加。`max_channels_per_run`、`budget_mb_per_run` 与轮转游标保证分摊到多轮。
- 面板安装由单个 `ffmpeg` 变为 `ffmpeg` + `ffprobe`。旧安装的 `ffmpeg` 继续可用；重新安装后两者都到位。
- 新字段未写入时不会出现在 `QualityOverride` 的已覆盖字段列表。
- `probe_mode=full` 在 `ffprobe=false` 时仍按测速执行，行为与现在相同。

## 8. 代码改动清单

### 8.1 后端

- `src/config/model.rs`
  - `QualityCfg` 新增 6 个字段与默认值。
  - `QualityRules` 新增 `fallback_speed_mib`。
  - `QualityOverride` 新增 `fallback_speed_mib`。
- `src/config.rs`
  - `validate_quality` 增加新字段范围校验。
- `src/quality/rules.rs`
  - `EffectiveRules` 新增字段。
  - `effective_rules` 解析。
  - `evaluate` 使用兜底。
  - 测试。
- `src/quality/admission.rs`
  - `rule_hash` 加入兜底字段与 `ProbeSignature`，前缀升 v2。
  - `has_override` 加入 `fallback_speed_mib`。
- `src/quality/schedule.rs`
  - `probe_mode` 支持 `full`。
  - `required_probe_mode` 在兜底 > 0 时提升到测速。
  - `ProbeLimits` 增加 `ffprobe_path`、`ffprobe_ready` 与采集参数。
  - `probe_limits` 接收调用方的 `ffprobe` 路径与可用性。
- `src/quality/probe.rs`
  - `ProbeMode::Full`。
  - 采集切片。需要采集时获取 `probe_semaphore`，持有到 `ffprobe` 完成。
  - `ffprobe` 子模块：格式判断、进程调用、JSON 解析。
  - 失败处理。
- `src/quality/gate.rs`
  - memo 等级加入 Full。
  - 传递 `probe_semaphore` 与 `ffprobe_counter`。
  - Host 缓存规则。
  - `decide`、`paused_outcome`、`result_for` 传入 `ProbeSignature`。
- `src/quality/engine.rs`
  - 每轮创建两个句柄：`probe_semaphore` 与 `ffprobe_counter`。
  - 每次探测传递。
  - `due_targets` 比较 `rule_hash` 与 `ProbeSignature`。
  - `decide` 传入 `ProbeSignature`。
- `src/subscription.rs`
  - `filter_plan` 创建两个句柄。
- `src/http/routes/quality.rs`
  - `effective` 增加兜底。
  - `status` 增加 `ffprobe` 状态。
- `src/http/routes/media.rs`
  - `ffmpeg-status` 增加 `ffprobe` 的来源、版本、路径与安装状态。
- `src/state.rs`
  - 增加 `ffprobe` 可用性缓存与清理入口。
- `src/tools/install.rs`
  - 平台制品从单个 `ffmpeg` 改为 `ffmpeg` + `ffprobe`。
  - 新增 4 个平台的 `ffprobe` 固定制品、大小与 sha256。
  - 按制品检查版本前缀，按 `name` 生成临时与目标路径。
  - 安装进度覆盖两个文件。删除同时移除两者。
- `src/http/routes/panel.rs`
  - 无需改动。`PUT /panel/api/config` 自动保存新字段。

`ffprobe` 固定制品：

| 平台 | 文件 | 大小 | sha256 |
| --- | --- | --- | --- |
| linux x86_64 | `ffprobe-linux-x64.gz` | 29,276,839 | `25d9b6ccb05e3d9de9e04e31e2506d8dd7f9f0418981965ac6df12e8d3afd067` |
| linux aarch64 | `ffprobe-linux-arm64.gz` | 25,493,573 | `2ab6aba60ee84412dff9188720703376cb4e7aaf7e0b5e43aa8249f2acae5bf8` |
| macos x86_64 | `ffprobe-darwin-x64.gz` | 25,239,438 | `d4da574d6e2e197bd259b47d69cf262df9e312af24ad960444f6d806d3d4c186` |
| macos aarch64 | `ffprobe-darwin-arm64.gz` | 19,207,077 | `d986a8ec7b030899fe66a8a288ed809a3543338705a3ce178cfb85869c5d80be` |

安装顺序：先 `ffprobe`，后 `ffmpeg`。任一步失败则整体失败，已下载的临时文件清理。

### 8.2 前端

- `frontend/src/api.ts`
  - `QualityRules`、`QualityStatus` 类型增加字段。
- `frontend/src/quality-page.tsx`
  - 拆分判定阈值卡。
  - 新增兜底字段。
  - 重写速率表。
  - 新增分辨率探测卡。
  - 更新 `PROBE_LABEL` 与提示。
  - 覆盖标签增加兜底速率。
- `frontend/src/style.css`
  - 新增容器查询网格。
  - 重写 `.quality-speed-map` / `.quality-speed-row`。
  - 删除固定宽度。
- `frontend/src/ffmpeg.tsx`
  - 状态卡显示 `ffmpeg` 与 `ffprobe` 两行。
  - 安装按钮、进度与删除确认覆盖两者。
- `frontend/src/quality.tsx`
  - 无需改动。结果原因不变。

### 8.3 文档

- `docs/design/13-availability-and-speedtest.md`
  - 更新 `probe_mode=full` 的实现状态。
  - 把 §5.6 的“在最终地址上运行 ffprobe”改为“切片字节 + stdin”。
  - 把 ffprobe 失败从 `probe_error` 改为“保持未知，使用兜底”。
  - 记录 fMP4 限制与新的配置字段。
- `docs/design/15-quality-admission-function.md`
  - 规则页增加兜底速率与分辨率探测卡。
- `docs/tests/07-python-m3u8-simulation.md`
  - 增加兜底与 ffprobe 验收项。
- `docs/superpowers/specs/2026-11-18-panel-ffmpeg-install-design.md`
  - 安装制品改为 `ffmpeg` + `ffprobe`。更新状态与删除说明。

## 9. 测试与验收

### 9.1 单元测试

`rules.rs`：

- 宽高未知，兜底 0.8 → 阈值 0.8。
- 宽高未知，兜底 0 → 阈值等于 `min_speed_mib`。
- 宽高 1280x720，表有 720p → 用表值。
- 宽高 1920x1080，表只有 2560x1440 → 用兜底。
- 宽高 1920x1080，表有 1080p → 用 1080p。
- 宽高未知，上下限不生效。
- 兜底进入有效规则覆盖链。

`admission.rs`：

- 兜底变化会改变 `rule_hash`。
- `ProbeSignature` 变化会改变 `rule_hash`。
- `ffprobe_ready` 从 false 变 true 会改变 `rule_hash`。
- 哈希前缀为 v2。
- `has_override` 识别 `fallback_speed_mib`。

`config.rs`：

- `ffprobe_timeout_secs`、`probe_concurrency`、`ffprobe_sample_kb` 的超界值被拒绝。
- `fallback_speed_mib` 与 `ffprobe_min_speed_mib` 的负值被拒绝。

`schedule.rs`：

- 只设置兜底速率时，档位至少为 `Speed`。
- `full` 映射为 `Full`。

`engine.rs`：

- 哈希不一致且未到期 → 到期。
- 哈希一致且未到期 → 不到期。
- 空哈希 → 到期一次。
- 手动运行即使哈希相同也探测全部。
- `max_channels_per_run` 截断后，剩余频道下一轮仍到期。

`gate.rs`：

- `full` + `ffprobe` 命中 Host 缓存时仍走 Full 探测。
- `max_ffprobe_per_run` 在并发下不被突破。
- `probe_semaphore` 不可用时，测速继续，分辨率保持未知。

`tools/install.rs`：

- 4 个平台的 `ffprobe` 制品存在，大小与 sha256 固定。
- 两个制品分别用 `ffmpeg version` 与 `ffprobe version` 校验。
- `remove_managed` 同时删除 `ffmpeg` 与 `ffprobe`。
- 安装进度总量等于两个制品大小之和。
- 第二个制品失败时，第一个制品不启用。

`media.rs`：

- `ffmpeg-status` 同时返回 `ffmpeg` 与 `ffprobe` 的状态。

`probe.rs`：

- TS 同步字节判断。
- `ffprobe` JSON 解析：正常、无视频流、空输出。
- 采集上限。
- 采集许可不可用时跳过采集，测速继续。
- 采集许可在获取 `ffprobe` 许可前释放。
- `ffprobe` 关闭时不采集。
- 清单有分辨率时不运行。

### 9.2 手工验收

使用 `docs/tests/07-python-m3u8-simulation.md` 的模拟器，并增加一个真实 TS 切片源：

1. 用 `ffmpeg` 生成一份不带 `RESOLUTION` 的主清单。切片 URL 不带 `.ts` 扩展名。
2. 设置 `probe_mode=full`、`ffprobe=true`。
3. 预期结果页显示 `1920x1080`，原因按速率表或兜底判定。
4. 关闭 `ffprobe`。预期分辨率显示 `—`，使用兜底速率。
5. 设置兜底 0.8、最低速率 0。慢速频道预期 `speed_low`。
6. 已知 1080p，但速率表只有 2560x1440（或表为空）。预期使用兜底速率。
7. 在 1440、1024、720、460 宽度检查规则页。预期无重叠、无横向滚动。
8. `ffprobe` 未安装时，预期提示可见，判定不产生 `probe_error`。
9. 修改兜底速率并保存。开启质量校验，把周期设为 1 小时，等待自动运行。预期哈希变化的频道重测，未变化的跳过。手动“立即校验”仍探测全部，不用于本项。
10. 面板安装。预期 `ffmpeg` 与 `ffprobe` 都下载、校验并可用。执行 `ffprobe -version` 成功。删除后两者都消失。
11. 切换 `ffprobe` 开关。`ProbeSignature` 是全局的，所有频道哈希都会变化；下一轮周期运行会重测全部频道（受单轮上限约束）。
12. 先开启 `ffprobe` 但不安装，运行一轮。再安装 `ffprobe`。预期可用性变化使哈希变化，下一轮重新探测并取得分辨率。

### 9.3 回归

- `cargo test`
- `cargo clippy --all-targets`
- `cd frontend && pnpm build`
- 现有质量准入手工验收不变。

## 10. 风险与取舍

| 风险 | 影响 | 处理 |
| --- | --- | --- |
| 切片开头不足以解析 | 分辨率保持未知 | 使用兜底。提示增大 `ffprobe_sample_kb`。 |
| fMP4 HLS 需要初始化段 | 无法识别 | 第一版不支持。使用兜底。 |
| `ffprobe` 进程拖慢低配机 | 单轮变慢 | 默认关闭；并发 1；单轮上限；最低测速门槛。 |
| 采集内存峰值 | 低配机内存压力 | `probe_semaphore`；峰值不超过 `probe_concurrency * ffprobe_sample_kb`。默认 512 KiB，最大 8 MiB。 |
| `ffprobe` 可用性误判 | 永远不运行或误报未安装 | 用 `-version` 探测，不用相对路径是否存在；安装/删除后清缓存。 |
| 规则哈希重判 | 规则变更后首轮探测变多 | 受单轮频道上限、流量预算与轮转游标约束。 |
| 兜底 0 被误解为“不限制” | 配置困惑 | UI 提示明确写“0 = 用最低速率”。 |
| Host 缓存跳过分辨率 | 结果不一致 | Full 模式在 `ffprobe=true` 时不走 Host 缓存。 |
| `ffprobe` 安装增大下载与磁盘 | 低配机磁盘压力 | 面板安装是可选操作。低配用户可用系统 PATH 或跳过。 |
| 两个二进制安装一半失败 | 工具不可用 | 任一制品失败则整体失败，清理临时文件。 |

## 11. 留给实现者的决定

- `ffprobe` 状态检查：已定首次请求时用 `-version` 探测并缓存。安装或删除成功后清缓存。
- 安装任务进度：已定单一总进度，总量为两个制品之和。
- 是否在结果详情显示“分辨率来源”。第一版界面不显示，日志记录。
- 兜底速率加入 `GET /panel/api/quality/effective` 的规则快照。已定：加入。
