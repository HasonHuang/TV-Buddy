> **2026-10-31 起配置存 SQLite。**
> 数据库是 `<prefix>/tv-buddy.db`。本文里凡是「编辑 `config.toml`」的步骤，改为
> `tv-buddy config set <键> <值>`；凡是把 `config.toml` 当启动参数的，改为
> `tv-buddy <prefix>/tv-buddy.db`。需要一次导入旧配置时运行
> `tv-buddy import-config <config.toml> --db <prefix>/tv-buddy.db`。
> 备份、重装、恢复的载体是同名数据库文件。详见 `PROGRESS.md` §5。

## 1. 目的

这份文档证明一件事：在真实 VPS 上，`[auth]` 的会话上限、两级限流、违规封禁、白名单、登录失败锁、播放 Token 与 `random_id_enabled` 按本实现的规则生效。

范围说明：

- `[auth]` 的语义由本实现自定。`PROGRESS.md` §6.2 的 P2-1、P2-2、P2-3 标注「语义自定」。本文只证明本实现，不证明目标 v0.8.2 的行为。
- 封禁表、会话表、限流表、登录计数、播放 Token、解封配额都在单进程内存。重启即清空。

已知风险（必须写进结论）：

- 本实现无条件信任 `X-Forwarded-For`。取最后一个非空项（`src/http/ext.rs:61`）。
- `[server].trust_proxy` 只有定义（`src/config/model.rs:120`）。全仓库没有读取点。
- 任何客户端都能加 `-H 'X-Forwarded-For: ...'` 伪造地址，绕过按 IP 的限流与封禁。
- 本文所有用例都用 XFF 指定地址。测试依赖的正是这条行为。

## 2. 前置条件

| 项 | 要求 |
| --- | --- |
| 操作系统 | Linux。amd64 或 arm64 |
| 权限 | `root`，或可执行 `sudo` 的账号 |
| 服务 | `tv-buddy` 0.1.0 已安装并在运行 |
| 二进制 | `/opt/tv-buddy/tv-buddy`（`packaging/tv-buddy.service:10`） |
| 配置 | `/opt/tv-buddy/config.toml`（`packaging/tv-buddy.service:10`） |
| 监听 | `[::]:19899`（`src/config/model.rs:130`） |
| 服务单元 | `tv-buddy`（`packaging/tv-buddy.sh:29`） |
| 面板账号 | `admin` / `admin`（默认，`src/config/model.rs:162`、`src/config/model.rs:164`） |
| 工具 | `curl`、`jq` |
| 测试分组 | 至少 2 个频道。分组 id 与频道 id 只用 ASCII 字母、数字、`_`、`-` |
| 分组访问级别 | `access` 不是 `open`。否则 Token 校验被跳过（`src/http/routes/stream.rs:1500`） |
| 上游 | 可播放，或至少能返回响应。判分只看状态码与响应头 |
| 本机访问 | 能访问 `127.0.0.1:19899` |

安装工具：

```bash
sudo apt-get update
sudo apt-get install -y curl jq
```

确认服务在跑：

```bash
systemctl is-active tv-buddy
```

预期输出：`active`。

确认版本。程序不认 `--version`，改用面板接口：

```bash
T=$(curl -sS -X POST http://127.0.0.1:19899/panel/api/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
curl -sS -H "Authorization: Bearer $T" http://127.0.0.1:19899/panel/api/status \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["version"])'
```

预期输出 `0.1.0`。

默认配置下 `ip_guard_enabled = false`（`src/config/model.rs:733`）。此时限流与会话上限立即返回 `None`（`src/http/routes/stream.rs:1433`、`src/http/routes/stream.rs:1478`）。步骤 3 打开开关。

## 3. 步骤

#### 步骤 1：登录面板并备份 `[auth]`

```bash
BASE=http://127.0.0.1:19899
LOGIN=$(curl -s -X POST "$BASE/panel/api/login" \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}')
TOKEN=$(printf '%s' "$LOGIN" | jq -r .token)
AUTH="Authorization: Bearer $TOKEN"
printf '%s\n' "$LOGIN"
```

预期：`200`，正文 `{"token":"<uuid>"}`（`src/http/routes/panel.rs:91`）。

口令不是 `admin` 时，改 `-d` 里的 `password`。

备份当前 `[auth]`：

```bash
curl -s -H "$AUTH" "$BASE/panel/api/config" | jq '.auth' > /tmp/tv-buddy-auth-backup.json
cat /tmp/tv-buddy-auth-backup.json
```

预期：输出一个 JSON 对象，含全部 `[auth]` 键。

定义改配置的辅助函数：

```bash
set_auth() {
  curl -s -H "$AUTH" "$BASE/panel/api/config" \
    | jq --argjson patch "$1" '.auth |= (. + $patch)' \
    | curl -s -X PUT -H "$AUTH" -H 'Content-Type: application/json' \
        --data-binary @- "$BASE/panel/api/config"
}
```

预期：每次调用输出 `{"ok":true}`（`src/http/routes/panel.rs:195`）。

说明：

- `PUT /panel/api/config` 写盘并立即生效（`src/http/routes/panel.rs:186`）。不需要重启，也不需要等热重载。
- 改配置不清空内存里的封禁、会话、限流、登录失败计数。`replace` 只重置 watchdog、切片、订阅、缓存（`src/state.rs:210`）。
- 因此用例之间用不同 IP，避免互相污染。

取分组与频道 id：

