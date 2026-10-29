## 1. 目的

本验收证明两件事：S4 的磁盘回收在真实磁盘上收敛到上限；S6 的 64 MB 实例连续运行 24 小时不 OOM，且回收目录不超限。

本验收不证明以下三项。仓库没有对应依据，结果记为「未验证」。

- 1 vCPU 抖动的量化阈值。仓库只写「不抖动」，没有延迟或 CPU 上限（`docs/design/06-low-spec-optimization.md:390`）。
- 非回收目录的增长阈值。GC 只扫 `seg_bytes` 与 `seg_url`（`src/cache_gc.rs:33`）。
- 64 MB 档位的专用配置。仓库只有 128 MB 档位的 `memory_max_mb = 4` 注释（`docs/design/config-low-spec.toml:29-31`）。

## 2. 前置条件

| 项 | 要求 |
| --- | --- |
| 机器 | 1 vCPU。内存 64 MB 或 128 MB。磁盘 1 GB。带宽足够拉流 |
| 系统 | Linux，amd64 或 arm64。systemd 可用 |
| 权限 | `root`（`sudo`） |
| 已通过 | [`01-install-and-start.md`](01-install-and-start.md) |
| 二进制 | 来自 `v*` tag 的发布包。执行前先 `git fetch --tags` 并记录实际 tag |
| 源码目录 | 需要 `docs/design/config-low-spec.toml` |
| 真实源 | S6 需要一条能播放的 HLS 地址。样例里的 `upstream.example` 不能播放（`docs/design/config-low-spec.toml:87`） |
| 工具 | `curl`、`sha256sum`、`dd`、`find`、`awk`、`sed`、`grep`、`tar`、`systemctl`、`journalctl` |
| 工具（S4-3） | `pidstat`、`mpstat`。属于 sysstat。仓库未声明该依赖，先确认可用 |
| 磁盘余量 | S4-1 开始前，`/opt/tv-buddy/cache` 所在文件系统可用空间不少于 313 MB |
| 风险 | S4-2 会把磁盘填到可用空间小于 100 MB。只在测试 VPS 上做，不要在承载业务的机器上做 |

S4 只需要本机访问。S6 需要 VPS 能访问上游 HLS 源。

## 3. 步骤

### 3.1 确认机器规格

执行：

```bash
nproc --all
lscpu | grep -E '^CPU\(s\)|^On-line CPU'
grep -c '^processor' /proc/cpuinfo
```

预期：三条命令的结果都是 `1`。

执行：

```bash
cat /sys/fs/cgroup/cpu.max
```

预期：`max 100000`（无 CPU 配额）或 `100000 100000`（1 个 CPU）。两种都算 1 vCPU。

cgroup v1 的机器执行：

```bash
cat /sys/fs/cgroup/cpu/cpu.cfs_quota_us
cat /sys/fs/cgroup/cpu/cpu.cfs_period_us
```

预期：`-1` 表示无配额；否则配额除以周期等于 1。

执行：

```bash
free -m
cat /sys/fs/cgroup/memory.max
```

预期：64 MB 档的 `memory.max` 不大于 `67108864`；128 MB 档不大于 `134217728`。`free -m` 的 `total` 与档位一致。

仓库没有写「64 MB」是 MiB 还是十进制 MB。本验收按 64 MiB = 67,108,864 字节判定，并记录实际值。

执行：

```bash
df -B1 /opt
stat -f -c '%b %S' /opt
```

预期：`df` 的 `1B-blocks` 约 1,000,000,000 或 1,073,741,824。`stat` 两个数相乘等于同一容量。

`/opt/tv-buddy` 尚未安装时，用 `/` 代替 `/opt`。安装后重复一次：

```bash
df -B1 /opt/tv-buddy/cache
stat -f -c '%b %S' /opt/tv-buddy/cache
```

### 3.2 安装并写入低配配置

先在源码目录确认要测的 tag：

