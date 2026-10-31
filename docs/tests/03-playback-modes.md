## 1. 目的

本测试证明六种分组模式都能从订阅入口走到切片响应。六种模式是 `proxy`、`redirect`、`rewrite`、`passthrough`、`remux`、`dash`。本测试还证明 `proxy` 模式的 MPD 反代分支，以及 DASH 的轨道命名。

模式字段是 `[[groups]].mode`。合法值见 `src/config/model.rs:879-880`。默认值是 `proxy`，见 `src/config/model.rs:907`。

## 2. 前置条件

| 项 | 要求 |
| --- | --- |
| 机器 | Linux VPS。x86_64 或 arm64。1 vCPU 以上 |
| 系统 | Debian 12，或 Ubuntu 22.04 / 24.04 |
| 权限 | `sudo` |
| 磁盘 | 可用空间 ≥ 2 GB |
| 端口 | `19899/tcp` 与 `18080/tcp` 空闲 |
| 二进制 | `/opt/tv-buddy/tv-buddy`。来自 `v*` tag 的发布产物 |
| 工具 | `curl`、`python3`、`ffmpeg` |
| ffmpeg | 带 `libx264` 编码器与 `dash` 复用器 |
| 播放器 | 本机有 `ffplay` 或 VLC。可选 |
| 网络 | 本机能用 SSH 隧道访问 VPS 的 `19899` 与 `18080` |

先确认工具。执行：

```
ls -l /opt/tv-buddy/tv-buddy
ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libx264 && echo libx264-ok
ffmpeg -hide_banner -muxers 2>/dev/null | awk '{print $2}' | grep -qx dash && echo dash-ok
```

预期输出：

```
-rwxr-xr-x ... /opt/tv-buddy/tv-buddy
libx264-ok
dash-ok
```

二进制不在时，先按 `01-install-and-start.md` 安装。

`ffmpeg` 不存在时，先安装：

```
sudo apt-get update
sudo apt-get install -y ffmpeg
```

本测试只监听 `127.0.0.1`。测试不改动系统防火墙。测试结束后删除 `/srv/fixture` 与 `/var/lib/tv-buddy`。

## 3. 步骤

### 3.1 记录二进制

```
ls -l /opt/tv-buddy/tv-buddy
sha256sum /opt/tv-buddy/tv-buddy
```

预期输出：

```
-rwxr-xr-x ... /opt/tv-buddy/tv-buddy
<64 位十六进制>  /opt/tv-buddy/tv-buddy
```

程序不认 `--version`。它把第一个参数当配置路径（`src/main.rs:14-17`）。版本号从 `/panel/api/status` 的 `version` 字段取，见 3.9。

### 3.2 建立目录

```
sudo mkdir -p /srv/fixture/dash /srv/fixture/360 /srv/fixture/720
sudo mkdir -p /var/lib/tv-buddy/cache /etc/tv-buddy
sudo chown -R "$USER":"$USER" /srv/fixture /var/lib/tv-buddy /etc/tv-buddy
```

预期：无输出。退出码为 0。

### 3.3 生成 HLS 媒体清单夹具

```
cd /srv/fixture
ffmpeg -loglevel error -f lavfi -i testsrc=size=640x360:rate=25 -t 8 \
  -c:v libx264 -preset ultrafast -g 50 \
  -f hls -hls_time 2 -hls_list_size 0 \
  -hls_segment_filename /srv/fixture/seg%d.ts /srv/fixture/gen.m3u8
```

这一步生成 `seg0.ts` 到 `seg3.ts`。接着写一份直播清单。这份清单没有 `#EXT-X-ENDLIST`。

```
cat > /srv/fixture/live.m3u8 <<'EOF'
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:0
#EXTINF:2.000000,
seg0.ts
#EXTINF:2.000000,
seg1.ts
#EXTINF:2.000000,
seg2.ts
#EXTINF:2.000000,
seg3.ts
EOF
ls -l /srv/fixture/seg0.ts /srv/fixture/seg1.ts /srv/fixture/seg2.ts /srv/fixture/seg3.ts
```

预期：四个 `.ts` 文件都存在，且大小不为 0。

### 3.4 生成 HLS 主清单夹具

```
ffmpeg -loglevel error -f lavfi -i testsrc=size=640x360:rate=25 -t 8 \
  -c:v libx264 -preset ultrafast -g 50 \
  -f hls -hls_time 2 -hls_list_size 0 \
  -hls_segment_filename /srv/fixture/360/seg%d.ts /srv/fixture/360/index.m3u8
ffmpeg -loglevel error -f lavfi -i testsrc=size=1280x720:rate=25 -t 8 \
  -c:v libx264 -preset ultrafast -g 50 -vf scale=1280:720 \
  -f hls -hls_time 2 -hls_list_size 0 \
  -hls_segment_filename /srv/fixture/720/seg%d.ts /srv/fixture/720/index.m3u8
cat > /srv/fixture/master.m3u8 <<'EOF'
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360
360/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1280x720
720/index.m3u8
EOF
grep -c '^#EXT-X-STREAM-INF:' /srv/fixture/master.m3u8
```

预期：最后一条命令输出 `2`。

### 3.5 生成连续 TS 与 DASH 夹具

连续 TS 用于 `remux`。它必须有真实的 188 字节 TS 包。

```
ffmpeg -loglevel error -f lavfi -i testsrc=size=320x240:rate=25 \
  -f lavfi -i sine=frequency=440 -t 20 \
  -c:v libx264 -preset ultrafast -g 125 -c:a aac \
  -f mpegts /srv/fixture/live.ts
```