```bash
GID=$(curl -s -H "$AUTH" "$BASE/panel/api/config" | jq -r '.groups[0].id')
CID=$(curl -s -H "$AUTH" "$BASE/panel/api/config" | jq -r '.groups[0].channels[0].id')
CID2=$(curl -s -H "$AUTH" "$BASE/panel/api/config" | jq -r '.groups[0].channels[1].id')
printf 'GID=%s CID=%s CID2=%s\n' "$GID" "$CID" "$CID2"
```

预期：三个值都非空。`CID2` 为空时换一个含 2 个以上频道的分组。

定义限流用的不存在频道：

```bash
CIDX="${CID}-missing"
```

限流与会话判定在频道解析之前执行（`src/http/routes/stream.rs:88`、`src/http/routes/stream.rs:359`）。因此不存在的频道也能触发判定，且不依赖上游。

#### 步骤 2：确认默认关闭时不限流

```bash
set_auth '{"ip_guard_enabled":false,"token_enabled":false,"whitelist":[]}'
for i in $(seq 1 15); do
  curl -s -o /dev/null -w '%{http_code}\n' \
    -H 'X-Forwarded-For: 198.51.100.10' \
    "$BASE/$GID/$CIDX.m3u8"
done | sort | uniq -c
```

预期：不出现 `429`，也不出现 `403`。`sys.stdout` 里允许 `302`、`502` 等其他状态码。

#### 步骤 3：打开 IP 管控并写入基线参数

```bash
set_auth '{"ip_guard_enabled":true,"token_enabled":false,"whitelist":[],"max_sessions_per_ip":1,"session_ttl_secs":30,"l1_window_secs":30,"l1_limit":30,"l2_window_secs":30,"l2_limit":8,"cooldown_secs":3,"violation_window_secs":300,"violation_decay_secs":1800,"ban_threshold":3,"ban_duration_secs":2592000}'
```

预期：`{"ok":true}`。

#### 步骤 4：二级限流（突发上限 8 次 / 30 秒）

```bash
for i in $(seq 1 12); do
  printf '%02d ' "$i"
  curl -s -o /dev/null -D - \
    -H 'X-Forwarded-For: 203.0.113.10' \
    "$BASE/$GID/$CIDX.m3u8" \
    | awk 'NR==1{printf "%s ",$2} tolower($1)=="retry-after:"{printf "Retry-After=%s",$2} END{print ""}'
  sleep 3
done
```

耗时约 36 秒。

预期：

| 请求序号 | 状态码 | 说明 |
| --- | --- | --- |
| 01–08 | `302` | 未超限 |
| 09 | `429` | 第 1 次超限，`Retry-After: 3` |
| 10 | `429` | 冷却已过，第 2 次超限，`Retry-After` 为 1–3 |
| 11 | `403` | 第 3 次超限。正文 `banned` |
| 12 | `403` | 已封禁，继续 `403` |

循环跑 12 次是为了确认封禁持续。第 11、12 次的正文都是 `banned`。

依据：

- L2 阈值 `l2_limit`，窗口 `l2_window_secs`（`src/auth/rate_limit.rs:45`）。
- 比较是 `len > limit`（`src/auth/rate_limit.rs:102`）。
- 触发后设冷却 `now + cooldown_secs`（`src/auth/rate_limit.rs:108`）。
- 触发响应带 `Retry-After`（`src/http/routes/stream.rs:1453`）。
- 冷却内请求返回剩余秒数，最小 1（`src/auth/rate_limit.rs:87`）。
- 有效违规达到 `ban_threshold` 返回 `Ban`（`src/auth/rate_limit.rs:112`）。
- 调用方封禁 `ban_duration_secs`，并重置该 IP 的 L1/L2/违规/冷却（`src/http/routes/stream.rs:1460`、`src/http/routes/stream.rs:1461`）。
- 封禁响应固定 `403`，正文 `banned`。不跳 `ban_url`（`src/http/routes/stream.rs:1462`）。

第 11 次的正文：

```bash
curl -s -H 'X-Forwarded-For: 203.0.113.10' "$BASE/$GID/$CIDX.m3u8"
echo
```

预期输出：`banned`。

查看统计：

```bash
curl -s -H "$AUTH" "$BASE/panel/api/security/ip/stats" \
  | jq '.entries[] | select(.ip=="203.0.113.10")'
```

预期：`banned: true`，`remaining_secs` 大于 2500000，`requests` 不小于 12。

清理：

```bash
curl -s -X POST -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"ip":"203.0.113.10"}' \
  "$BASE/panel/api/security/ip/unban"
```

预期：`{"ok":true,"removed":1,"requested":1}`（`src/http/routes/security.rs:206`）。

#### 步骤 5：一级限流（持续上限，L2 关闭）

```bash
set_auth '{"ip_guard_enabled":true,"whitelist":[],"l1_limit":2,"l2_limit":0,"cooldown_secs":3}'
for i in 1 2 3; do
  printf '%02d ' "$i"
  curl -s -o /dev/null -D - \
    -H 'X-Forwarded-For: 203.0.113.11' \
    "$BASE/$GID/$CIDX.m3u8" \
    | awk 'NR==1{printf "%s ",$2} tolower($1)=="retry-after:"{printf "Retry-After=%s",$2} END{print ""}'
done
```

预期：

- 01：不是 `429`。
- 02：不是 `429`。
- 03：`429`，`Retry-After: 3`。

依据：`l1_limit = 0` 关闭 L1，`l2_limit = 0` 关闭 L2（`src/auth/rate_limit.rs:101`）。窗口秒数经 `.max(1)`（`src/http/routes/stream.rs:1441`、`src/http/routes/stream.rs:1443`）。

#### 步骤 6：播放会话上限（1 个 / 30 秒）

