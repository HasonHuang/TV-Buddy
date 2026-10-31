## 1. 目的

证明 `install.sh` 在真实 Linux VPS 上完成安装、注册系统服务并启动进程。同时证明默认监听端口、默认面板账号与健康检查可用。

## 2. 前置条件

| 项目 | 确定值 | 依据 |
| --- | --- | --- |
| 操作系统 | Linux。`uname -s` 输出 `Linux` | `install.sh:144` |
| CPU 架构 | `x86_64`、`amd64`、`aarch64`、`arm64` 之一 | `install.sh:146-150` |
| 执行身份 | root。`id -u` 输出 `0` | `install.sh:152-154` |
| 安装目录 | `/opt/tv-buddy` 不存在。首次安装 | `install.sh:339-349` |
| 网络 | 可访问 `raw.githubusercontent.com`、`github.com`、`api.github.com`。安装包下载会重定向到 `objects.githubusercontent.com` | `README.md:105`、`install.sh:179`、`install.sh:233`、`install.sh:250` |
| 下载工具 | `curl` 或 `wget` 至少一个 | `install.sh:178-184` |
| 校验工具 | 建议有 `sha256sum` 或 `shasum`。两者都没有时安装脚本跳过校验 | `install.sh:163-170` |
| 解包工具 | `tar` | `install.sh:280` |
| HTTP 检查工具 | `curl`。步骤 8 至 14 使用它 | 本文档 |
| 端口 | 本机 19899/TCP 空闲 | `ServerCfg::default`（`src/config/model.rs`） |
| 服务管理器 | systemd 或 OpenRC。两者都没有时走 PID 文件模式 | `install.sh:372-386` |
| 外网访问 | VPS 安全组与防火墙放行 19899/TCP | 仓库无防火墙配置依据，未验证。本测试不依赖 |

本文档全部命令以 root 执行。当前不是 root 时，先执行 `sudo -i`。

注意：`README.md:105` 的管道安装命令不带 `sudo`。非 root 执行会失败，并输出 `错误：注册系统服务需要 root；可改用 sudo，或加 --no-service`（`install.sh:152-154`）。本文档不使用该管道形式。

本测试只在本机 `127.0.0.1` 上验证服务。判定不依赖防火墙。

## 3. 步骤

### 1. 记录机器信息

```bash
uname -a
id -u
uname -m
for c in curl wget sha256sum shasum tar systemctl rc-update rc-service; do
  printf '%s: ' "$c"
  command -v "$c" || echo "缺失"
done
```

预期：`id -u` 输出 `0`。`uname -m` 输出四种受支持架构之一。每个工具显示路径或 `缺失`。缺失项决定后面走哪条分支。

判定：`uname -s` 为 `Linux`（`install.sh:144`）。

### 2. 下载安装脚本并记录摘要

```bash
cd /root
curl -fsSL -o /root/install-tv-buddy.sh https://raw.githubusercontent.com/HasonHuang/TV-Buddy/main/install.sh
# 只有 wget 时改用：
# wget -O /root/install-tv-buddy.sh https://raw.githubusercontent.com/HasonHuang/TV-Buddy/main/install.sh
sha256sum /root/install-tv-buddy.sh
# 只有 shasum 时改用：shasum -a 256 /root/install-tv-buddy.sh
ls -l /root/tv-buddy /root/tv-buddy.sh 2>/dev/null || echo "本地包文件不存在"
```

预期：`install-tv-buddy.sh` 存在，输出 64 位十六进制摘要。`本地包文件不存在`。

预期第 3 条的原因：脚本对当前目录有分支。`/root` 存在可执行的 `tv-buddy` 时，脚本改用本地文件，不再下载。

### 3. 记录安装前状态

```bash
ls -ld /opt/tv-buddy 2>/dev/null || echo "无 /opt/tv-buddy"
ls -l /etc/systemd/system/tv-buddy.service /etc/init.d/tv-buddy /usr/local/bin/tv-buddy 2>/dev/null || true
```

预期首次安装：`无 /opt/tv-buddy`。后一条命令无输出。

### 4. 执行安装

```bash
cd /root
sh /root/install-tv-buddy.sh
echo "exit=$?"
```

预期输出。尖括号内的值随版本与网络变化：

```text
下载：https://github.com/HasonHuang/TV-Buddy/releases/download/<tag>/tv-buddy-<version>-<arch>.tar.gz
校验通过：pkg.tar.gz
（首次启动时创建数据库并生成管理员口令文件）
命令：/usr/local/bin/tv-buddy
安装完成：/opt/tv-buddy
数据：/opt/tv-buddy/tv-buddy.db
初始口令：/opt/tv-buddy/cache/admin_initial_credentials.txt
面板：http://<主机>:19899/panel（默认 admin / admin，请及时修改）
状态：服务已启动
exit=0
```

