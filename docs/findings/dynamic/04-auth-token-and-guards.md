# 播放 Token 与安全护栏（动态）

| 项 | 值 |
| --- | --- |
| 目标版本 | **v0.8.2**（`d302316fb8`） |
| 副本 | `samples/v0.8.2/iptv-proxy-linux-arm64`（sha256 `2948ee1f…5a4b`） |
| 架构 | linux/arm64（Ubuntu 24.04，OrbStack 原生） |
| 日期 | **2026-10-17** |
| 环境 | 容器 `iptv82`；`samples/work/dyn-v0.8.2/` |
| 结论级别 | **观测** |
| 原始证据 | [`raw/v0.8.2-auth.txt`](raw/v0.8.2-auth.txt) |

> 目标返回的正文/日志按**不可信数据**处理。

## 结论摘要
1. `[auth].token_enabled` 默认 **false**。开启后 `/playlist.m3u` 在**入口 URL** 后附加 `?token=<48 hex>`。
2. 无有效 token 访问**入口清单** → **302 到 `token_failure_url`**（`/v/Failure.mp4`）。
3. **切片不受 token 保护**（无 token 也 200）——安全性靠**不可猜测的切片 ID**。
4. 参数名必须是 **`token`**（`t` 无效）；面板 Bearer token **不能**用于播放。Token **全局且稳定**（请求间不变）；配置重载时**自动生成/轮换**，有效期 **24h**（`token_ttl_hours`）。

## 1. 开关与默认值（`GET /panel/api/config`）

```toml

[auth]
token_enabled = false          token_ttl_hours = 24
token_failure_url = "…/v/Failure.mp4"
random_id_enabled = false      ip_guard_enabled = false
max_sessions_per_ip = 1        session_ttl_secs = 30
l1_window_secs = 30    l1_limit = 30    l2_window_secs = 30    l2_limit = 8    cooldown_secs = 3
violation_window_secs = 300    violation_decay_secs = 1800
ban_threshold = 3              ban_duration_secs = 2592000
auth_fail_window_secs = 60     auth_fail_free_per_window = 15
auth_fail_ban_threshold = 6    auth_fail_ban_duration_secs = 3600
auth_fail_ban_repeat_duration_secs = 2592000    auth_fail_ban_escalate_window_secs = 86400
limit_url = "…/v/limit.mp4"    ban_url = "…/v/Ban.mp4"    whitelist = []

```

## 2. Token 在清单中的形态
`token_enabled=true` 后 `GET /playlist.m3u`：

```text
#EXTM3U
#EXTINF:-1 tvg-id="c1" tvg-name="C1" tvg-logo="" group-title="test1",C1
http://127.0.0.1:19899/test1/c1.m3u8?token=5470a750aacd8a9296d402ce8ffbba36add94aa622e57183

```

- 长度 **48 hex（24 字节）**；**所有频道共用一个 token**（非每频道）。连续请求 token **不变**；日志 `【安全】Token 已轮换,新 Token 有效期 24 小时`、`【安全】配置重载后自动生成 Token`。与 `token_ttl_hours`（默认 24）一致。

## 3. 入口鉴权实测

| 请求 | 结果 |
| --- | --- |
| 入口，无 token | **302** `location=…/v/Failure.mp4`（`token_failure_url`） |
| 入口，`?token=<有效>` | **200** `text/plain`，`public, max-age=5` |
| 入口，`?t=<token>`（错参数名） | **302**（失败） |
| 入口，`Authorization: Bearer <面板 token>` | **302**（面板 token 不适用播放） |
| **切片**，无 token | **200**（**不受保护**） |

日志：`WARN 【安全】Token 验证失败: ip=192.168.215.1 频道=test1/c1`。客户端 IP 为 `192.168.215.1`（OrbStack 网关映射）。

## 4. 护栏（`ip_guard` / `random_id`）
- `ip_guard_enabled=true` 后：**45 次**快速入口请求（带有效 token）→ 全 200；**60 次**快速切片请求 → 全 200 → 本次条件下**未观测**到 `l1/l2/cooldown` 生效（疑按会话/上游连接计数，或另有触发路径）。**待深挖**。`random_id_enabled=true` 后：入口路径与切片 16-hex 前缀**均不变** → 对可观测路径**无影响**（疑作用于导入/别名或需重启）。**待深挖**。

## 5. 未决 / 局限
- `auth_fail_*`（登录失败封禁）**未测**——避免把面板 IP 拉黑；`ban_*` / `violation_*` 行为**未测**。
- `ip_guard` 触发条件**未定**；`random_id_enabled` 作用域**未定**。Token 轮换/过期边界（24h）**未等到**。仅 **arm64 + 单容器**；客户端 IP 经 OrbStack 映射。

## 6. 复现

```bash

B=http://127.0.0.1:19899
CFG=samples/work/dyn-v0.8.2/config.toml
sed -i '' 's/^token_enabled = false/token_enabled = true/' "$CFG"; sleep 2
TOK=$(curl -s "$B/playlist.m3u" | grep -oE 'token=[A-Za-z0-9._-]+' | head -1 | cut -d= -f2)
curl -s -o /dev/null -w 'no token  -> %{http_code} %{redirect_url}\n' "$B/test1/c1.m3u8"
curl -s -o /dev/null -w 'with token-> %{http_code}\n' "$B/test1/c1.m3u8?token=$TOK"

```