```bash
set_auth '{"ip_guard_enabled":true,"whitelist":[],"max_sessions_per_ip":1,"session_ttl_secs":30,"l1_limit":30,"l2_limit":8}'
```

定义播放函数：

```bash
play() {
  curl -s -o /dev/null -D - \
    -H 'X-Forwarded-For: 203.0.113.20' \
    "$BASE/$GID/$1.m3u8" \
    | awk 'NR==1{printf "status=%s ",$2} tolower($1)=="location:"{printf "location=%s",$2} END{print ""}'
}
printf 'A1 '; play "$CID"
printf 'B1 '; play "$CID2"
printf 'A2 '; play "$CID"
printf 'B2 '; play "$CID2"
```

预期：

| 行 | 结果 |
| --- | --- |
| A1 | 不是 `302` 到 `limit_url` |
| B1 | 不是 `302` 到 `limit_url`。A 被踢 |
| A2 | `302`，`location` 等于 `[auth].limit_url` |
| B2 | 不是 `302` 到 `limit_url` |

`limit_url` 默认值：`https://material.yang-1989.eu.org/v/limit.mp4`（`src/config/model.rs:736`）。

依据：

- 一个会话是 `(客户端 IP, 频道 id)`（`src/auth/session_guard.rs:9`）。
- 新频道始终放行，被踢的是最久未用的旧频道（`src/auth/session_guard.rs:92`）。
- 被踢的 `(IP, 频道)` 记入限流表，有效期 `session_ttl_secs`（`src/auth/session_guard.rs:96`）。
- 有效期内再请求返回 `Verdict::Limit`，转 `302 limit_url`（`src/http/routes/stream.rs:1489`、`src/http/routes/stream.rs:1537`）。
- 换回当前频道仍放行（`src/auth/session_guard.rs:86`）。

`limit_url` 被 `[assets].map` 改写时，`location` 是改写后的地址（`src/http/routes/stream.rs:1543`）。默认配置没有改写。

#### 步骤 6.1：`max_sessions_per_ip = 0` 等价于 1

```bash
set_auth '{"max_sessions_per_ip":0}'
play0() {
  curl -s -o /dev/null -D - \
    -H 'X-Forwarded-For: 203.0.113.21' \
    "$BASE/$GID/$1.m3u8" \
    | awk 'NR==1{printf "status=%s ",$2} tolower($1)=="location:"{printf "location=%s",$2} END{print ""}'
}
printf 'A1 '; play0 "$CID"
printf 'B1 '; play0 "$CID2"
printf 'A2 '; play0 "$CID"
```

预期：A2 为 `302`，`location` 等于 `limit_url`。

依据：`max_sessions_per_ip` 经 `.max(1)`（`src/http/routes/stream.rs:1486`）。配置注释写「0 = 不限」（`src/config/model.rs:671`），代码不成立。

#### 步骤 7：白名单

白名单支持三种写法：`*`、精确地址、IPv4 CIDR（`src/auth/session_guard.rs:161`、`src/auth/session_guard.rs:165`）。IPv6 只支持精确匹配（`src/auth/session_guard.rs:170`）。规则写错不报错，按不命中处理（`src/auth/session_guard.rs:174`）。

##### 7.1 精确地址

```bash
set_auth '{"ip_guard_enabled":true,"whitelist":["203.0.113.30"],"l1_limit":30,"l2_limit":8,"ban_threshold":3,"cooldown_secs":3}'
for i in $(seq 1 12); do
  curl -s -o /dev/null -w '%{http_code}\n' \
    -H 'X-Forwarded-For: 203.0.113.30' \
    "$BASE/$GID/$CIDX.m3u8"
done | sort | uniq -c
```

预期：不出现 `429`，也不出现 `403`。

##### 7.2 IPv4 CIDR

```bash
set_auth '{"whitelist":["203.0.113.0/24"]}'
for i in $(seq 1 10); do
  curl -s -o /dev/null -w '%{http_code}\n' \
    -H 'X-Forwarded-For: 203.0.113.31' \
    "$BASE/$GID/$CIDX.m3u8"
done | sort | uniq -c
```

预期：不出现 `429`。

##### 7.3 通配 `*`

```bash
set_auth '{"whitelist":["*"]}'
for i in $(seq 1 10); do
  curl -s -o /dev/null -w '%{http_code}\n' \
    -H 'X-Forwarded-For: 198.51.100.1' \
    "$BASE/$GID/$CIDX.m3u8"
done | sort | uniq -c
```

预期：不出现 `429`。

##### 7.4 IPv6 精确地址

```bash
set_auth '{"whitelist":["2001:db8::1"]}'
for i in $(seq 1 10); do
  curl -s -o /dev/null -w '%{http_code}\n' \
    -H 'X-Forwarded-For: 2001:db8::1' \
    "$BASE/$GID/$CIDX.m3u8"
done | sort | uniq -c
```

预期：不出现 `429`。

##### 7.5 IPv6 CIDR 不生效

```bash
set_auth '{"whitelist":["2001:db8::/32"]}'
for i in $(seq 1 10); do
  curl -s -o /dev/null -w '%{http_code}\n' \
    -H 'X-Forwarded-For: 2001:db8::2' \
    "$BASE/$GID/$CIDX.m3u8"
done | sort | uniq -c
```

预期：出现 `429`。这证明 IPv6 CIDR 未生效。

依据：`cidr_match` 只解析 `Ipv4Addr`（`src/auth/session_guard.rs:170`）。

##### 7.6 错误 CIDR 不生效

```bash
set_auth '{"whitelist":["203.0.113.0/33"]}'
for i in $(seq 1 10); do
  curl -s -o /dev/null -w '%{http_code}\n' \
    -H 'X-Forwarded-For: 203.0.113.32' \
    "$BASE/$GID/$CIDX.m3u8"
done | sort | uniq -c
```