DASH 夹具用于 `dash`。它包含视频轨与音频轨。

```
ffmpeg -loglevel error -f lavfi -i testsrc=size=320x240:rate=25 \
  -f lavfi -i sine=frequency=440 -t 16 \
  -c:v libx264 -preset ultrafast -g 50 -c:a aac \
  -f dash -seg_duration 2 -use_template 1 -use_timeline 1 \
  /srv/fixture/dash/out.mpd
```

检查：

```
ls -l /srv/fixture/live.ts /srv/fixture/dash/out.mpd
grep -Ec 'contentType="audio"|mimeType="audio/' /srv/fixture/dash/out.mpd
```

预期：`live.ts` 与 `out.mpd` 都存在。`grep -c` 输出 `1` 或更大的数。

### 3.6 生成 proxy MPD 夹具与坏 MPD 夹具

```
cat > /srv/fixture/proxy.mpd <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2S">
  <Period id="0">
    <AdaptationSet contentType="video" maxWidth="640" maxHeight="360">
      <SegmentTemplate timescale="1000" media="dash-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="2000" r="0"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" codecs="avc1.42c01e" bandwidth="1000000" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>
EOF
head -c 2048 /dev/urandom > /srv/fixture/dash-1.m4s
printf '%s\n' '<MPD><Period></Period></MPD>' > /srv/fixture/bad.mpd
wc -c < /srv/fixture/dash-1.m4s
```

预期：`wc -c` 输出 `2048`。

### 3.7 写测试配置

先备份已有配置。执行：

```
sudo cp -a /etc/tv-buddy/tv-buddy.db /etc/tv-buddy/tv-buddy.db.bak.$(date +%s) 2>/dev/null || true
```

再写新配置。执行：

```bash
sudo /opt/tv-buddy/tv-buddy config import /etc/tv-buddy/config.json --db /etc/tv-buddy/tv-buddy.db --force
```

夹具内容（写到 `/etc/tv-buddy/config.json`）：

```json
{
  "server": {
    "bind": "127.0.0.1:19899",
    "not_found_url": "http://127.0.0.1:18080/not-found.mp4",
    "error_url": "http://127.0.0.1:18080/error.mp4",
    "log_level": "info"
  },
  "auth": {
    "token_enabled": false
  },
  "cache": {
    "disk_path": "/var/lib/tv-buddy/cache",
    "prefetch": {
      "enabled": false
    },
    "ttl": {
      "segment_secs": 300
    }
  },
  "cdn": {
    "enabled": false
  },
  "epg": {
    "enabled": false
  },
  "groups": [
    {
      "id": "pg",
      "name": "proxy 媒体",
      "mode": "proxy",
      "access": "open",
      "format": "hls",
      "channels": [
        {
          "id": "c1",
          "name": "HLS 媒体清单",
          "url": "http://127.0.0.1:18080/live.m3u8"
        },
        {
          "id": "c2",
          "name": "上游 404",
          "url": "http://127.0.0.1:18080/missing.m3u8"
        }
      ]
    },
    {
      "id": "pm",
      "name": "proxy 主清单",
      "mode": "proxy",
      "access": "open",
      "format": "hls",
      "channels": [
        {
          "id": "c1",
          "name": "HLS 主清单",
          "url": "http://127.0.0.1:18080/master.m3u8"
        }
      ]
    },
    {
      "id": "pmp",
      "name": "proxy MPD",
      "mode": "proxy",
      "access": "open",
      "format": "mpd",
      "channels": [
        {
          "id": "c1",
          "name": "MPD",
          "url": "http://127.0.0.1:18080/proxy.mpd"
        }
      ]
    },
    {
      "id": "rd",
      "name": "redirect 直连",
      "mode": "redirect",
      "access": "open",
      "redirect_mode": "direct",
      "channels": [
        {
          "id": "c1",
          "name": "直连",
          "url": "http://127.0.0.1:18080/live.m3u8"
        }
      ]
    },
    {
      "id": "rb",
      "name": "redirect 选画质",
      "mode": "redirect",
      "access": "open",
      "redirect_mode": "best",
      "redirect_ttl": 300,
      "channels": [
        {
          "id": "c1",
          "name": "选画质",
          "url": "http://127.0.0.1:18080/master.m3u8"
        }
      ]
    },
    {
      "id": "rw",
      "name": "rewrite",
      "mode": "rewrite",
      "access": "open",
      "channels": [
        {
          "id": "c1",
          "name": "改写",
          "url": "http://127.0.0.1:18080/live.m3u8"
        }
      ]
    },
    {
      "id": "pt",
      "name": "passthrough",
      "mode": "passthrough",
      "access": "open",
      "channels": [
        {
          "id": "c1",
          "name": "透传",
          "url": "http://127.0.0.1:18080/master.m3u8"
        }
      ]
    },
    {
      "id": "rx",
      "name": "remux",
      "mode": "remux",
      "access": "open",
      "channels": [
        {
          "id": "c1",
          "name": "连续 TS",
          "url": "http://127.0.0.1:18080/live.ts"
        }
      ]
    },
    {
      "id": "dh",
      "name": "dash",
      "mode": "dash",
      "access": "open",
      "format": "mpd",
      "channels": [
        {
          "id": "c1",
          "name": "DASH",
          "url": "http://127.0.0.1:18080/dash/out.mpd"
        },
        {
          "id": "c2",
          "name": "DASH 坏清单",
          "url": "http://127.0.0.1:18080/bad.mpd"
        }
      ]
    }
  ]
}
```

再确认分组数量：