说明：

- `<version>` 是 tag 去掉前缀 `v`。资产名由工作流生成：`tv-buddy-<version>-<arch>.tar.gz`（`.github/workflows/ci.yml:137-138`）。
- `下载：` 行的 URL 可能来自 Release 资产列表，也可能由 tag 拼接（`install.sh:212-270`）。仓库只有预发布版本时，脚本先打印 `未找到正式版，改用最新 Release：<asset>`（`install.sh:253`）。
- `校验通过：pkg.tar.gz` 打印的是临时文件名，不是 Release 资产名（`install.sh:172`、`install.sh:293`、`install.sh:326`）。Release 缺少同名 `.sha256` 时不打印该行（`install.sh:160`、`install.sh:294-298`）。
- 输出 `状态：服务已启动` 表示重启命令返回 0（`install.sh:394`、`install.sh:399`、`install.sh:404`、`install.sh:414-415`）。
- systemd 的 `Type=simple` 在进程刚拉起时就返回成功（`packaging/tv-buddy.service:8`）。配置错误时进程会退出，随后每 3 秒重启一次（`packaging/tv-buddy.service:11-12`）。因此 `is-active` 可能短暂报告 active。判定以第 8 步的健康检查为准。
- 首次安装不再下发配置文件。首次启动创建 `tv-buddy.db`，写入默认配置，并创建管理员 `admin`（口令随机，见 `admin_initial_credentials.txt`）。
- 面板行固定写 `19899` 与 `<主机>` 占位符（`install.sh:413`）。它不是最后一行。最后一行是 `状态：服务已启动`，或 `启动：...`。实际地址以第 12 步的 `tv-buddy url` 为准。
- 没有任何服务管理器时，脚本先打印 `未发现 systemd/OpenRC；用 /opt/tv-buddy/tv-buddy.sh start 启动`（`install.sh:385`），再走 PID 文件模式。

### 5. 核对安装文件与软链

```bash
ls -l /opt/tv-buddy
ls -ld /opt/tv-buddy/cache
readlink -f /usr/local/bin/tv-buddy
```

预期 `ls -l` 输出应包含：

```text
-rwxr-xr-x ... tv-buddy
-rwxr-xr-x ... tv-buddy.sh
-rw------- ... tv-buddy.db
-rw-r--r-- ... tv-buddy.log
```

`readlink -f` 预期输出 `/opt/tv-buddy/tv-buddy.sh`（`install.sh:357`）。

说明：`install.sh` 不把自身复制进安装目录。它安装两个资产：`tv-buddy`、`tv-buddy.sh`。数据库由首次启动创建。因此以后执行 `tv-buddy install` 时，脚本在安装目录及其两级父目录都找不到 `install.sh`，只打印「已安装」提示，不重新安装。

### 6. 核对配置取值

```bash
sudo tv-buddy config get server.bind
sudo tv-buddy config get panel.enabled
```

预期：`bind = "[::]:19899"`。第二条输出 `无 [panel] 段`。

预期：第一条输出 `[::]:19899`，第二条输出 `true`。账号 `admin` 与随机口令由首次启动创建，登录结果在第 10 步验证。

### 7. 核对服务状态

按第 1 步探测到的管理器，只执行一条分支。

systemd：

```bash
systemctl is-active --quiet tv-buddy; echo "is-active=$?"
systemctl is-enabled tv-buddy
```

预期：`is-active=0`。`is-enabled` 输出 `enabled`（`install.sh:376`）。

OpenRC：

```bash
rc-service tv-buddy status; echo "status=$?"
rc-update show default | grep tv-buddy
```

预期：`status=0`。`rc-update` 列出 `tv-buddy`（`install.sh:382`）。

无服务管理器：

```bash
/opt/tv-buddy/tv-buddy.sh status; echo "status=$?"
```

预期：输出 `运行中（pid <N>）`，`status=0`（`packaging/tv-buddy.sh:657-662`）。

判定：systemd 与 OpenRC 分支以退出码为准。PID 分支的文本固定为 `运行中（pid <N>）` 或 `未运行`（`packaging/tv-buddy.sh:649-663`）。

### 8. 核对健康检查

```bash
curl -sS -o /tmp/tv-health.txt -w '%{http_code}\n' http://127.0.0.1:19899/health
cat /tmp/tv-health.txt; echo
curl -sS -D - -o /dev/null http://127.0.0.1:19899/health | tr -d '\r' | head -n 5
```