```bash
TAG=$(git describe --tags --abbrev=0 --match 'v*')
echo "$TAG"
```

预期：输出本次要测的 tag。把它记进运行记录。

用已下载的发布包安装。`<version>` 是 tag 去掉前缀 `v`，`<arch>` 是 `amd64` 或 `arm64`：

```bash
sudo sh install.sh --archive "/root/tv-buddy-<version>-<arch>.tar.gz"
```

或者直接从 Release 装指定 tag：

```bash
sudo sh install.sh --version "$TAG"
```

包名格式为 `tv-buddy-<version>-<arch>.tar.gz`（`.github/workflows/ci.yml`）。

预期输出包含：

```
已生成配置：/opt/tv-buddy/config.toml（请修改后重启）
安装完成：/opt/tv-buddy
状态：服务已启动
```

安装脚本复制的是 `packaging/config.toml.example`（`install.sh:342-348`），不是低配样例。所以下一步覆盖配置：

```bash
sudo install -m 0644 docs/design/config-low-spec.toml /opt/tv-buddy/config.toml
```

生成面板口令哈希。把 `NEWPASS` 换成你的口令：

```bash
printf '%s' 'NEWPASS' | sha256sum | cut -d' ' -f1
```

预期：64 位十六进制字符串。

编辑配置：

```bash
sudoedit /opt/tv-buddy/config.toml
```

在 `[panel]` 段内补两行。样例只有 `enabled = true`，没有口令：

```toml
password_sha256 = "<上一步的 64 位十六进制>"
session_ttl = 604800
```

`session_ttl` 默认 86400 秒（`src/config/model.rs:166`）。S6 要跑 24 小时，会话会在同一时刻过期。本验收临时改为 7 天。回滚时删除这一行。

改分组与频道。这三处必须指向你的真实值：

```toml
[[groups]]
id = "<分组 id>"
name = "<分组名>"
mode = "proxy"
access = "public"
format = "hls"

[[groups.channels]]
id = "<频道 id>"
name = "<频道名>"
url = "<真实 HLS 地址>"
```

S4 不需要真实源。S6 必须有真实源，否则 `seg_bytes` 不会增长（`src/http/routes/stream.rs:1278`）。

重启并确认：

```bash
sudo systemctl restart tv-buddy
sleep 5
systemctl is-active tv-buddy
```

预期：`active`。

确认服务读到的版本：

```bash
sudo grep -m1 'config' /opt/tv-buddy/tv-buddy.log
```

预期：无「读取配置失败」或 TOML 解析错误。

### 3.3 取面板 Token

```bash
TOKEN=$(curl -s -X POST http://127.0.0.1:19899/panel/api/login \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"NEWPASS"}' \
  | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
echo "${#TOKEN}"
```

预期：输出 36。空输出表示口令错误，或 `[panel].enabled` 为 false。

登录端点见 `src/http/routes/panel.rs:40`；响应字段见 `src/http/routes/panel.rs:91`。

验证 Token：

```bash
curl -s -o /dev/null -w '%{http_code}\n' -H "X-Token: $TOKEN" \
  http://127.0.0.1:19899/panel/api/cache/stats
```

预期：`200`。面板接受 `X-Token` 与 `Authorization: Bearer`（`src/http/ext.rs:23-38`）。

### 3.4 读取当前磁盘上限

```bash
curl -s -H "X-Token: $TOKEN" http://127.0.0.1:19899/panel/api/cache/stats
```

预期：JSON 含 `"recommend":{...,"disk_cap_mb":<整数>,"source":"fallback",...}`。

取值：

```bash
CAP_MB=$(curl -s -H "X-Token: $TOKEN" http://127.0.0.1:19899/panel/api/cache/stats \
  | sed -n 's/.*"disk_cap_mb":\([0-9]*\).*/\1/p')
echo "CAP_MB=$CAP_MB"
```