```bash
sudo /opt/tv-buddy/tv-buddy config get groups --db /etc/tv-buddy/tv-buddy.db \
  | grep -o '"id"' | wc -l
```

预期：输出 `9`。

夹具用 `--force` 覆盖配置。账号 `admin` 与口令由首次启动生成，见 `cache/admin_initial_credentials.txt`。服务只监听 `127.0.0.1`，不对外暴露。

### 3.8 启动上游夹具

```
cd /srv/fixture
nohup python3 -m http.server 18080 --bind 127.0.0.1 >/tmp/upstream.log 2>&1 &
echo $! > /tmp/upstream.pid
curl -fsS http://127.0.0.1:18080/live.m3u8 >/dev/null && echo upstream-ok
```

预期：输出 `upstream-ok`。

### 3.9 启动 tv-buddy 并登录面板

```
nohup /opt/tv-buddy/tv-buddy /etc/tv-buddy/tv-buddy.db >/tmp/tv-buddy.out 2>&1 &
echo $! > /tmp/tv-buddy.pid
for i in $(seq 1 50); do
  curl -fsS http://127.0.0.1:19899/health >/dev/null 2>&1 && break
  sleep 0.2
done
curl -fsS http://127.0.0.1:19899/health
```

预期：输出 `ok`。健康检查见 `src/http.rs:25`。

登录面板。执行：

```
TOKEN=$(curl -s -X POST http://127.0.0.1:19899/panel/api/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["token"])')
test -n "$TOKEN" && echo token-ok
```

预期：输出 `token-ok`。

在同一个 shell 中定义两个小函数。后面的步骤直接使用。

```
hdr() { tr -d '\r' < "$1" | awk -v k="$2" 'BEGIN{IGNORECASE=1} index($0,k":")==1 {print substr($0,length(k)+3)}'; }
loc() { tr -d '\r' < "$1" | awk 'BEGIN{IGNORECASE=1} /^location:/ {print substr($0,11)}'; }
```

### 3.10 proxy：媒体清单入口

```
code=$(curl -s -o /tmp/pg.m3u8 -D /tmp/pg.h -w '%{http_code}' http://127.0.0.1:19899/pg/c1.m3u8)
echo "$code"
hdr /tmp/pg.h content-type
hdr /tmp/pg.h cache-control
head -n 5 /tmp/pg.m3u8
grep -oE 'http://127\.0\.0\.1:19899/pg/c1/[0-9a-f]{16}_[0-9a-f]{32}\.[a-z]+' /tmp/pg.m3u8 | head -n 1
```

预期输出：

```
200
text/plain; charset=utf-8
no-cache, no-store
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:0
#EXTINF:2.000000,
http://127.0.0.1:19899/pg/c1/<16 位小写 hex>_<32 位小写 hex>.<扩展名>
```

入口形状见 `src/http/routes/stream.rs:1403-1421`。媒体清单响应头见 `src/http/routes/stream.rs:606-629`。切片地址改写见 `src/proxy/media.rs:295-343`。

### 3.11 proxy：切片与缓存命中

```
SEG=$(grep -oE 'http://127\.0\.0\.1:19899/pg/c1/[0-9a-f]{16}_[0-9a-f]{32}\.[a-z]+' /tmp/pg.m3u8 | head -n 1)
curl -s -o /tmp/pg-seg1 -D /tmp/pg-seg1.h -w '%{http_code}\n' "$SEG"
hdr /tmp/pg-seg1.h content-type
hdr /tmp/pg-seg1.h cache-control
wc -c < /tmp/pg-seg1
curl -s -o /tmp/pg-seg2 -D /tmp/pg-seg2.h -w '%{http_code}\n' "$SEG"
hdr /tmp/pg-seg2.h cache-control
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:19899/panel/api/stats \
  | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d["segments"], d["segment_hits"])'
```

预期输出：

```
200
image/<扩展名对应类型>
public, max-age=300
<字节数，大于 0>
200
private, max-age=300
2 1
```

类型白名单见 `src/proxy/media.rs:143-155`。切片缓存头见 `src/http/routes/stream.rs:47-56`。计数见 `src/http/routes/stream.rs:1276-1281` 与 `src/stats.rs:51-56`。

### 3.12 proxy：主清单与变体清单

```
curl -s -o /tmp/pm.m3u8 -D /tmp/pm.h -w '%{http_code}\n' http://127.0.0.1:19899/pm/c1.m3u8
hdr /tmp/pm.h content-type
hdr /tmp/pm.h cache-control
cat /tmp/pm.m3u8
curl -s -o /tmp/pm-media.m3u8 -D /tmp/pm-media.h -w '%{http_code}\n' http://127.0.0.1:19899/pm/c1/video/index.txt
hdr /tmp/pm-media.h content-type
grep -oE 'http://127\.0\.0\.1:19899/pm/c1/[0-9a-f]{16}_[0-9a-f]{32}\.[a-z]+' /tmp/pm-media.m3u8 | head -n 1
```

预期输出：

```
200
application/vnd.apple.mpegurl
no-cache, no-store
#EXTM3U
#EXT-X-VERSION:6
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1280x720
http://127.0.0.1:19899/pm/c1/video/index.txt
200
text/plain; charset=utf-8
http://127.0.0.1:19899/pm/c1/<16 位小写 hex>_<32 位小写 hex>.<扩展名>
```

主清单见 `src/http/routes/stream.rs:469-493`。变体清单路径见 `src/http/routes/stream.rs:404-406` 与 `src/http/routes/stream.rs:637-674`。

### 3.13 proxy：MPD 反代