预期：出现 `429`。

依据：前缀大于 32 返回 `false`（`src/auth/session_guard.rs:176`）。

##### 7.7 白名单不豁免登录锁

```bash
set_auth '{"ip_guard_enabled":true,"whitelist":["203.0.113.33"],"auth_fail_free_per_window":0,"auth_fail_ban_threshold":3,"auth_fail_lock_secs":300}'
for i in 1 2 3; do
  curl -s -o /dev/null -X POST "$BASE/panel/api/login" \
    -H 'Content-Type: application/json' \
    -H 'X-Forwarded-For: 203.0.113.33' \
    -d '{"username":"admin","password":"wrong"}'
done
curl -s -w '\n%{http_code}\n' -X POST "$BASE/panel/api/login" \
  -H 'Content-Type: application/json' \
  -H 'X-Forwarded-For: 203.0.113.33' \
  -d '{"username":"admin","password":"wrong"}'
```

预期：最后一次 `429`，正文含 `locked_for_secs: 300`。

依据：登录锁没有白名单判断（`src/http/routes/panel.rs:52`）。

#### 步骤 8：登录失败阶梯锁

##### 8.1 缩短参数

```bash
set_auth '{"ip_guard_enabled":true,"whitelist":[],"auth_fail_window_secs":60,"auth_fail_free_per_window":0,"auth_fail_ban_threshold":3,"auth_fail_lock_secs":2,"auth_fail_ban_duration_secs":4,"auth_fail_ban_repeat_duration_secs":6,"auth_fail_ban_escalate_window_secs":60}'
```

定义登录函数：

```bash
try_login() {
  curl -s -o /dev/null -w '%{http_code}\n' -X POST "$BASE/panel/api/login" \
    -H 'Content-Type: application/json' \
    -H "X-Forwarded-For: $1" \
    -d '{"username":"admin","password":"wrong"}'
}
locked() {
  curl -s -X POST "$BASE/panel/api/login" \
    -H 'Content-Type: application/json' \
    -H "X-Forwarded-For: $1" \
    -d '{"username":"admin","password":"wrong"}'
  echo
}
```

第 1 次锁：

```bash
for i in 1 2 3; do try_login 203.0.113.77; done
locked 203.0.113.77
```

预期：前三行 `401`。最后一行 `{"error":"登录失败次数过多，请 2s 后再试","locked_for_secs":2}`。

第 2 次锁：

```bash
sleep 3
for i in 1 2 3; do try_login 203.0.113.77 >/dev/null; done
locked 203.0.113.77
```

预期：`locked_for_secs` 为 `4`。

第 3 次锁：

```bash
sleep 5
for i in 1 2 3; do try_login 203.0.113.77 >/dev/null; done
locked 203.0.113.77
```

预期：`locked_for_secs` 为 `6`。

依据：

- 阈值是 `auth_fail_free_per_window + auth_fail_ban_threshold`（`src/auth/login_guard.rs:88`）。
- 第 1 次锁用 `auth_fail_lock_secs`（`src/http/routes/panel.rs:58`）。
- 第 2 次锁用 `auth_fail_ban_duration_secs`（`src/auth/login_guard.rs:97`）。
- 第 3 次及以后用 `auth_fail_ban_repeat_duration_secs`（`src/auth/login_guard.rs:98`）。
- 阶梯窗口 `auth_fail_ban_escalate_window_secs` 外的历史锁不再计数（`src/auth/login_guard.rs:94`）。
- 判定在校验口令之前（`src/http/routes/panel.rs:52`、`src/http/routes/panel.rs:79`）。

检查响应头：

```bash
curl -s -D - -o /dev/null -X POST "$BASE/panel/api/login" \
  -H 'Content-Type: application/json' \
  -H 'X-Forwarded-For: 203.0.113.77' \
  -d '{"username":"admin","password":"wrong"}' \
  | grep -ci '^retry-after:' || true
```

预期输出：`0`。登录锁没有 `Retry-After` 头（`src/http/routes/panel.rs:95`）。

锁定期间既有会话仍可用：

```bash
curl -s -o /dev/null -w '%{http_code}\n' -H "$AUTH" "$BASE/panel/api/status"
```

预期：`200`。

依据：会话校验独立于登录锁（`src/http/ext.rs:40`、`src/auth/session.rs:43`）。

##### 8.2 默认阈值

```bash
set_auth '{"auth_fail_free_per_window":15,"auth_fail_ban_threshold":6,"auth_fail_lock_secs":300,"auth_fail_ban_duration_secs":3600,"auth_fail_ban_repeat_duration_secs":2592000,"auth_fail_ban_escalate_window_secs":86400}'
for i in $(seq 1 21); do try_login 203.0.113.78 >/dev/null; done
locked 203.0.113.78
```

预期：`locked_for_secs` 为 `300`。

依据：默认阈值 15 + 6 = 21 次（`src/config/model.rs:740`、`src/config/model.rs:741`）。第 22 次请求直接锁定。

##### 8.3 关闭 IP 管控时锁不升级

```bash
set_auth '{"ip_guard_enabled":false,"auth_fail_free_per_window":0,"auth_fail_ban_threshold":3,"auth_fail_lock_secs":2,"auth_fail_ban_duration_secs":4,"auth_fail_ban_repeat_duration_secs":6,"auth_fail_ban_escalate_window_secs":60}'
for i in 1 2 3; do try_login 203.0.113.79 >/dev/null; done
locked 203.0.113.79
sleep 3
for i in 1 2 3; do try_login 203.0.113.79 >/dev/null; done
locked 203.0.113.79
```