从未播放过时，`working` 为空，上限回退 150 MB（`src/cache_gc.rs:73`）。可用空间不少于 313 MB 时，`disk_cap_mb` 应为 `150`。

取值依据 `src/cache_gc.rs:305-320`。

### 3.5 S4-1：造 200 MB 后回落到上限

本条测「超量 LRU」。不要同时播放，避免真实切片混进统计。

步骤 1：记录当前可用空间。命令与代码同口径，用 `f_bavail × f_frsize ÷ 1,000,000`：

```bash
stat -f -c '%a %S' /opt/tv-buddy/cache | awk '{printf "%d MB\n", int(($1*$2)/1000000)}'
```

预期：不少于 313。

步骤 2：造 200 MB。50 个文件，每个 4 MiB，合计 209,715,200 字节：

```bash
mkdir -p /opt/tv-buddy/cache/seg_bytes
for i in $(seq 1 50); do
  dd if=/dev/zero of=/opt/tv-buddy/cache/seg_bytes/fill_$i.bin bs=1M count=4 status=none
done
```

文件必须是新建的。mtime 超过 1800 秒的文件先被「超龄」删除，测不到超量路径（`src/cache_gc.rs:188-195`）。

步骤 3：确认造盘结果：

```bash
find /opt/tv-buddy/cache/seg_bytes -type f -printf '%s\n' | awk '{s+=$1} END{print s+0}'
```

预期：`209715200`。

步骤 4：等一轮 GC。进程启动后 30 秒跑第一轮，之后每 `interval_secs` 秒一轮（`src/cache_gc.rs:233-242`）。样例是 300 秒。

```bash
sudo tail -n 20 /opt/tv-buddy/tv-buddy.log | grep '磁盘回收完成'
```

预期：出现一行，含 `files=`（大于 0）、`mb=`、`emergency=false`。

步骤 5：再算实际占用。与代码同口径，两份目录的顶层文件：

```bash
find /opt/tv-buddy/cache/seg_bytes /opt/tv-buddy/cache/seg_url -type f -printf '%s\n' \
  | awk '{s+=$1} END{print s+0}'
```

预期：不大于 `CAP_MB × 1000000`。`CAP_MB=150` 时，结果不超过 `150000000`。50 个文件会删掉约 15 个。

步骤 6：确认面板统计同步下降：

```bash
curl -s -H "X-Token: $TOKEN" http://127.0.0.1:19899/panel/api/cache/stats \
  | sed -n 's/.*"seg_bytes":\([0-9]*\).*/\1/p'
```

预期：明显小于 209715200。该字段只统计 `seg_bytes`，不含 `seg_url`（`src/cache_gc.rs:315`）。

步骤 7：清理测试文件，避免影响 S4-2 的起点：

```bash
sudo find /opt/tv-buddy/cache/seg_bytes -maxdepth 1 -name 'fill_*.bin' -delete
```

### 3.6 S4-2：可用空间低于预留时紧急清理

本条测「紧急模式」。条件是 `free_mb < reserve_mb`，默认阈值 100 MB（`src/cache_gc.rs:90`）。

步骤 1：确认上限是自动值：

```bash
sudo sed -n '/^\[cache.gc\]/,/^\[/p' /opt/tv-buddy/config.toml | grep max_disk_mb
```

预期：`max_disk_mb = 0`。显式填数时，代码直接返回该值，公式不参与（`src/cache_gc.rs:70-72`）。

步骤 2：停服务，避免中途 tick 打断造盘：

```bash
sudo systemctl stop tv-buddy
```

步骤 3：持续写盘，直到可用空间小于 100 MB：

```bash
i=0
while [ "$(stat -f -c '%a %S' /opt/tv-buddy/cache | awk '{print int(($1*$2)/1000000)}')" -ge 100 ]; do
  i=$((i+1))
  dd if=/dev/zero of=/opt/tv-buddy/cache/seg_bytes/fill_$i.bin bs=1M count=4 status=none
done
echo "created=$i"
```