```
curl -s -o /tmp/pmp.mpd -D /tmp/pmp.h -w '%{http_code}\n' http://127.0.0.1:19899/pmp/c1.mpd
hdr /tmp/pmp.h content-type
hdr /tmp/pmp.h cache-control
grep -o '<BaseURL>[^<]*</BaseURL>' /tmp/pmp.mpd
BASE=$(grep -o '<BaseURL>[^<]*</BaseURL>' /tmp/pmp.mpd | head -n 1 | sed 's#<BaseURL>##; s#</BaseURL>##')
echo "$BASE"
curl -s -o /tmp/pmp-seg -D /tmp/pmp-seg.h -w '%{http_code}\n' "${BASE}dash-1.m4s"
hdr /tmp/pmp-seg.h content-type
wc -c < /tmp/pmp-seg
```

预期输出：

```
200
application/dash+xml; charset=utf-8
public, max-age=1, s-maxage=1
<BaseURL>http://127.0.0.1:19899/pmp/c1/mpd/<32 位 hex>/<Base64URL>/</BaseURL>
http://127.0.0.1:19899/pmp/c1/mpd/<32 位 hex>/<Base64URL>/
200
video/mp4
2048
```

错令牌检查。执行：

```
curl -s -o /dev/null -D /tmp/pmp-bad.h -w '%{http_code}\n' \
  http://127.0.0.1:19899/pmp/c1/mpd/00000000000000000000000000000000/AAAA/dash-1.m4s
loc /tmp/pmp-bad.h
```

预期：`302`，`Location` 是 `http://127.0.0.1:18080/error.mp4`。

MPD 响应头见 `src/http/routes/stream.rs:32-35` 与 `src/http/routes/stream.rs:696-715`。`<BaseURL>` 改写见 `src/http/routes/stream.rs:1049-1096`。MPD 切片见 `src/http/routes/stream.rs:1106-1158`。MPD 切片类型见 `src/http/routes/stream.rs:1186-1192`。

### 3.14 proxy：失败分支

上游返回 404。执行：

```
curl -s -o /tmp/pg-bad.body -D /tmp/pg-bad.h -w '%{http_code}\n' http://127.0.0.1:19899/pg/c2.m3u8
loc /tmp/pg-bad.h
```

预期：`302`，`Location` 是 `http://127.0.0.1:18080/error.mp4`。

切片名形状错误。执行：

```
curl -s -o /dev/null -D /tmp/pg-badseg.h -w '%{http_code}\n' http://127.0.0.1:19899/pg/c1/not-a-segment.ts
loc /tmp/pg-badseg.h
```

预期：`302`，`Location` 是 `http://127.0.0.1:18080/error.mp4`。

非 `proxy` 模式访问子路径。执行：

```
curl -s -o /dev/null -D /tmp/rd-sub.h -w '%{http_code}\n' http://127.0.0.1:19899/rd/c1/video/index.txt
loc /tmp/rd-sub.h
```

预期：`302`，`Location` 是 `http://127.0.0.1:18080/not-found.mp4`。

上游失败见 `src/http/routes/stream.rs:447-453`。重试规则见 `src/http/routes/stream.rs:1692-1731`。切片名失败见 `src/http/routes/stream.rs:1253-1265`。非 `proxy` 子路径见 `src/http/routes/stream.rs:419-429`。

### 3.15 redirect：直接跳转

```
before=$(grep -c 'GET /live.m3u8' /tmp/upstream.log || true)
curl -s -o /dev/null -D /tmp/rd.h -w '%{http_code}\n' http://127.0.0.1:19899/rd/c1.m3u8
loc /tmp/rd.h
after=$(grep -c 'GET /live.m3u8' /tmp/upstream.log || true)
echo "$before $after"
```

预期输出：

```
302
http://127.0.0.1:18080/live.m3u8
<同一个数> <同一个数>
```

`before` 与 `after` 必须相等。`direct` 模式不访问上游，见 `src/http/routes/stream.rs:219-221`。默认 `redirect_mode` 见 `src/config/model.rs:913`。

### 3.16 redirect：解析最佳画质

```
before=$(grep -c 'GET /master.m3u8' /tmp/upstream.log || true)
curl -s -o /dev/null -D /tmp/rb1.h -w '%{http_code}\n' http://127.0.0.1:19899/rb/c1.m3u8
loc /tmp/rb1.h
mid=$(grep -c 'GET /master.m3u8' /tmp/upstream.log || true)
curl -s -o /dev/null -D /tmp/rb2.h -w '%{http_code}\n' http://127.0.0.1:19899/rb/c1.m3u8
loc /tmp/rb2.h
after=$(grep -c 'GET /master.m3u8' /tmp/upstream.log || true)
echo "$before $mid $after"
```

预期输出：

```
302
http://127.0.0.1:18080/720/index.m3u8
302
http://127.0.0.1:18080/720/index.m3u8
<n> <n+1> <n+1>
```

最佳画质判定见 `src/proxy/modes.rs:37-43` 与 `src/proxy/modes.rs:54-83`。地址缓存见 `src/http/routes/stream.rs:204-223`。

### 3.17 rewrite：清单改写

```
before=$(grep -c 'GET /live.m3u8' /tmp/upstream.log || true)
curl -s -o /tmp/rw.m3u8 -D /tmp/rw.h -w '%{http_code}\n' http://127.0.0.1:19899/rw/c1.m3u8
hdr /tmp/rw.h content-type
hdr /tmp/rw.h cache-control
grep -c '^http://127.0.0.1:18080/seg0.ts$' /tmp/rw.m3u8
grep -c '/rw/c1/' /tmp/rw.m3u8
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:19899/rw/c1.m3u8
after=$(grep -c 'GET /live.m3u8' /tmp/upstream.log || true)
echo "$before $after"
```