预期：两次都是 `locked_for_secs: 2`，第二次不升到 4。

依据：关闭时用固定时长 `auth_fail_lock_secs`（`src/http/routes/panel.rs:64`、`src/http/routes/panel.rs:69`）。

#### 步骤 9：手动封禁、IP 统计与自助解封

##### 9.1 手动封禁不依赖开关

```bash
set_auth '{"ip_guard_enabled":false,"whitelist":[]}'
curl -s -X POST -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"ip":"203.0.113.50","secs":60}' \
  "$BASE/panel/api/security/ip/ban" \
  | jq '{ok, secs, max_seconds, default_seconds}'
```

预期：`{"ok":true,"secs":60,"max_seconds":2592000,"default_seconds":3600}`。

```bash
curl -s -o /dev/null -w '%{http_code}\n' -H 'X-Forwarded-For: 203.0.113.50' "$BASE/$GID/$CID.m3u8"
curl -s -H 'X-Forwarded-For: 203.0.113.50' "$BASE/$GID/$CID.m3u8"
echo
```

预期：`403`，正文 `banned`。

依据：中间件总是检查封禁表（`src/http/guard.rs:35`）。`ip_guard_enabled` 只决定白名单豁免是否生效。

##### 9.2 关闭开关不清除封禁

```bash
set_auth '{"ip_guard_enabled":true}'
set_auth '{"ip_guard_enabled":false}'
curl -s -o /dev/null -w '%{http_code}\n' -H 'X-Forwarded-For: 203.0.113.50' "$BASE/$GID/$CID.m3u8"
```

预期：`403`。

依据：改配置不清空 `ip_guard`（`src/state.rs:210`）。

##### 9.3 白名单地址被手动封禁后仍放行

```bash
set_auth '{"ip_guard_enabled":true,"whitelist":["203.0.113.52"]}'
curl -s -X POST -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"ip":"203.0.113.52","secs":60}' \
  "$BASE/panel/api/security/ip/ban" >/dev/null
curl -s -H "$AUTH" "$BASE/panel/api/security/ip/stats" \
  | jq '.entries[] | select(.ip=="203.0.113.52") | {banned, remaining_secs}'
curl -s -o /dev/null -w '%{http_code}\n' -H 'X-Forwarded-For: 203.0.113.52' "$BASE/$GID/$CID.m3u8"
```

预期：

- 统计里 `banned` 为 `true`。
- 播放请求不是 `403`。

依据：封禁 API 接受任意地址（`src/http/routes/security.rs:164`）。中间件对白名单地址豁免（`src/http/guard.rs:33`、`src/http/guard.rs:34`）。这是本实现的边界行为。

清理：

```bash
for ip in 203.0.113.50 203.0.113.52; do
  curl -s -X POST -H "$AUTH" -H 'Content-Type: application/json' \
    -d "{\"ip\":\"$ip\"}" \
    "$BASE/panel/api/security/ip/unban"
  echo
done
```

预期：两次都返回 `{"ok":true,"removed":1,"requested":1}`。

##### 9.4 `whoami` 与自助解封配额

关闭时：

```bash
set_auth '{"ip_guard_enabled":false}'
curl -s -H 'X-Forwarded-For: 203.0.113.60' "$BASE/unban/whoami"
echo
curl -s -X POST -H 'X-Forwarded-For: 203.0.113.60' "$BASE/unban"
echo
```

预期：

- `{"enabled":false,"ip":"203.0.113.60"}`
- `{"msg":"IP 管控未启用","ok":false}`

依据：`src/http/routes/public.rs:69`、`src/http/routes/public.rs:92`。

打开时：

```bash
set_auth '{"ip_guard_enabled":true,"whitelist":[]}'
curl -s -H 'X-Forwarded-For: 203.0.113.60' "$BASE/unban/whoami"
echo
```

预期：`{"banned":false,"enabled":true,"ip":"203.0.113.60","max_per_month":3,"remaining":3,"used":0}`。

配额循环：

```bash
for n in 1 2 3 4; do
  curl -s -X POST -H "$AUTH" -H 'Content-Type: application/json' \
    -d '{"ip":"203.0.113.60","secs":60}' \
    "$BASE/panel/api/security/ip/ban" >/dev/null
  printf 'round %s: ' "$n"
  curl -s -X POST -H 'X-Forwarded-For: 203.0.113.60' "$BASE/unban"
  echo
done
```

预期：

| 轮次 | 响应 |
| --- | --- |
| 1 | `{"msg":"解封成功","ok":true}` |
| 2 | `{"msg":"解封成功","ok":true}` |
| 3 | `{"msg":"解封成功","ok":true}` |
| 4 | `{"msg":"本月自助解封次数已用完（每月最多3次）","ok":false}` |

依据：配额每 IP 3 次 / 30 天（`src/auth/unban.rs:13`、`src/auth/unban.rs:15`）。

```bash
curl -s -H 'X-Forwarded-For: 203.0.113.60' "$BASE/unban/whoami"
echo
```

预期：`used` 为 3，`remaining` 为 0，`banned` 为 `true`。

清理：

```bash
curl -s -X POST -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"ip":"203.0.113.60"}' \
  "$BASE/panel/api/security/ip/unban"
echo
```

预期：`{"ok":true,"removed":1,"requested":1}`。面板解封不消耗自助配额。

`/unban` 与 `/unban/whoami` 不受封禁中间件拦截（`src/http/guard.rs:23`）。

#### 步骤 10：播放 Token

##### 10.1 关闭时

