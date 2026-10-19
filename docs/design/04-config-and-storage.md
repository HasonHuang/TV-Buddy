# 04 · 配置与存储设计

> 设计版本 v1.0 ｜ 2026-10-19 ｜ 状态：**初稿**
> 上游：[`03-http-contract.md`](03-http-contract.md) ｜ 判据：[`../spec/08-target-spec.md`](../spec/08-target-spec.md) §11 ｜ 证据：F-006、F-008、F-015、F-013

## 1. 配置文件

| 项 | 设计 | 依据 |
| --- | --- | --- |
| 路径 | 来自启动参数（`iptv-proxy <config.toml>`），默认 `config.toml` | S-ENV-06 |
| 解析 | 严格 TOML：重复键、类型错都报错并退出 | S-EPG-01（F-008 实测） |
| 默认值 | 缺省键用内置默认补齐；默认值对齐目标 v0.8.2 | S-CFG-04 |
| 未建模键 | 存 `extra`（`toml::Table`），保存时原样写回 | 设计决策（避免丢第三方配置） |
| 写回 | 全量规范化 + 原子替换（`config.toml.tmp` → `rename`） | S-CFG-01（F-006） |
| 并发 | 进程内用一把写锁串行化"改内存 + 写盘" | 设计决策 |

### 1.1 段与职责

| 段 | 关键键 | P0 用途 |
| --- | --- | --- |
| `[server]` | `bind`、`not_found_url`、`error_url`、`home_url`、`m3u_header_lines`、`timezone_offset_hours`、`log_level`、`trust_proxy` | 监听、跳转、订阅头行、日志级别 |
| `[panel]` | `enabled`、`username`、`password_sha256`、`session_ttl` | 面板登录 |
| `[cache]` | `backend`、`disk_path`、`m3u8/segment_max_entries`、`disk_max_mb`、`disguise.*` | 缓存与伪装 |
| `[auth]` | `token_enabled`、`token_ttl_hours`、`token_failure_url`、`whitelist` | 播放 Token |
| `[health]`/`[stats]` | `enabled`、`interval_secs`、`online_window_secs` | 健康与指标 |
| `[[groups]]`/`[[groups.channels]]` | 见 `02-architecture.md` §4.1 | 数据面 |

## 2. 热重载矩阵

| 段 | 生效方式 | 依据 |
| --- | --- | --- |
| `[server]`（含 `m3u_header_lines`） | 立即 | S-CFG-02（F-006） |
| `[auth]` | 立即 | S-CFG-02 |
| `[[groups]]`/`[[groups.channels]]` | 立即 | 面板改动的实测行为 |
| `[cache]`（`disk_path`） | 需重启 | 目标同 |
| `[epg]` | **需重启** | F-008 |
| `[panel]` | 立即（新会话按新口令） | 设计决策 |

实现：`notify` 监视配置文件。事件到达后重读；解析失败只记日志、保留旧快照。自身写盘时先登记内容摘要，忽略自己触发的下一次事件（避免写回循环）。

> 竞态：目标存在"文件直改 vs 面板保存"的竞态（面板以内存态回写）。本实现以**文件为唯一事实源**：任何保存都从当前文件重读合并，再写回。这会与目标行为不同（更安全），登记为设计差异。

## 3. 存储布局

```text
<disk_path>/                     （默认 ./cache）
├─ m3u8/          入口与媒体清单缓存
├─ seg_url/       上游切片地址缓存
├─ seg_bytes/     切片正文缓存
├─ sub_url/       订阅源缓存
├─ dash/          DASH 会话（flush 保留）
├─ livestream/    本地回看
├─ recordings/    录制产物 + schedule.json
├─ logos/         台标文件（含 .orig/ 原图备份）
└─ resolvers/     解析脚本缓存
```

| 项 | 设计 | 依据 |
| --- | --- | --- |
| 目录创建 | 仅启动时创建 | F-006 |
| 缓存键 | 清单：`{gid}_{cid}__{sha256(url)[..8]}`；切片：`sha256(上游绝对 URL)` | 内部实现，非兼容契约 |
| 切片伪装 | `<S>` = 同一 sha256 前 32 hex | S-DIS-04 |
| flush | 清 `m3u8`/`seg_url`/`seg_bytes`/`sub_url`，**保留 `dash/`** | S-CACHE-06 |
| 容量上限 | P0 **不实现**（目标上限未生效） | F-015 |
| 索引 | 启动扫描目录重建内存索引 | 设计决策 |

## 4. 运行时数据

| 数据 | 存放 | 生命周期 | 依据 |
| --- | --- | --- | --- |
| 面板会话 | 内存 | 重启失效、TTL `session_ttl` | F-004 |
| 播放 Token | 内存 | 重启失效、TTL `token_ttl_hours`、旧值宽限 ≈1h | S-AUTH-09 |
| IP 封禁 | 内存 | 重启失效、默认 30 天 | S-AUTH-10 |
| 限流窗口 | 内存 | 滑动窗口 | S-GAP-08 |
| 录制任务 | `cache/recordings/schedule.json` | 持久 | F-014 |
| 看门狗状态 | 内存 | 重启重置 | F-005 |

`schedule.json` 形状（F-014）：

```json
{ "version": 1, "tasks": [ { "id": "<uuid>", "name": "…", "group_id": "…", "channel_id": "…",
  "start_time": "2026-10-20T19:00:00", "end_time": "…", "format": "mkv", "status": "pending" } ] }
```

## 5. 日志

| 项 | 设计 | 依据 |
| --- | --- | --- |
| 出口 | 标准输出 + `<安装目录>/iptv-proxy.log` | S-ENV-03 |
| 级别 | `[server].log_level`（`trace`/`debug`/`info`/`warn`/`error`） | S-CACHE-05 |
| 截断 | 文件超过 5 MB 截断 | S-CACHE-05 |
| 格式 | `tracing` 文本，含时间、级别、目标 | 设计决策 |

## 6. 发布布局

```text
/opt/iptv-proxy/
├─ iptv-proxy          静态 musl 二进制
├─ config.toml         首次启动生成默认配置
├─ iptv-proxy.log
├─ cache/
└─ iptv-proxy.sh       启停脚本
```

| 项 | 设计 | 依据 |
| --- | --- | --- |
| 目标平台 | `aarch64-unknown-linux-musl`、`x86_64-unknown-linux-musl` | 需求 |
| 交叉编译 | `cross`（Docker）产出静态二进制 | ADR（E1） |
| 服务 | systemd 单元；Alpine 用 OpenRC | S-ENV-04 |
| 备份（P2） | `tar.gz` 含 `config.toml` + `cache/logos` + `schedule.json`；恢复时原配置另存 `config.toml.bak` | S-BAK-01/02、F-013 §9 |

## 7. 未决问题

| 问题 | 处理 |
| --- | --- |
| 是否复刻目标的"面板保存"规范化格式 | 用 `toml::to_string_pretty`；不追求与目标逐字节相同 |
| 安装脚本是否需要 `uninstall` | S-CLI-02 登记；P2 决定 |
| `disk_path` 变更是否允许热重载 | 当前要求重启（与目标一致） |