预期输出：

```
200
text/plain; charset=utf-8
no-cache, must-revalidate
1
0
200
<n> <n+1>
```

正文必须指向上游绝对地址。正文不能包含本服务切片路径。响应头见 `src/http/routes/stream.rs:306-314`。改写逻辑见 `src/proxy/media.rs:295-343`。5 秒缓存见 `src/http/routes/stream.rs:234-237` 与 `src/state.rs:125`。

### 3.18 passthrough：直接透传

```
before=$(grep -c 'GET /master.m3u8' /tmp/upstream.log || true)
curl -s -o /dev/null -D /tmp/pt.h -w '%{http_code}\n' http://127.0.0.1:19899/pt/c1.m3u8
loc /tmp/pt.h
after=$(grep -c 'GET /master.m3u8' /tmp/upstream.log || true)
echo "$before $after"
curl -s "http://127.0.0.1:19899/sub/$PTOK/playlist.m3u" | grep -A1 '直通'
```

预期输出：

```
302
http://127.0.0.1:18080/master.m3u8
<同一个数> <同一个数>
#EXTM3U
#EXTINF:-1 tvg-id="c1" tvg-name="透传" tvg-logo="" group-title="passthrough",透传
http://127.0.0.1:18080/master.m3u8
```

订阅条目不能包含 `/pt/c1`，也不能包含 `token=`。入口见 `src/http/routes/stream.rs`。订阅条目见 `src/playlist.rs` 的 `entry_url`。

### 3.19 remux：预热、就绪与切片

第一次请求必须返回 `503`。

```
curl -s -o /tmp/rx.body -D /tmp/rx.h -w '%{http_code}\n' http://127.0.0.1:19899/rx/c1.m3u8
cat /tmp/rx.body
hdr /tmp/rx.h retry-after
hdr /tmp/rx.h cache-control
```

预期输出：

```
503
remux warming up
2
no-store
```

等待就绪。最多等 30 秒。

```
for i in $(seq 1 60); do
  code=$(curl -s -o /tmp/rx.m3u8 -D /tmp/rx-ready.h -w '%{http_code}' http://127.0.0.1:19899/rx/c1.m3u8)
  [ "$code" = "200" ] && break
  sleep 0.5
done
echo "$code"
hdr /tmp/rx-ready.h content-type
hdr /tmp/rx-ready.h cache-control
grep -c '#EXT-X-TARGETDURATION:5' /tmp/rx.m3u8
grep -oE '/rx/c1/seg_[0-9]{12}\.[a-z]+' /tmp/rx.m3u8 | head -n 1
```

预期输出：

```
200
text/plain; charset=utf-8
no-cache, must-revalidate
1
/rx/c1/seg_<12 位数字>.<扩展名>
```

读取一个切片。执行：

```
RSEG=$(grep -oE '/rx/c1/seg_[0-9]{12}\.[a-z]+' /tmp/rx.m3u8 | head -n 1)
curl -s -o /tmp/rx-seg -D /tmp/rx-seg.h -w '%{http_code}\n' "http://127.0.0.1:19899$RSEG"
hdr /tmp/rx-seg.h content-type
hdr /tmp/rx-seg.h cache-control
wc -c < /tmp/rx-seg
```

预期：`200`，`image/<扩展名对应类型>`，`public, max-age=30`，字节数大于 0。

未知切片。执行：

```
curl -s -o /dev/null -D /tmp/rx-miss.h -w '%{http_code}\n' http://127.0.0.1:19899/rx/c1/seg_999999999999.png
loc /tmp/rx-miss.h
```

预期：`302`，`Location` 是 `http://127.0.0.1:18080/not-found.mp4`。

预热响应见 `src/http/routes/stream.rs:341-346`。就绪清单见 `src/http/routes/stream.rs:326-338`。清单正文见 `src/remux.rs:75-93`。切片名见 `src/remux.rs:249-257` 与 `src/remux.rs:33`。切片响应见 `src/remux.rs:195-217`。未知切片见 `src/remux.rs:190-206` 与 `src/http/routes/stream.rs:423-429`。

### 3.20 dash：主清单

```
curl -s -o /tmp/dh.m3u8 -D /tmp/dh.h -w '%{http_code}\n' http://127.0.0.1:19899/dh/c1.m3u8
hdr /tmp/dh.h content-type
hdr /tmp/dh.h cache-control
grep -c '^#EXT-X-VERSION:7$' /tmp/dh.m3u8
grep -c '^#EXT-X-INDEPENDENT-SEGMENTS$' /tmp/dh.m3u8
grep -c '^#EXT-X-STREAM-INF:' /tmp/dh.m3u8
grep -o '/dh/c1/video_240p/index.txt' /tmp/dh.m3u8
grep -c '#EXT-X-MEDIA:TYPE=AUDIO' /tmp/dh.m3u8
grep -c 'AUDIO="audio"' /tmp/dh.m3u8
```

预期输出：

```
200
application/vnd.apple.mpegurl
no-cache, no-store
1
1
1
/dh/c1/video_240p/index.txt
1
1
```

入口响应见 `src/http/routes/stream.rs:271-280`。主清单见 `src/dash_hls.rs:676-760`。音频行见 `src/dash_hls.rs:688-702`。变体属性见 `src/dash_hls.rs:747-752`。

### 3.21 dash：媒体清单与切片

