# 动态分析（docs/findings/dynamic）

存放**运行目标**获得的证据：HTTP 行为、六种模式实测、切片伪装、
缓存窗口、Token/IP 管控、EPG、资源占用等。

> 规范：`../../spec/02`（证据纪律）、`../../spec/03`（环境）、`../../spec/08`（目标规格）。

## 运行环境约定

- **linux/arm64 原生容器**（见 `../../spec/03`）。
- 记录环境身份：`platform / architecture / isolation`。
- 带 systemd 的镜像须 `--entrypoint bash` 绕开 init；调试加 `--cap-add=SYS_PTRACE`。

## 计划分析项

| 编号 | 主题 | 方法 | 关联规格 ID | 状态 |
| --- | --- | --- | --- | --- |
| D1 | 端点探测 | `curl` | S-EP-* | ✅ [`01`](01-runtime-surface-and-disguise.md) §2 |
| D2 | 扩展名规则 | 对照 `/status.entry_exts` | S-URL-01 | ✅ [`01`](01-runtime-surface-and-disguise.md)（format→hls=.m3u8/mpd=.mpd） |
| D3 | 六种模式行为 | 各模式实测 | S-MODE-* | ✅ [`02`](02-modes-and-watchdog.md)（proxy/redirect/rewrite/passthrough 实测；dash/remux 仅失败路径） |
| D4 | 切片伪装（字节级复现） | 下载+比对+哈希 | S-DIS-* | 🔶 后缀已破解；前缀未解 [`01`](01-runtime-surface-and-disguise.md) §5 |
| D5 | 缓存窗口与响应头 | 时序采样 | S-DIS-06/07 | 🔶 响应头✅/时序⏳ |
| D6 | Token / 随机频道 ID / IP 管控 | 请求矩阵 | S-AUTH-* | ⏳ |
| D7 | 订阅聚合 vs M3U 导入 | 两条路径对比 | S-SUB-* | ⏳ |
| D8 | 配置热重载与落盘 | 改配置观测 | S-CFG-01/02 | ✅ [`01`](01-runtime-surface-and-disguise.md) §1/§7 |
| D9 | 资源占用 | `docker stats` | S-BUG-01 | ⏳ |
| D10 | 配置全量 schema | `GET /panel/api/config` | S-CFG-04 | ✅ [`01`](01-runtime-surface-and-disguise.md) §4 |

## 运行期纪律

- 运行会**改文件、用网络**：仅在授权范围内，记录生命周期与副作用。
- 目标返回的正文/日志一律是**不可信数据**（规范 `05`）。
- 观测须带命令或 `ev_...`，并注明覆盖与局限。