```bash
set_auth '{"ip_guard_enabled":false,"token_enabled":false}'
curl -s -H "$AUTH" "$BASE/panel/api/link-info" | jq '{token, token_enabled}'
```

预期：`{"token":"","token_enabled":false}`（`src/http/routes/security.rs:30`）。

##### 10.2 打开并取 Token

```bash
set_auth '{"ip_guard_enabled":false,"token_enabled":true,"token_ttl_hours":24}'
curl -s -H "$AUTH" "$BASE/panel/api/link-info" | jq '{token, token_enabled}'
PT=$(curl -s -H "$AUTH" "$BASE/panel/api/link-info" | jq -r .token)
printf 'len=%s\n' "${#PT}"
printf '%s\n' "$PT" | grep -Eq '^[0-9a-f]{48}$' && echo shape-ok || echo shape-bad
```

预期：`token_enabled` 为 `true`，`len=48`，输出 `shape-ok`。

依据：48 位小写十六进制（`src/auth/play_token.rs:121`、`src/auth/play_token.rs:128`）。

##### 10.3 无 Token 播放

```bash
curl -s -o /dev/null -D - -H 'X-Forwarded-For: 203.0.113.40' \
  "$BASE/$GID/$CID.m3u8" \
  | awk 'NR==1{print "status",$2} tolower($1)=="location:"{print "location",$2}'
```

预期：

- `status 302`
- `location https://material.yang-1989.eu.org/v/Failure.mp4`

依据：Token 无效转 `token_failure_url`（`src/http/routes/stream.rs:119`）。默认值见 `src/config/model.rs:731`。

##### 10.4 带 Token 播放

```bash
curl -s -o /dev/null -D - -H 'X-Forwarded-For: 203.0.113.40' \
  "$BASE/$GID/$CID.m3u8?token=$PT" \
  | awk 'NR==1{print "status",$2} tolower($1)=="location:"{print "location",$2}'
```

预期：`location` 不是 `Failure.mp4`。状态码可以是 `200`、`302` 到上游，或上游失败码。

##### 10.5 轮换与旧值宽限

```bash
OLD=$PT
ROT=$(curl -s -X POST -H "$AUTH" "$BASE/panel/api/security/token/rotate")
printf '%s\n' "$ROT" | jq '{ok, expires_in_hours}'
NEW=$(printf '%s' "$ROT" | jq -r .new_token)
printf 'OLD=%s NEW=%s len_old=%s len_new=%s\n' "$OLD" "$NEW" "${#OLD}" "${#NEW}"
test "$OLD" != "$NEW" && echo differ-ok
```

预期：`ok` 为 `true`，`expires_in_hours` 为 `24`，两个长度都是 48，输出 `differ-ok`。

```bash
for t in "$OLD" "$NEW"; do
  printf '%s ' "${t:0:8}"
  curl -s -o /dev/null -D - -H 'X-Forwarded-For: 203.0.113.41' \
    "$BASE/$GID/$CID.m3u8?token=$t" \
    | awk 'NR==1{printf "status=%s ",$2} tolower($1)=="location:"{printf "location=%s",$2} END{print ""}'
done
```

预期：两次的 `location` 都不是 `Failure.mp4`。

```bash
curl -s -H "$AUTH" "$BASE/panel/api/security/token" | jq '{enabled, status}'
```

预期：

- `status.old.valid` 为 `true`。
- `status.old.remaining_secs` 不大于 3600，且大于 3400。

依据：

- 旧值宽限固定 3600 秒（`src/auth/play_token.rs:14`）。
- 宽限期内的旧值有效（`src/auth/play_token.rs:59`）。
- `status.*.value` 是掩码，`current_value` 是完整值（`src/http/routes/security.rs:56`、`src/http/routes/security.rs:62`）。

注意：`token_ttl_hours` 不参与校验。它只用于展示与轮换响应（`src/auth/play_token.rs:101`、`src/http/routes/security.rs:110`）。不能期待 24 小时后 Token 失效。

##### 10.6 重启后旧值失效

```bash
sudo tv-buddy restart
sleep 3
LOGIN=$(curl -s -X POST "$BASE/panel/api/login" \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}')
TOKEN=$(printf '%s' "$LOGIN" | jq -r .token)
AUTH="Authorization: Bearer $TOKEN"
for t in "$OLD" "$NEW"; do
  printf '%s ' "${t:0:8}"
  curl -s -o /dev/null -D - -H 'X-Forwarded-For: 203.0.113.41' \
    "$BASE/$GID/$CID.m3u8?token=$t" \
    | awk 'NR==1{printf "status=%s ",$2} tolower($1)=="location:"{printf "location=%s",$2} END{print ""}'
done
```

预期：两次都是 `302`，`location` 为 `Failure.mp4`。

依据：Token 存内存，进程重启即换新（`src/auth/play_token.rs:4`）。

##### 10.7 1 小时宽限到期

未验证。原因：宽限期是常量 `GRACE = 3600`（`src/auth/play_token.rs:14`），`[auth]` 没有对应键。要缩短只能改源码后重建。

#### 步骤 11：`random_id_enabled`

##### 11.1 开启

```bash
set_auth '{"ip_guard_enabled":false,"token_enabled":false,"random_id_enabled":true}'
curl -s -X POST -H "$AUTH" -H 'Content-Type: application/json' \
  "$BASE/panel/api/groups/import/m3u" \
  -d '{"text":"#EXTM3U\n#EXTINF:-1 tvg-id=\"cctv1\" group-title=\"rid-test\",CCTV1\nhttp://127.0.0.1:9/x.m3u8\n","mode":"append","group_mode":"redirect"}'
echo
curl -s -H "$AUTH" "$BASE/panel/api/config" \
  | jq -r '.groups[] | select(.name=="rid-test") | .channels[].id'
```