```
curl -s -o /tmp/dh-media.m3u8 -D /tmp/dh-media.h -w '%{http_code}\n' http://127.0.0.1:19899/dh/c1/video_240p/index.txt
hdr /tmp/dh-media.h content-type
hdr /tmp/dh-media.h cache-control
grep -c '^#EXT-X-VERSION:7$' /tmp/dh-media.m3u8
grep -c '^#EXT-X-TARGETDURATION:2$' /tmp/dh-media.m3u8
grep -c '^#EXT-X-DISCONTINUITY-SEQUENCE:0$' /tmp/dh-media.m3u8
grep -c '^#EXT-X-MAP:URI=' /tmp/dh-media.m3u8
grep -c '^#EXT-X-ENDLIST$' /tmp/dh-media.m3u8
DSEG=$(grep -oE '/dh/c1/video_240p/[0-9]+_[0-9a-f]{8}\.[a-z]+' /tmp/dh-media.m3u8 | head -n 1)
echo "$DSEG"
curl -s -o /tmp/dh-seg -D /tmp/dh-seg.h -w '%{http_code}\n' "http://127.0.0.1:19899$DSEG"
hdr /tmp/dh-seg.h content-type
hdr /tmp/dh-seg.h cache-control
wc -c < /tmp/dh-seg
```

预期输出：

```
200
text/plain
public, max-age=5, s-maxage=5
1
1
1
1
1
/dh/c1/video_240p/<段号>_<8 位小写 hex>.<扩展名>
200
image/<扩展名对应类型>
public, max-age=300
<字节数，大于 0>
```

媒体清单见 `src/dash_hls.rs:474-539`。正文见 `src/dash_hls.rs:825-876`。切片响应头见 `src/dash_hls.rs:465-468`。

### 3.22 dash：轨道表

```
curl -s -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:19899/panel/api/dash/status -o /tmp/dash-status.json
python3 - <<'PY'
import json
data = json.load(open('/tmp/dash-status.json'))
sessions = [s for s in data if s.get('group_id') == 'dh' and s.get('channel_id') == 'c1']
assert len(sessions) == 1, sessions
tracks = sessions[0]['active_tracks']
print('kinds', sorted(t['kind'] for t in tracks))
print('names', sorted(t['name'] for t in tracks))
PY
```

预期输出：

```
kinds ['audio', 'video']
names ['audio-...', 'video_240p']
```

视频轨名固定是 `video_240p`。音频轨名按 `audio-{语言}` 生成。没有语言时按序号生成。规则见 `src/dash.rs:361-411`。语言来源见 `src/dash.rs:243`。接口字段见 `src/http/routes/dash.rs:43-45` 与 `src/dash_hls.rs:60-97`。

### 3.23 dash：两个负面检查

未知轨道。执行：

```
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:19899/dh/c1/video/index.txt
```

预期：`404`。这里不能返回 `302`。见 `src/http/routes/stream.rs:388-397` 与 `src/dash_hls.rs:490-496`。

坏 MPD。执行：

```
curl -s -o /tmp/dh-bad.body -D /tmp/dh-bad.h -w '%{http_code}\n' http://127.0.0.1:19899/dh/c2.m3u8
cat /tmp/dh-bad.body
```

预期输出：

```
502
Upstream Error
```

见 `src/http/routes/stream.rs:317-323`。

### 3.24 订阅形状

订阅只从用户令牌进入。先从面板取令牌，再取订阅：

```
TOK=$(curl -s -X POST http://127.0.0.1:19899/panel/api/login \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"<口令>"}' \
  | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
PTOK=$(curl -s -H "Authorization: Bearer $TOK" \
  http://127.0.0.1:19899/panel/api/me \
  | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
curl -s "http://127.0.0.1:19899/sub/$PTOK/playlist.m3u"
```

预期一：全局订阅已停用。

```
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:19899/playlist.m3u
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:19899/pg/playlist.m3u
```

两次都是 `404`。见 `src/http/routes/sub.rs` 的 `gone`。

预期二：用户订阅包含本服务入口。执行结果包含：

```
#EXTM3U
#EXTINF:-1 tvg-id="c1" tvg-name="HLS 媒体清单" tvg-logo="" group-title="proxy 媒体",HLS 媒体清单
http://127.0.0.1:19899/pg/c1.m3u8?token=<PTOK>
```

预期三：透传组条目直接给上游原始地址，且不带令牌，见 3.18。

入口扩展名规则见 `src/playlist.rs` 的 `entry_url`。

### 3.25 播放器观察（不计入判定）

本测试的判定在 HTTP 层。播放器行为不在仓库取证范围。本服务把切片伪装成图片扩展名，见 `src/proxy/media.rs:143-155`。部分播放器按扩展名判断类型。本步只记录观察结果。

在带图形界面的本机执行。先建 SSH 隧道。执行：

```
ssh -N -L 19899:127.0.0.1:19899 -L 18080:127.0.0.1:18080 <用户名>@<VPS 地址>
```

保持这条命令运行。另开一个本机终端。先预热 remux 会话。执行：

```
for i in $(seq 1 30); do
  code=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:19899/rx/c1.m3u8)
  [ "$code" = "200" ] && break
  sleep 0.5
done
echo "$code"
```

预期：输出 `200`。然后逐个执行：

```
ffplay -autoexit -t 10 http://127.0.0.1:19899/pg/c1.m3u8
ffplay -autoexit -t 10 http://127.0.0.1:19899/pm/c1.m3u8
ffplay -autoexit -t 10 http://127.0.0.1:19899/rd/c1.m3u8
ffplay -autoexit -t 10 http://127.0.0.1:19899/rw/c1.m3u8
ffplay -autoexit -t 10 http://127.0.0.1:19899/pt/c1.m3u8
ffplay -autoexit -t 10 http://127.0.0.1:19899/rx/c1.m3u8
ffplay -autoexit -t 10 http://127.0.0.1:19899/dh/c1.m3u8
```