预期：循环结束，`created` 为正整数。

文件必须放在 `seg_bytes` 或 `seg_url` 的顶层。其它目录与子目录不回收（`src/cache_gc.rs:33`、`src/cache_gc.rs:158-174`）。

步骤 4：记录停机前可用空间：

```bash
stat -f -c '%a %S' /opt/tv-buddy/cache | awk '{printf "%d MB\n", int(($1*$2)/1000000)}'
```

预期：小于 100。

步骤 5：启动服务：

```bash
sudo systemctl start tv-buddy
```

步骤 6：等第一轮 tick。固定在启动后 30 秒（`src/cache_gc.rs:29`）：

```bash
sleep 45
```

步骤 7：查两条日志：

```bash
sudo grep -E '已紧急回收|磁盘回收完成' /opt/tv-buddy/tv-buddy.log | tail -n 2
```

预期：先出现 `可用空间低于预留，已紧急回收`，随后出现 `磁盘回收完成`，且 `emergency=true`（`src/cache_gc.rs:272-274`、`src/cache_gc.rs:208-215`）。

步骤 8：算实际占用：

```bash
find /opt/tv-buddy/cache/seg_bytes /opt/tv-buddy/cache/seg_url -type f -printf '%s\n' \
  | awk '{s+=$1} END{print s+0}'
```

预期：不大于 `25000000`。紧急模式的目标是 `cap / 2`；`cap` 为 50 MB 时为 25 MB（`src/cache_gc.rs:74-76`、`src/cache_gc.rs:199`）。

步骤 9：再算可用空间：

```bash
stat -f -c '%a %S' /opt/tv-buddy/cache | awk '{printf "%d MB\n", int(($1*$2)/1000000)}'
```

预期：明显大于 100。

步骤 10：清理测试文件：

```bash
sudo find /opt/tv-buddy/cache/seg_bytes -maxdepth 1 -name 'fill_*.bin' -delete
```

如果磁盘被填在缓存目录之外，GC 删完缓存也无法把可用空间抬回 100 MB。此时每轮都会进入紧急模式。

### 3.7 S4-3：1 vCPU 不抖动

代码事实：目录遍历与删除在阻塞线程池执行，不占 tokio 异步线程（`src/cache_gc.rs:265-269`）。单轮最多处理 `scan_limit` 个文件，默认 500（`src/cache_gc.rs:184`）。

步骤 1：确认采集工具：

```bash
command -v pidstat mpstat
```

预期：两行绝对路径。缺工具时先安装，Debian/Ubuntu 用 `sudo apt-get install -y sysstat`，RHEL 系用 `sudo dnf install -y sysstat`。

步骤 2：取主进程 PID：

```bash
PID=$(systemctl show -p MainPID --value tv-buddy)
echo "$PID"
```

预期：正整数。

步骤 3：启动三项采集，窗口 180 秒。用 `setsid` 让它脱离当前终端：

```bash
sudo setsid sh -c "pidstat -p $PID 1 180 > /root/s4-gc-pidstat.txt 2>&1" &
sudo setsid sh -c "mpstat 1 180 > /root/s4-gc-mpstat.txt 2>&1" &
sudo setsid sh -c 'for n in $(seq 1 180); do curl -s -o /dev/null -w "%{http_code} %{time_total}\n" http://127.0.0.1:19899/health; sleep 1; done > /root/s4-gc-latency.txt' &
```

步骤 4：在采集窗口内制造一次回收。另开一个终端，重复 3.5 的步骤 2。等 60 秒后确认日志出现「磁盘回收完成」。

步骤 5：采集结束后分析请求失败数：

```bash
grep -vc '^200 ' /root/s4-gc-latency.txt
```

预期：`0`。

步骤 6：分析延迟分布：

```bash
awk '{print $2}' /root/s4-gc-latency.txt | sort -n \
  | awk '{a[NR]=$1} END{printf "n=%d p95=%.3f max=%.3f\n", NR, a[int(NR*0.95)], a[NR]}'
```