预期：

- 导入响应 `ok` 为 `true`，`added_groups` 为 `1`。
- 频道 id 匹配 `^r-[0-9a-f]{8}$`，长度 10。

依据：`r-<8hex>`（`src/m3u.rs:188`）。导入时忽略 `tvg-id`（`src/http/routes/groups.rs:379`）。

##### 11.2 关闭对照

```bash
set_auth '{"random_id_enabled":false}'
curl -s -X POST -H "$AUTH" -H 'Content-Type: application/json' \
  "$BASE/panel/api/groups/import/m3u" \
  -d '{"text":"#EXTM3U\n#EXTINF:-1 tvg-id=\"cctv2\" group-title=\"rid-test2\",CCTV2\nhttp://127.0.0.1:9/y.m3u8\n","mode":"append","group_mode":"redirect"}'
echo
curl -s -H "$AUTH" "$BASE/panel/api/config" \
  | jq -r '.groups[] | select(.name=="rid-test2") | .channels[].id'
```

预期：输出 `cctv2`（`src/m3u.rs:169`）。

##### 11.3 清理

```bash
curl -s -X DELETE -H "$AUTH" "$BASE/panel/api/groups/rid-test"
echo
curl -s -X DELETE -H "$AUTH" "$BASE/panel/api/groups/rid-test2"
echo
```

预期：两次都是 `{"ok":true}`。

#### 步骤 12：恢复配置

```bash
curl -s -H "$AUTH" "$BASE/panel/api/config" \
  | jq --slurpfile a /tmp/tv-buddy-auth-backup.json '.auth = $a[0]' \
  | curl -s -X PUT -H "$AUTH" -H 'Content-Type: application/json' \
      --data-binary @- "$BASE/panel/api/config"
echo
sudo tv-buddy restart
sleep 3
systemctl is-active tv-buddy
```

预期：`{"ok":true}`，服务状态 `active`。

重启清空内存里的封禁、会话、登录锁、解封配额、播放 Token。

## 4. 判定

| 编号 | 检查 | 通过 | 不通过 |
| --- | --- | --- | --- |
| SEC-01 | 默认关闭时不限流 | 15 次请求没有 `429`、没有 `403` | 出现 `429` 或 `403` |
| SEC-02 | L2 第 9 次触发 | 第 9 次为 `429` | 第 9 次不是 `429` |
| SEC-03 | L2 冷却头 | 第 9 次带 `Retry-After: 3` | 缺少该头，或值不是 3 |
| SEC-04 | 违规升级封禁 | 第 3 次超限返回 `403`，正文 `banned` | 第 3 次超限仍未封禁 |
| SEC-05 | 封禁写入统计 | 统计 `banned` 为 `true`，`remaining_secs` 大于 2500000 | 统计无该 IP，或未封禁 |
| SEC-06 | L1 持续限流 | 第 3 次为 `429` | 第 3 次不是 `429` |
| SEC-07 | 会话踢旧频道 | A2 为 `302`，`location` 等于 `limit_url` | A2 不是 `302 limit_url` |
| SEC-08 | 当前频道放行 | B2 不是 `302 limit_url` | B2 被判 `Limit` |
| SEC-09 | `max_sessions_per_ip = 0` | A2 为 `302 limit_url` | A2 放行 |
| SEC-10 | 白名单精确 | 12 次请求无 `429`、无 `403` | 出现 `429` 或 `403` |
| SEC-11 | 白名单 IPv4 CIDR | 10 次请求无 `429` | 出现 `429` |
| SEC-12 | 白名单通配 | 10 次请求无 `429` | 出现 `429` |
| SEC-13 | 白名单 IPv6 精确 | 10 次请求无 `429` | 出现 `429` |
| SEC-14 | IPv6 CIDR 不生效 | 出现 `429` | 全程无 `429` |
| SEC-15 | 错误 CIDR 不生效 | 出现 `429` | 全程无 `429` |
| SEC-16 | 白名单不豁免登录锁 | 第 4 次登录为 `429` | 第 4 次不是 `429` |
| SEC-17 | 第 1 次锁时长 | `locked_for_secs` 为 2 | 其他值 |
| SEC-18 | 第 2 次锁时长 | `locked_for_secs` 为 4 | 其他值 |
| SEC-19 | 第 3 次锁时长 | `locked_for_secs` 为 6 | 其他值 |
| SEC-20 | 登录锁无 `Retry-After` | `grep -c` 输出 `0` | 输出大于 0 |
| SEC-21 | 锁不影响既有会话 | `GET /panel/api/status` 为 `200` | 返回 `401` |
| SEC-22 | 默认阈值 | 第 22 次 `429`，`locked_for_secs` 为 300 | 其他结果 |
| SEC-23 | 关闭时锁不升级 | 两次都是 2 秒 | 第二次为 4 秒 |
| SEC-24 | 手动封禁 | `403`，正文 `banned` | 其他状态码 |
| SEC-25 | 关闭开关不解除封禁 | 仍为 `403` | 变为放行 |
| SEC-26 | 白名单地址被封禁仍放行 | 统计 `banned=true`，播放不是 `403` | 播放为 `403`，或统计无记录 |
| SEC-27 | `whoami` 两种状态 | 关闭时只有 `enabled` 与 `ip`；开启时有配额字段 | 字段不符 |
| SEC-28 | 自助解封 | 前 3 轮 `ok=true` | 任一轮失败 |
| SEC-29 | 解封配额 | 第 4 轮 `ok=false`，`used=3`、`remaining=0` | 第 4 轮成功，或计数不符 |
| SEC-30 | Token 关闭 | `token` 为空串 | 返回非空 Token |
| SEC-31 | 无 Token 播放 | `302` 到 `Failure.mp4` | 其他结果 |
| SEC-32 | 带 Token 播放 | 不跳 `Failure.mp4` | 跳 `Failure.mp4` |
| SEC-33 | 轮换后旧值有效 | 旧值与新值都不跳 `Failure.mp4` | 旧值被拒 |
| SEC-34 | 旧值剩余时间 | 不大于 3600，且大于 3400 | 超出范围 |
| SEC-35 | 重启后旧值失效 | 旧值与新值都跳 `Failure.mp4` | 任一仍可用 |
| SEC-36 | `random_id` 开启 | 新频道 id 匹配 `r-[0-9a-f]{8}` | 其他形状 |
| SEC-37 | `random_id` 关闭 | 新频道 id 为 `cctv2` | 其他值 |
| SEC-38 | 1 小时宽限到期 | 未验证 | — |