预期：

```text
200
ok
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
```

正文 `ok` 没有换行，`echo` 用于换行显示（`src/http.rs:25`、`src/http/routes/public.rs:29-35`）。

说明：默认绑定位是 `[::]`。Linux 默认接受 IPv4 映射地址，`127.0.0.1` 可用。若 `127.0.0.1` 连接失败而 `curl 'http://[::1]:19899/health'` 成功，检查 `sysctl net.ipv6.bindv6only`。

### 9. 核对面板页面

```bash
curl -sS -o /tmp/tv-panel.html -w '%{http_code}\n' http://127.0.0.1:19899/panel
wc -c /tmp/tv-panel.html
```

预期：状态码 `200`，字节数大于 0（`src/http.rs:50-51`）。

### 10. 核对默认账号登录

```bash
curl -sS -X POST http://127.0.0.1:19899/panel/api/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}'
```

预期：`{"token":"<uuid>"}`（`src/http/routes/panel.rs:91`）。令牌是 UUID v4（`src/auth/session.rs:29`）。

记录时只写「返回 token 字段」，不要贴完整 token。

### 11. 核对错误口令被拒绝

```bash
curl -sS -o /tmp/tv-login-bad.json -w '%{http_code}\n' -X POST http://127.0.0.1:19899/panel/api/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"not-the-password"}'
cat /tmp/tv-login-bad.json; echo
```

预期：状态码 `401`，正文 `{"error":"用户名或密码错误"}`（`src/http/routes/panel.rs:79-85`）。

本步只做一次。默认 60 秒窗口内，失败数达到 `auth_fail_free_per_window + auth_fail_ban_threshold` = 15 + 6 = 21 后锁定 300 秒（`src/config/model.rs:739-742`、`src/auth/login_guard.rs:88`）。锁定后的下一次请求返回 `429`，正文含 `locked_for_secs`（`src/http/routes/panel.rs:94-100`）。

### 12. 核对 CLI

```bash
tv-buddy url
tv-buddy status; echo "status=$?"
tv-buddy log 5 > /tmp/tv-log.txt
cat /tmp/tv-log.txt
grep -a -n 'tv-buddy 启动' /opt/tv-buddy/tv-buddy.log | tail -n 1
```

预期：

- `tv-buddy url` 输出两行：`面板地址：http://<host>:19899/panel` 与 `账号：admin`（`packaging/tv-buddy.sh:369-385`）。绑定位为 `[::]` 时自动探测本机 IP。
- `tv-buddy status` 退出码 `0`（`packaging/tv-buddy.sh:649-663`）。
- 日志中出现启动记录 `tv-buddy 启动`（`src/main.rs:39-43`）。

注意：

- `tv-buddy log` 在终端下会持续跟随文件（`tail -f`），不返回。本步重定向到文件，脚本按非终端处理，只打印最后 N 行（`packaging/tv-buddy.sh:666-674`）。
- 不要用「最后 5 行」判定启动记录。登录成功会再写一行 `面板登录成功`（`src/http/routes/panel.rs:90`）。日志增长后，启动行可能不在末尾窗口。用全文件查找。

### 13. 核对日志文件

```bash
ls -l /opt/tv-buddy/tv-buddy.log
grep -a -c 'tv-buddy 启动' /opt/tv-buddy/tv-buddy.log
```

预期：文件存在，计数至少为 1。每次重启追加一条新记录。

说明：日志文件与配置文件同目录（`src/logging.rs:163-173`）。程序启动时以追加方式创建文件（`src/logging.rs:53-56`）。

### 14. 核对重启后仍可用

```bash
tv-buddy restart; echo "restart=$?"
sleep 2
curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:19899/health
```

预期：`restart=0`，随后状态码 `200`。

说明：服务模式下 `restart` 需要 root（`packaging/tv-buddy.sh:215-223`、`packaging/tv-buddy.sh:109-111`）。

## 4. 判定