步骤 7：分析进程 CPU 峰值。最后一列可能是 `Command`，所以取倒数第二列：

```bash
awk 'NR>3 && $1 ~ /^[0-9]/ {print $(NF-1)}' /root/s4-gc-pidstat.txt | sort -n | tail -1
```

步骤 8：分析整机最低空闲：

```bash
grep -E '^[0-9]' /root/s4-gc-mpstat.txt | awk '{print $NF}' | sort -n | head -1
```

步骤 9：采集一段空闲基线。不造盘，重复步骤 3 与步骤 6，文件名改成 `/root/s4-base-latency.txt`。

步骤 10：清理测试文件：

```bash
sudo find /opt/tv-buddy/cache/seg_bytes -maxdepth 1 -name 'fill_*.bin' -delete
```

抖动幅度的通过阈值在仓库中不存在。步骤 6 到 8 的数值只记录，结论记「未验证」。

### 3.8 S6：64 MB 实例跑 24 小时

本条需要真实播放流量。只起服务不播放，`seg_bytes` 不会增长。

步骤 1：确认内存档位与 GC：

```bash
free -m
cat /sys/fs/cgroup/memory.max
sudo sed -n '/^\[cache.gc\]/,/^\[/p' /opt/tv-buddy/config.toml | grep enabled
```

预期：内存与 64 MB 或 128 MB 档一致；`enabled = true`。

步骤 2：重启服务，记录起点：

```bash
sudo systemctl restart tv-buddy
date -Is
```

重启会换播放 Token（Token 在内存里，见 `src/auth/play_token.rs:1-5`）。所以先重启，再取播放地址。

步骤 3：取播放入口地址：

```bash
GID=<分组 id>
curl -s "http://127.0.0.1:19899/$GID/playlist.m3u" | grep -v '^#' | grep -v '^$' | head -1
```

预期：`http://127.0.0.1:19899/<分组 id>/<频道 id>.m3u8?token=<48 位十六进制>`。

订阅会自动带播放 Token（`src/http/routes/play.rs:103-106`）。`access = "open"` 的分组不带 Token，且免校验。

步骤 4：写负载脚本：

```bash
sudo tee /root/s6-load.sh >/dev/null <<'EOF'
#!/bin/sh
GID=main
ENTRY=$(curl -s "http://127.0.0.1:19899/$GID/playlist.m3u" | grep -v '^#' | grep -v '^$' | head -1)
echo "ENTRY=$ENTRY" > /root/s6-load.log
while :; do
  curl -s --max-time 20 "$ENTRY" -o /tmp/s6-pl.m3u8
  grep -v '^#' /tmp/s6-pl.m3u8 2>/dev/null | grep -v '^$' | tail -3 | while read -r u; do
    curl -s -o /dev/null --max-time 20 "$u"
  done
  sleep 10
done
EOF
sudo chmod 0755 /root/s6-load.sh
```

把脚本里的 `GID=main` 改成你的分组 id。脚本每 10 秒取清单末尾 3 个切片，模拟连续播放。

步骤 5：写采样脚本：