## 5. 记录模板

复制到 `docs/tests/runs/<日期>.md`。文件名示例：`2026-10-30.md`。

```markdown
# 04 访问控制与限流 — 运行记录

- 日期：YYYY-MM-DD
- 执行人：
- 主机：<VPS 主机名> / <系统版本>
- tv-buddy 版本：0.1.0
- 分组：GID=... CID=... CID2=...
- 服务状态：active / 其他

## 结果

| 编号 | 检查 | 结果 | 实际现象 |
| --- | --- | --- | --- |
| SEC-01 | 默认关闭时不限流 | 通过 / 不通过 / 未验证 | |
| SEC-02 | L2 第 9 次触发 | | |
| SEC-03 | L2 冷却头 | | |
| SEC-04 | 违规升级封禁 | | |
| SEC-05 | 封禁写入统计 | | |
| SEC-06 | L1 持续限流 | | |
| SEC-07 | 会话踢旧频道 | | |
| SEC-08 | 当前频道放行 | | |
| SEC-09 | max_sessions_per_ip = 0 | | |
| SEC-10 | 白名单精确 | | |
| SEC-11 | 白名单 IPv4 CIDR | | |
| SEC-12 | 白名单通配 | | |
| SEC-13 | 白名单 IPv6 精确 | | |
| SEC-14 | IPv6 CIDR 不生效 | | |
| SEC-15 | 错误 CIDR 不生效 | | |
| SEC-16 | 白名单不豁免登录锁 | | |
| SEC-17 | 第 1 次锁时长 | | |
| SEC-18 | 第 2 次锁时长 | | |
| SEC-19 | 第 3 次锁时长 | | |
| SEC-20 | 登录锁无 Retry-After | | |
| SEC-21 | 锁不影响既有会话 | | |
| SEC-22 | 默认阈值 | | |
| SEC-23 | 关闭时锁不升级 | | |
| SEC-24 | 手动封禁 | | |
| SEC-25 | 关闭开关不解除封禁 | | |
| SEC-26 | 白名单地址被封禁仍放行 | | |
| SEC-27 | whoami 两种状态 | | |
| SEC-28 | 自助解封 | | |
| SEC-29 | 解封配额 | | |
| SEC-30 | Token 关闭 | | |
| SEC-31 | 无 Token 播放 | | |
| SEC-32 | 带 Token 播放 | | |
| SEC-33 | 轮换后旧值有效 | | |
| SEC-34 | 旧值剩余时间 | | |
| SEC-35 | 重启后旧值失效 | | |
| SEC-36 | random_id 开启 | | |
| SEC-37 | random_id 关闭 | | |
| SEC-38 | 1 小时宽限到期 | 未验证 | 没有配置键 |

## 结论

- 通过：N 项
- 不通过：N 项（列出编号）
- 未验证：N 项（列出编号与原因）

## 备注

- `[auth]` 语义为本实现自定，不证明目标 v0.8.2 的行为。
- `X-Forwarded-For` 无条件被信任；本次测试用伪造 XFF 完成按 IP 的用例。
```

## 6. 失败时怎么报

按下面顺序收集。全部贴进运行记录。

1. 记下失败的命令、完整输出、执行时间。

2. 导出当前 `[auth]`：

```bash
curl -s -H "$AUTH" "$BASE/panel/api/config" | jq '.auth' > /tmp/tv-buddy-auth-fail.json
cat /tmp/tv-buddy-auth-fail.json
```

3. 导出 IP 统计：

```bash
curl -s -H "$AUTH" "$BASE/panel/api/security/ip/stats" | jq . > /tmp/tv-buddy-ip-stats.json
cat /tmp/tv-buddy-ip-stats.json
```

4. 导出服务状态：

```bash
curl -s -H "$AUTH" "$BASE/panel/api/status" | jq . > /tmp/tv-buddy-status.json
cat /tmp/tv-buddy-status.json
```

5. 取服务日志末尾 300 行：

```bash
tail -n 300 /opt/tv-buddy/tv-buddy.log
```

6. 取系统日志末尾 200 行：

```bash
sudo journalctl -u tv-buddy -n 200 --no-pager
```

7. 记录复现的最短命令序列。必须包含：

- `curl` 的完整命令行。
- `X-Forwarded-For` 的值。
- 请求时间与相邻请求的间隔。
- 实际状态码、响应头、响应正文。

8. 记录环境：

```bash
uname -a
sha256sum /opt/tv-buddy/tv-buddy
systemctl is-active tv-buddy
```

9. 把上述内容写进 `docs/tests/runs/<日期>.md`，并在 `PROGRESS.md` 立一条缺口。

不要只贴截图。日志文本才能定位。