| 编号 | 检查项 | 通过标准 | 不通过标准 |
| --- | --- | --- | --- |
| 1 | 机器条件 | `uname -s` 为 `Linux`，架构为 `x86_64`、`amd64`、`aarch64`、`arm64` 之一，`id -u` 为 `0` | 任一条件不符。此时停止测试 |
| 2 | 脚本下载 | `install-tv-buddy.sh` 存在，`sha256sum` 输出 64 位摘要 | 下载失败或摘要为空 |
| 3 | 安装退出码 | `exit=0` | 非 0 |
| 4 | 安装完成标记 | 输出含 `安装完成：/opt/tv-buddy` | 缺该行 |
| 5 | 安装文件 | 存在 `tv-buddy`、`tv-buddy.sh`；首次启动后存在 `tv-buddy.db` | 缺任一文件 |
| 6 | 文件权限 | `tv-buddy` 与 `tv-buddy.sh` 为 `0755`，`tv-buddy.db` 为 `0600` | 权限不符 |
| 7 | 命令软链 | `readlink -f /usr/local/bin/tv-buddy` 输出 `/opt/tv-buddy/tv-buddy.sh` | 软链不存在或指向别的文件 |
| 8 | 配置端口 | `bind = "[::]:19899"` | 其他值 |
| 9 | 服务状态 | 第 7 步所选分支的退出码为 `0` | 非 0 |
| 10 | 开机自启 | systemd 下 `is-enabled` 输出 `enabled`；OpenRC 下 `rc-update` 列出 `tv-buddy` | 未启用 |
| 11 | 健康检查 | `/health` 返回 `200`，正文 `ok`，`content-type` 为 `text/plain; charset=utf-8` | 非 200 或正文不符 |
| 12 | 面板页面 | `/panel` 返回 `200`，字节数大于 0 | 非 200 或空正文 |
| 13 | 默认账号 | 登录返回 `200` 且正文含 `token` | 401 或 429 |
| 14 | 错误口令 | 返回 `401`，正文为 `{"error":"用户名或密码错误"}` | 返回 200 或其他正文 |
| 15 | CLI 地址 | `tv-buddy url` 输出两行，端口为 19899 | 行数不符或端口不符 |
| 16 | CLI 状态 | `tv-buddy status` 退出码为 `0` | 非 0 |
| 17 | 日志文件 | `/opt/tv-buddy/tv-buddy.log` 存在，且含 `tv-buddy 启动` | 文件缺失或不含该行 |
| 18 | 重启 | `restart=0`，重启后 `/health` 返回 `200` | 非 0 或健康检查失败 |

通过条件：第 1 至 18 项全部通过。第 10 项仅对 systemd 与 OpenRC 有效。PID 文件模式跳过第 10 项。

补充：缺少 `校验通过：pkg.tar.gz` 行时，单独记录「安装包校验未执行」。Release 应带 `.sha256`（`.github/workflows/ci.yml:154`、`.github/workflows/ci.yml:183-184`）。

## 5. 记录模板

```text
### 环境
执行时间（UTC）：
uname -a：
uname -m：
id -u：
安装脚本 SHA-256：
curl/wget/sha256sum/shasum/tar 是否存在：
服务管理器（systemd/OpenRC/无）：

### 安装
完整输出：
退出码：
（粘贴 sh /root/install-tv-buddy.sh 的全部输出）

### 文件
ls -l /opt/tv-buddy：
ls -ld /opt/tv-buddy/cache：
readlink -f /usr/local/bin/tv-buddy：

### 配置
bind 行：
是否有 [panel] 段：

### 服务
分支：
is-active 或 status 退出码：
开机自启输出：

### HTTP
/health 状态码：
/health 正文：
/health content-type：
/panel 状态码：
/panel 字节数：
/login 正确口令：是否含 token（不贴 token）：
/login 错误口令状态码与正文：

### CLI
tv-buddy url 输出：
tv-buddy status 退出码：
tv-buddy log 5 输出：
日志中的启动记录（grep -a 'tv-buddy 启动'）：
restart 退出码：
restart 后 /health 状态码：
```

## 6. 失败时怎么报

先收集证据，不要删除 `/opt/tv-buddy`。

```bash
ls -l /opt/tv-buddy > /tmp/tv-files.txt 2>&1
cp /opt/tv-buddy/tv-buddy.db /tmp/tv-buddy.db
cp /opt/tv-buddy/tv-buddy.log /tmp/tv-buddy.log
systemctl cat tv-buddy > /tmp/tv-unit.txt 2>&1
systemctl status tv-buddy --no-pager -l > /tmp/tv-status.txt 2>&1
journalctl -u tv-buddy -n 200 --no-pager > /tmp/tv-journal.txt 2>&1
ss -lntp > /tmp/tv-listen.txt 2>&1 || netstat -lntp > /tmp/tv-listen.txt 2>&1 || echo "无 ss/netstat" > /tmp/tv-listen.txt
uname -a > /tmp/tv-env.txt
id -u >> /tmp/tv-env.txt
```

OpenRC 机器改用：