```bash
sudo tee /root/s6-sample.sh >/dev/null <<'EOF'
#!/bin/sh
OUT=/root/s6-24h.txt
: > "$OUT"
n=0
while [ "$n" -lt 25 ]; do
  {
    date -Is
    printf 'health '
    curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:19899/health
    printf 'seg_total_bytes '
    find /opt/tv-buddy/cache/seg_bytes /opt/tv-buddy/cache/seg_url -type f -printf '%s\n' \
      | awk '{s+=$1} END{print s+0}'
    printf 'avail_bytes '
    stat -f -c '%a %S' /opt/tv-buddy/cache | awk '{printf "%d\n", $1*$2}'
    printf 'disk_cap_mb '
    curl -s -H "X-Token: $TOKEN" http://127.0.0.1:19899/panel/api/cache/stats \
      | sed -n 's/.*"disk_cap_mb":\([0-9]*\).*/\1/p'
    PID=$(systemctl show -p MainPID --value tv-buddy)
    grep -E 'VmRSS|VmHWM' /proc/"$PID"/status 2>/dev/null
    CG=$(systemctl show -p ControlGroup --value tv-buddy)
    cat "/sys/fs/cgroup$CG/memory.current" 2>/dev/null
    cat "/sys/fs/cgroup$CG/memory.events" 2>/dev/null
    echo '---'
  } >> "$OUT"
  n=$((n+1))
  [ "$n" -lt 25 ] && sleep 3600
done
EOF
sudo chmod 0755 /root/s6-sample.sh
```

采样 25 次，间隔 3600 秒，覆盖 24 小时。

步骤 6：启动负载与采样：

```bash
sudo setsid /root/s6-load.sh >/dev/null 2>&1 &
sudo TOKEN="$TOKEN" setsid /root/s6-sample.sh >/dev/null 2>&1 &
```

步骤 7：每天另存一份日志。日志文件到 5 MB 会截断并写 `[log truncated]`（`src/logging.rs:15`、`src/logging.rs:85-90`）：

```bash
sudo cp /opt/tv-buddy/tv-buddy.log /root/s6-log-$(date +%Y%m%d-%H%M).log
```

步骤 8：24 小时后取结果。

查 OOM 计数：

```bash
CG=$(systemctl show -p ControlGroup --value tv-buddy)
cat "/sys/fs/cgroup$CG/memory.events"
```

预期：`oom 0`，`oom_kill 0`。`max` 可以不为 0。

查内存限额与峰值：

```bash
cat "/sys/fs/cgroup$CG/memory.max"
cat "/sys/fs/cgroup$CG/memory.current"
cat "/sys/fs/cgroup$CG/memory.peak"
```

`memory.peak` 需要内核 6.8 及以上。文件不存在时改用采样文件里的 `VmHWM`。

查内核日志：

```bash
dmesg -T | grep -iE 'out of memory|oom-kill|Killed process'
```

预期：无输出。

查 systemd 日志：

```bash
journalctl -u tv-buddy --since '24 hours ago' --no-pager | grep -iE 'oom|killed process|Out of memory'
```

预期：无输出。

查存活与运行时长。会话可能在 24 小时边界过期，先重新登录：

```bash
TOKEN=$(curl -s -X POST http://127.0.0.1:19899/panel/api/login \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"NEWPASS"}' \
  | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
systemctl is-active tv-buddy
curl -s -H "X-Token: $TOKEN" http://127.0.0.1:19899/panel/api/stats \
  | sed -n 's/.*"uptime_secs":\([0-9]*\).*/\1/p'
```

预期：`active`；`uptime_secs` 不小于 `86400`。

查写盘失败：

```bash
sudo grep -c 'No space left on device' /opt/tv-buddy/tv-buddy.log
```

预期：`0`。

步骤 9：停止负载与采样：

```bash
sudo pkill -f /root/s6-load.sh
sudo pkill -f /root/s6-sample.sh
```

### 3.9 回滚

把三处开关关掉。配置热重载，下一请求生效，不需要重启（`docs/design/06-low-spec-optimization.md:401-402`）：

```toml
[cdn]
enabled = false

[cache.gc]
enabled = false
```

```toml
[groups.prefetch]
enabled = false
```

删除本验收加入的 `session_ttl`，恢复默认 86400 秒：

```bash
sudoedit /opt/tv-buddy/config.toml
```

删除测试数据：

```bash
sudo find /opt/tv-buddy/cache/seg_bytes -maxdepth 1 -name 'fill_*.bin' -delete
sudo rm -f /root/s6-load.sh /root/s6-sample.sh /root/s4-gc-*.txt /root/s4-base-latency.txt
```