观察目标：

| 地址 | 画面 | 声音 |
| --- | --- | --- |
| `pg/c1.m3u8` | 640x360 测试图案 | 无 |
| `pm/c1.m3u8` | 1280x720 测试图案 | 无 |
| `rd/c1.m3u8` | 640x360 测试图案 | 无 |
| `rw/c1.m3u8` | 640x360 测试图案 | 无 |
| `pt/c1.m3u8` | 1280x720 测试图案 | 无 |
| `rx/c1.m3u8` | 320x240 测试图案 | 440 Hz 单音 |
| `dh/c1.m3u8` | 320x240 测试图案 | 440 Hz 单音 |

播放器能出画面时，记录「播放器可播」。播放器报错或黑屏时，记录「播放器不可播」，并附播放器名称与版本。该结果不改变第 4 节的结论。

### 3.26 停止测试进程

```
kill "$(cat /tmp/tv-buddy.pid)" "$(cat /tmp/upstream.pid)" 2>/dev/null || true
sleep 1
```

预期：无输出。两个进程都退出。

## 4. 判定

| 编号 | 检查项 | 通过标准 | 不通过标准 |
| --- | --- | --- | --- |
| P-01 | proxy 媒体入口 | `200`；`text/plain; charset=utf-8`；`no-cache, no-store` | 非 `200`；类型或缓存头不符 |
| P-02 | proxy 切片地址 | 每条切片行匹配 `http://127.0.0.1:19899/pg/c1/<16hex>_<32hex>.<ext>` | 出现上游地址；形状不符 |
| P-03 | proxy 首次切片 | `200`；`image/*`；`public, max-age=300`；正文非空 | 非 `200`；类型不是 `image/*`；缓存头不是 `public` |
| P-04 | proxy 切片命中 | 第二次 `private, max-age=300`；统计为 `2 1` | 第二次仍是 `public`；统计不是 `2 1` |
| P-05 | proxy 主清单 | `200`；`application/vnd.apple.mpegurl`；`no-cache, no-store`；只有一条 `STREAM-INF`，`BANDWIDTH=2800000` | 多条变体；类型或缓存头不符 |
| P-06 | proxy 变体清单 | `200`；`text/plain; charset=utf-8`；切片指向本服务 `/pm/c1/` | 切片指向上游；非 `200` |
| P-07 | proxy MPD | `200`；`application/dash+xml; charset=utf-8`；`public, max-age=1, s-maxage=1`；`BaseURL` 指向本服务 `/pmp/c1/mpd/` | 类型或缓存头不符；`BaseURL` 指向上游 |
| P-08 | proxy MPD 切片 | `200`；`video/mp4`；正文 2048 字节 | 非 `200`；类型不符；字节数不符 |
| P-09 | proxy MPD 错令牌 | `302`；`Location` 是 `error_url` | 返回 `200` 或 `404` |
| P-10 | proxy 上游 404 | `302`；`Location` 是 `error_url` | 返回 `200` 或 `5xx` |
| P-11 | proxy 错切片名 | `302`；`Location` 是 `error_url` | 返回 `404` 或 `200` |
| P-12 | 非 proxy 子路径 | `302`；`Location` 是 `not_found_url` | 返回 `404` 或 `200` |
| R-01 | redirect 直连 | `302`；`Location` 是上游清单；上游日志计数不变 | 上游计数增加；`Location` 不符 |
| R-02 | redirect 最佳画质 | `302`；`Location` 是 `http://127.0.0.1:18080/720/index.m3u8` | 指向 360 变体；非 `302` |
| R-03 | redirect 地址缓存 | 第一次主清单计数 `+1`；第二次 `+0` | 第二次仍访问上游 |
| W-01 | rewrite 响应 | `200`；`text/plain; charset=utf-8`；`no-cache, must-revalidate` | 状态、类型或缓存头不符 |
| W-02 | rewrite 绝对化 | 正文有 `http://127.0.0.1:18080/seg0.ts`；本地路径计数为 `0` | 仍有相对地址；出现 `/rw/c1/` |
| W-03 | rewrite 缓存 | 两次请求，上游清单计数只 `+1` | 计数 `+2` |
| T-01 | passthrough 入口 | `302`；`Location` 是上游清单；上游计数不变 | 上游计数增加；`Location` 不符 |
| T-02 | passthrough 订阅 | 条目是上游原始地址；无 `/pt/c1`；无 `token=` | 出现本服务地址或 Token |
| X-01 | remux 冷启动 | `503`；正文 `remux warming up`；`retry-after: 2`；`no-store` | 非 `503`；正文或头不符 |
| X-02 | remux 就绪 | `200`；`text/plain; charset=utf-8`；`no-cache, must-revalidate`；含 `#EXT-X-TARGETDURATION:5` | 30 秒内未到 `200`；头或正文不符 |
| X-03 | remux 切片 | `200`；`image/*`；`public, max-age=30`；正文非空 | 非 `200`；类型或缓存头不符 |
| X-04 | remux 未知切片 | `302`；`Location` 是 `not_found_url` | 返回 `404` 或 `200` |
| D-01 | dash 主清单 | `200`；`application/vnd.apple.mpegurl`；`no-cache, no-store`；含 VERSION 7、INDEPENDENT-SEGMENTS、`STREAM-INF`、`video_240p/index.txt` | 状态、类型或正文不符 |
| D-02 | dash 音频行 | 主清单有 `TYPE=AUDIO`；变体行有 `AUDIO="audio"` | 缺音频行或音频组属性 |
| D-03 | dash 媒体清单 | `200`；`text/plain`；`public, max-age=5, s-maxage=5`；含 VERSION 7、TARGETDURATION 2、DISCONTINUITY-SEQUENCE 0、MAP、ENDLIST | 状态、类型、缓存头或正文不符 |
| D-04 | dash 切片 | `200`；`image/*`；`public, max-age=300`；正文非空 | 非 `200`；类型或缓存头不符 |
| D-05 | dash 轨道表 | `kinds ['audio', 'video']`；视频名 `video_240p` | 缺音频或视频；视频名不符 |
| D-06 | dash 未知轨道 | `404` | 返回 `302` 或 `200` |
| D-07 | dash 坏 MPD | `502`；正文 `Upstream Error` | 返回 `503` 或 `200` |
| S-01 | 总订阅过滤 | 总订阅只有 `#EXTM3U` | 出现 `open` 分组条目 |
| S-02 | 分组订阅入口 | `pg` 条目是 `http://127.0.0.1:19899/pg/c1.m3u8` | 出现上游地址；扩展名不符 |