```bash
rc-service tv-buddy status > /tmp/tv-rc.txt 2>&1
tail -n 200 /var/log/messages > /tmp/tv-messages.txt 2>&1
```

无服务管理器机器改用：

```bash
cat /opt/tv-buddy/tv-buddy.pid 2>/dev/null || echo "无 PID 文件"
```

按现象对应下表。

| 现象 | 先看 | 再收集 |
| --- | --- | --- |
| 输出 `错误：注册系统服务需要 root；可改用 sudo，或加 --no-service` | 执行身份，`id -u` | `/tmp/tv-env.txt` |
| 输出 `错误：只支持 Linux` | 操作系统 | `/tmp/tv-env.txt` |
| 输出 `错误：不支持的架构：<arch>（只支持 amd64/arm64）` | CPU 架构 | `/tmp/tv-env.txt` |
| 输出 `错误：在 <repo> 的 Release <version> 中找不到 <arch> 安装包……` | 版本与架构。试用 `--version <tag>` 指定版本 | 安装完整输出 |
| 输出 `错误：校验失败：<临时文件>` | 下载是否被截断或替换 | 安装完整输出、脚本 SHA-256 |
| 输出 `错误：下载失败：<url>` 或 `错误：需要 curl 或 wget 下载安装包` | 网络与工具 | 安装完整输出、`curl -I <url>` 输出 |
| 安装打印 `警告：systemctl restart tv-buddy 失败` | 单元状态 | `/tmp/tv-journal.txt`、`/tmp/tv-unit.txt` |
| 安装打印 `警告：rc-service tv-buddy restart 失败` | OpenRC 状态 | `/tmp/tv-rc.txt`、`/tmp/tv-messages.txt` |
| 安装打印 `警告：服务启动失败，见 /opt/tv-buddy/tv-buddy.log` | 日志末尾 | `/tmp/tv-buddy.log` |
| 服务反复重启 | `journalctl` 里是否有 `打开数据库失败` | `/tmp/tv-journal.txt`、`/tmp/tv-buddy.db` |
| `/health` 连接失败 | 进程是否存活、端口是否被占 | `/tmp/tv-listen.txt`、`/tmp/tv-journal.txt` |
| `/health` 返回 200 但登录 401 | 账号是否为 `admin`；口令是否取自凭据文件 | `/tmp/tv-buddy.db`、`cache/admin_initial_credentials.txt` |
| 登录返回 429 | 失败次数是否超限 | `/tmp/tv-journal.txt` |

补充规则：

- 配置读不到时，进程带上下文 `读取配置失败: <path>` 退出（`src/main.rs:19-20`）。
- 端口绑不上时，进程带上下文 `监听失败: <bind>` 退出（`src/main.rs:36-38`）。
- 配置是严格 TOML。重复键或类型错直接报错（`src/config.rs:23-27`）。
- 校验会拦：`server.bind` 为空、`panel.enabled=true` 且 `panel.password_sha256` 为空、分组 id 为空或重复、频道 id 重复（`src/config.rs:40-62`）。
- 缺少 sha256 工具时，安装脚本打印 `未找到 sha256 工具，跳过校验` 并继续（`install.sh:168-169`）。这不是安装失败，但要记录。
- systemd 下 `Restart=on-failure` 与 `RestartSec=3` 会让坏配置进入每 3 秒重启的循环（`packaging/tv-buddy.service:11-12`）。`is-active` 在该期间可能短暂为 active。以 `journalctl` 中的 `读取配置失败` 与 `/health` 结果为准。
- OpenRC 下 `tv-buddy log` 先读 `/opt/tv-buddy/tv-buddy.log`。只有该文件缺失时才回退到 `/var/log/tv-buddy.log`（`packaging/tv-buddy.sh:670-681`）。程序实际写 `/opt/tv-buddy/tv-buddy.log`（`src/logging.rs:166-173`）。正常安装下回退分支不执行。收集时两份日志都取。
- PID 文件模式下，`start` 只等进程存活，最多 10 秒（`packaging/tv-buddy.sh:185-190`）。`start` 成功不等于 HTTP 已就绪。以 `/health` 结果为准。
- 上报时附上 `/tmp/install-debug.log`。该文件由下面命令生成：

```bash
sh -x /root/install-tv-buddy.sh > /tmp/install-debug.log 2>&1; echo "exit=$?"
```

上报内容必须包含：安装脚本 SHA-256、完整安装输出、`/tmp/tv-journal.txt`、`/tmp/tv-buddy.log`、`/tmp/tv-env.txt`。数据库不要上报，里面有口令散列与访问令牌。