## 4. 判定

| 编号 | 检查项 | 通过标准 | 不通过标准 |
| --- | --- | --- | --- |
| P-1 | 机器规格 | 3.1 三条 CPU 命令都是 1；内存与 64 MB 或 128 MB 档一致 | 任一不符，或无法读取 cgroup |
| P-2 | 服务启动 | `systemctl is-active` 返回 `active` | 返回 `inactive` 或 `failed` |
| S4-1-a | 回落到上限 | 实际字节数不大于 `CAP_MB × 1000000` | 超时后仍大于上限 |
| S4-1-b | 回收日志 | 含「磁盘回收完成」，`files` 大于 0，`emergency=false` | 无该行，或只有「读磁盘空间失败」 |
| S4-1-c | 面板同步 | `recommend.seg_bytes` 明显小于造盘值 | 数值不变 |
| S4-2-a | 紧急日志 | 含「已紧急回收」，随后「磁盘回收完成」且 `emergency=true` | 无紧急告警 |
| S4-2-b | 紧急目标 | 实际字节数不大于 `25000000` | 大于 25000000 |
| S4-2-c | 空间回升 | 可用空间明显大于 100 MB | 仍小于 100 MB |
| S4-3-a | 请求无失败 | `grep -vc '^200 '` 为 0 | 出现非 200 或连接失败 |
| S4-3-b | 抖动幅度 | 未验证：仓库没有量化阈值 | 不适用 |
| S6-a | 无 OOM | `oom` 与 `oom_kill` 均为 0；`dmesg` 与 `journalctl` 无 OOM 记录 | 任一不为 0，或有 OOM 记录 |
| S6-b | 持续存活 | 服务 `active`，`uptime_secs` 不小于 86400 | 中途重启或退出 |
| S6-c | 回收目录不超限 | 每个采样点 `seg_total_bytes` 不大于该点的 `disk_cap_mb × 1000000` | 任一点超限 |
| S6-d | 写盘成功 | 日志无 `No space left on device` | 出现该串 |
| S6-e | 非回收目录 | 未验证：`m3u8`、`sub_url`、`dash` 等目录没有上限，仓库无增长阈值 | 不适用 |

S4-3-b 与 S6-e 只能记录数值，结论写「未验证」，并写缺什么。

## 5. 记录模板

复制到 `docs/tests/runs/<日期>.md`。文件名用执行当天日期，例如 `2026-10-30.md`。

```markdown
# 低配置真机验收记录

- 日期：
- 发布 tag：
- `version` 字段：
- VPS 供应商与型号：
- CPU 核数（3.1 三条命令）： / /
- cgroup CPU 配额：
- 内存档位：64 MB / 128 MB（实际 `memory.max`： ）
- 磁盘：`df -B1` 总字节 / `stat -f` 总字节

## 机器确认
- P-1：通过 / 不通过 / 未验证
- 实际现象：

## S4-1 造 200 MB
- 造盘前可用 MB：
- `CAP_MB`：
- 造盘后字节：
- 回收后字节：
- 回收日志行：
- `recommend.seg_bytes`：
- S4-1-a：通过 / 不通过
- S4-1-b：通过 / 不通过
- S4-1-c：通过 / 不通过
- 实际现象：

## S4-2 紧急清理
- 停机前可用 MB：
- 造盘文件数：
- 紧急日志行：
- 回收后字节：
- 回收后可用 MB：
- S4-2-a：通过 / 不通过
- S4-2-b：通过 / 不通过
- S4-2-c：通过 / 不通过
- 实际现象：

## S4-3 1 vCPU
- 采样窗口：180 秒
- 请求失败数：
- 延迟 p95 / max：
- 基线延迟 p95 / max：
- 进程 CPU 峰值（%）：
- 整机最低 `%idle`：
- S4-3-a：通过 / 不通过
- S4-3-b：未验证（仓库无阈值）
- 实际现象：

## S6 24 小时
- 起点时间：
- 结束时间：
- 负载脚本实际入口：
- `memory.max`：
- `memory.current`：
- `memory.peak` 或 `VmHWM`：
- `memory.events` 的 `oom` / `oom_kill`：
- `dmesg` OOM 行数：
- `journalctl` OOM 行数：
- `uptime_secs`：
- 各采样点 `seg_total_bytes` / `disk_cap_mb`：（贴 /root/s6-24h.txt）
- `No space left on device` 计数：
- S6-a：通过 / 不通过
- S6-b：通过 / 不通过
- S6-c：通过 / 不通过
- S6-d：通过 / 不通过
- S6-e：未验证（非回收目录无阈值）
- 实际现象：

## 结论
- 总判定：通过 / 不通过 / 未验证
- 缺口：
```