## 5. 记录模板

把下面内容复制到 `docs/tests/runs/<日期>.md`，填写后贴回。

```
### 六模式端到端播放 执行记录

- 执行日期：
- 执行人：
- 机器与系统：
- 仓库路径：
- 仓库 tag：<git describe --tags --always 输出>
- 二进制 SHA-256：<sha256sum 输出>
- tv-buddy 版本：<启动日志 version= 字段>
- ffmpeg 版本：<ffmpeg -version 首行>
- 总结果：通过 / 不通过
- 通过编号：
- 不通过编号：

#### 不通过明细

| 编号 | 执行的命令 | 实际输出 | 期望输出 |
| --- | --- | --- | --- |
|  |  |  |  |
```

## 6. 失败时怎么报

按下面的顺序收集证据。每一步都执行。

1. 收集服务日志。执行：

```
tail -n 300 /etc/tv-buddy/tv-buddy.log > /tmp/report-tv-buddy.log
tail -n 300 /tmp/tv-buddy.out > /tmp/report-stdout.log
```

2. 收集上游日志。执行：

```
tail -n 300 /tmp/upstream.log > /tmp/report-upstream.log
```

3. 收集面板日志与指标。执行：

```
curl -s -H "Authorization: Bearer $TOKEN" \
  'http://127.0.0.1:19899/panel/api/logs?limit=500' > /tmp/report-panel-logs.json
curl -s -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:19899/panel/api/stats > /tmp/report-stats.json
curl -s -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:19899/panel/api/cache/stats > /tmp/report-cache-stats.json
```

4. 收集磁盘与进程信息。执行：

```
find /var/lib/tv-buddy/cache -maxdepth 4 -type f -printf '%p %s\n' | sort > /tmp/report-cache-files.txt
ss -ltnp | grep -E ':(19899|18080)\b' > /tmp/report-ports.txt
ps -o pid,etime,cmd -p "$(cat /tmp/tv-buddy.pid)" "$(cat /tmp/upstream.pid)" > /tmp/report-procs.txt
```

5. 收集环境。执行：

```
uname -a > /tmp/report-env.txt
ffmpeg -version | head -n 1 >> /tmp/report-env.txt
sha256sum /opt/tv-buddy/tv-buddy >> /tmp/report-env.txt
```

6. 按模式查日志关键词。

| 模式 | 日志关键词 | 来源 |
| --- | --- | --- |
| `proxy` | `抓取上游清单失败`、`抓取变体清单失败`、`片段拉取失败`、`代理 MPD 切片失败` | `src/http/routes/stream.rs:450`、`src/http/routes/stream.rs:653`、`src/http/routes/stream.rs:1322`、`src/http/routes/stream.rs:1181` |
| `redirect` | `解析上游主清单失败` | `src/http/routes/stream.rs:215` |
| `rewrite` | `抓取上游清单失败` | `src/http/routes/stream.rs:241` |
| `remux` | `remux 上游状态异常`、`remux 上游连接失败`、`remux 上游读取失败` | `src/remux.rs:303`、`src/remux.rs:314`、`src/remux.rs:334` |
| `dash` | `DASH 主清单失败`、`DASH 连续失败，进入冷却`、`DASH 媒体清单抓取失败`、`DASH MPD 解析失败`、`DASH 切片抓取失败` | `src/http/routes/stream.rs:282`、`src/http/routes/stream.rs:288-294`、`src/dash_hls.rs:479`、`src/dash_hls.rs:486`、`src/dash_hls.rs:645-651` |

7. 打包证据。执行：

```
tar -czf /tmp/playback-modes-$(date +%Y%m%d-%H%M%S).tgz \
  /etc/tv-buddy/tv-buddy.db \
  /tmp/report-tv-buddy.log /tmp/report-stdout.log /tmp/report-upstream.log \
  /tmp/report-panel-logs.json /tmp/report-stats.json /tmp/report-cache-stats.json \
  /tmp/report-cache-files.txt /tmp/report-ports.txt /tmp/report-procs.txt /tmp/report-env.txt
ls -l /tmp/playback-modes-*.tgz
```

预期：输出一个 `.tgz` 路径。

报告时写三项：

1. 不通过的编号，例如 `P-04`。
2. 该编号对应的完整命令。
3. 实际输出与预期输出的差异。

把 `.tgz` 文件、这三项、以及 `docs/tests/runs/<日期>.md` 的记录一起提交。