## 6. 失败时怎么报

第 1 步：收集证据。`TOKEN` 未失效时直接执行：

```bash
D=/root/tv-buddy-fail-$(date +%Y%m%d-%H%M)
sudo mkdir -p "$D"
sudo cp /opt/tv-buddy/tv-buddy.log "$D"/ 2>/dev/null
sudo cp /opt/tv-buddy/config.toml "$D"/ 2>/dev/null
systemctl status tv-buddy --no-pager > "$D/systemctl.txt" 2>&1
journalctl -u tv-buddy --since '48 hours ago' --no-pager > "$D/journal.txt" 2>&1
PID=$(systemctl show -p MainPID --value tv-buddy)
grep -E 'VmRSS|VmHWM' /proc/"$PID"/status > "$D/rss.txt" 2>&1
dmesg -T | tail -n 500 > "$D/dmesg.txt" 2>&1
df -B1 > "$D/df.txt"
stat -f -c '%a %S %b' /opt/tv-buddy/cache > "$D/statvfs.txt"
find /opt/tv-buddy/cache/seg_bytes /opt/tv-buddy/cache/seg_url -type f -printf '%p %s %TY-%Tm-%Td %TH:%TM\n' \
  > "$D/cache-files.txt" 2>&1
curl -s -H "X-Token: $TOKEN" http://127.0.0.1:19899/panel/api/cache/stats > "$D/cache-stats.json"
curl -s -H "X-Token: $TOKEN" http://127.0.0.1:19899/panel/api/stats > "$D/stats.json"
CG=$(systemctl show -p ControlGroup --value tv-buddy)
cp "/sys/fs/cgroup$CG/memory.events" "$D"/ 2>/dev/null
cp "/sys/fs/cgroup$CG/memory.max" "$D"/ 2>/dev/null
cp "/sys/fs/cgroup$CG/memory.current" "$D"/ 2>/dev/null
cp /root/s4-gc-pidstat.txt /root/s4-gc-mpstat.txt /root/s4-gc-latency.txt "$D"/ 2>/dev/null
cp /root/s6-24h.txt /root/s6-load.log "$D"/ 2>/dev/null
sudo tar -czf "$D.tar.gz" -C /root "$(basename "$D")"
echo "$D.tar.gz"
```

预期：最后一行输出 tar 包路径。

第 2 步：在 `docs/tests/runs/<日期>.md` 的对应项写三项：

- 结论：`不通过`。
- 实际现象：数字与日志原文，一行。
- 证据：tar 包路径。

第 3 步：把 tar 包路径交给开发者。开发者在 `PROGRESS.md` 立一条缺口。

必须包含的文件：`tv-buddy.log`、`config.toml`、`cache-stats.json`、`statvfs.txt`、`cache-files.txt`、`memory.events`。

S4-1 或 S4-2 失败时，另附回收前后的两次 `cache-files.txt`。没有两份文件，无法判断是「没删」还是「删了又写回」。

S6 失败时，另附 24 小时采样文件 `/root/s6-24h.txt`。没有该文件，无法定位 OOM 发生的时刻。