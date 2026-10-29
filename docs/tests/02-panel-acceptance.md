## 1. 目的

这份文档证明管理面板的外观与结构可用。判据是**元素存在、顺序正确、尺寸正确**，不是像素一致。

本文档对应 `PROGRESS.md` §6.3 的 P3-6 与 §3.4 的 V-10。范围是 13 个一级分区。登录页是前置步骤，不算分区。

范围外：

- 目标 `/panel` 的像素级比对。`docs/design/07-coverage-and-plan.md` §5 明确不做像素比对。
- 浏览器兼容性。只测 Chrome。

> 本实现的界面文案全部自研。目标截图只用来对照布局层级，不用来对照文字（`docs/design/reference/README.md` §7）。

## 2. 前置条件

| 项 | 要求 |
| --- | --- |
| 机器 | 已按 `01-install-and-start.md` 装好并启动 |
| 浏览器 | Chrome 133 或更新版本 |
| 视口 | 1440×1000，设备像素比 1 |
| 面板账号 | 默认 `admin` / `admin` |
| 伪上游 | `python3 -m http.server 9999` 提供 `live.m3u8` 与 `live.mpd` |
| 端口 | `19898/tcp` 空闲。用独立实例测，不动生产实例 |

### 2.1 起一台独立测试实例

不要在正在服务的实例上造测试数据。

```bash
sudo mkdir -p /tmp/tv-buddy-panel-test/cache
sudo tee /tmp/tv-buddy-panel-test/config.toml >/dev/null <<'EOF'
[server]
bind = "127.0.0.1:19898"
log_level = "info"

[health]
interval_secs = 10

[cache]
disk_path = "/tmp/tv-buddy-panel-test/cache"
EOF
sudo /opt/tv-buddy/tv-buddy /tmp/tv-buddy-panel-test/config.toml \
  >/tmp/tv-buddy-panel-test/run.log 2>&1 &
echo $! | sudo tee /tmp/tv-buddy-panel-test/pid
sleep 1
curl -fsS http://127.0.0.1:19898/health; echo
```

预期：`ok`。

配置路径是**第一个位置参数**。没有 `--config` 开关（`src/main.rs:14-17`）。

### 2.2 启动本地伪上游

```bash
mkdir -p /tmp/tv-buddy-origin
printf '#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n' \
  > /tmp/tv-buddy-origin/live.m3u8
printf 'x' > /tmp/tv-buddy-origin/seg1.ts
printf '<?xml version="1.0"?><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT8S"><Period><AdaptationSet mimeType="video/mp4"><Representation id="v1" bandwidth="500000"><BaseURL>v1.mp4</BaseURL><SegmentBase indexRange="0-100"/></Representation></AdaptationSet></Period></MPD>' \
  > /tmp/tv-buddy-origin/live.mpd
cd /tmp/tv-buddy-origin && python3 -m http.server 9999 >/tmp/tv-buddy-origin/http.log 2>&1 &
```

预期：`curl -sI http://127.0.0.1:9999/live.m3u8` 返回 `200`。

### 2.3 造测试数据

```bash
B=http://127.0.0.1:19898
T=$(curl -sS -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
     -d '{"username":"admin","password":"admin"}' \
   | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
echo "$T" | head -c 8; echo
```

预期：输出 8 位十六进制前缀。token 是 UUID（`src/auth/session.rs`）。

```bash
cat >/tmp/tv-buddy-panel-test/seed.json <<'EOF'
[
  {"id":"qa1","name":"验收代理组","mode":"proxy","access":"public","format":"hls","channels":[
    {"id":"c1","name":"CCTV-1","url":"http://127.0.0.1:9999/live.m3u8",
     "tvg_id":"cctv1","tvg_name":"CCTV-1","logo":"/logo/CCTV1.png",
     "prefetch":{"enabled":true,"prefetch_limit":3,"concurrency":2}},
    {"id":"c2","name":"央视二套","url":"http://127.0.0.1:9999/live.m3u8",
     "livestream":true,"timeshift_hours":4},
    {"id":"c3","name":"故障频道","url":"http://127.0.0.1:1/dead.m3u8"}
  ]},
  {"id":"qa2","name":"验收 DASH 组","mode":"dash","access":"public","format":"mpd","channels":[
    {"id":"d1","name":"DASH 测试","url":"http://127.0.0.1:9999/live.mpd"}
  ]},
  {"id":"qa3","name":"验收直通组","mode":"passthrough","access":"public","format":"hls","channels":[
    {"id":"p1","name":"直通频道","url":"http://127.0.0.1:9999/live.m3u8"}
  ]},
  {"id":"qa4","name":"验收空组","mode":"proxy","access":"public","format":"hls","channels":[]}
]
EOF
curl -sS -X POST "$B/panel/api/groups/import" \
  -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  --data-binary @/tmp/tv-buddy-panel-test/seed.json; echo
```

预期：`{"ok":true,"imported":4,"skipped":0}`。

`qa1/c3` 的上游指向 `127.0.0.1:1`，用来造探测失败。`qa2/d1` 不写 `selected_tracks`，用来造 `全轨道` 徽章。

### 2.4 让健康探测先跑一轮

```bash
sleep 12
curl -sS -H "Authorization: Bearer $T" "$B/panel/api/health" > /tmp/tv-buddy-panel-test/health.json
python3 -c '
import json
d = json.load(open("/tmp/tv-buddy-panel-test/health.json"))["entries"]
for k in ("qa1/c1","qa1/c3","qa2/d1"):
    print(k, d.get(k, {}).get("ok"))
'
```

预期：`qa1/c1 True`、`qa1/c3 False`、`qa2/d1 True`。

探测周期下限 10 秒（`src/health.rs:118`）。

### 2.5 打开面板

```bash
echo "浏览器打开：http://127.0.0.1:19898/panel"
# 在远端 VPS 上测试时，先建隧道：
# ssh -N -L 19898:127.0.0.1:19898 <user>@<vps>
```

用账号 `admin`、口令 `admin` 登录。

## 3. 步骤

每一步给出两条：一条浏览器控制台命令，一条人工确认。控制台命令给出客观判据。人工确认给出布局判据。

先做视口自检。控制台执行：

```js
[innerWidth, innerHeight, devicePixelRatio]
```

预期：`[1440, 1000, 1]`。不符时先改视口，后面的判定全部无效。

### 3.1 侧栏骨架

控制台执行：

```js
JSON.stringify({
  count: document.querySelectorAll('.sidebar .nav-item').length,
  labels: [...document.querySelectorAll('.sidebar .nav-item')].map((b) => b.textContent.trim()),
  width: Math.round(document.querySelector('.sidebar').getBoundingClientRect().width),
  contentLeft: Math.round(document.querySelector('.content').getBoundingClientRect().left),
  active: document.querySelector('.nav-item.active')?.textContent.trim(),
})
```

预期：

```json
{"count":13,"labels":["概览","缓存状态","分组管理","系统设置","安全管控","频道热度","定时录制","节目指南","台标管理","订阅聚合","回看管理","视频素材","运行日志"],"width":224,"contentLeft":224,"active":"概览"}
```

人工确认：

- 每项左侧有图标，图标与文字颜色一致。
- 选中项是浅青底、青色文字（`frontend/src/style.css` 的 `.nav-item.active`）。
- 侧栏顶部显示 `TV Buddy` 与 `管理面板`。
- 侧栏底部有 `退出登录` 按钮。

### 3.2 逐屏页头

依次点击 13 个导航项，每屏读一次页头标题。

控制台执行：

```js
(async () => {
  const out = [];
  for (const label of ['概览','缓存状态','分组管理','系统设置','安全管控','频道热度','定时录制','节目指南','台标管理','订阅聚合','回看管理','视频素材','运行日志']) {
    [...document.querySelectorAll('.sidebar .nav-item')].find((b) => b.textContent.trim() === label)?.click();
    await new Promise((r) => setTimeout(r, 300));
    window.scrollTo(0, 0);
    out.push([label, document.querySelector('.page-head .title')?.textContent.trim(),
              document.querySelector('.page-head .sub')?.textContent.trim()]);
  }
  return JSON.stringify(out);
})()
```

预期页头标题顺序：

| 导航项 | 页头标题 | 副标题 |
| --- | --- | --- |
| 概览 | 系统概览 | 实时运行状态 |
| 缓存状态 | 缓存状态 | `<后端> · <字节数>` |
| 分组管理 | 分组管理 | 管理直播分组与频道 |
| 系统设置 | 系统设置 | 修改后点击保存生效；保存写回整份 config.toml。 |
| 安全管控 | 安全管控 | 播放 Token、访问控制与限流 |
| 频道热度 | 频道热度 | `版本 <版本号>` |
| 定时录制 | 定时录制 | 按时间表用 ffmpeg 录制，产物为 MKV |
| 节目指南 | 节目指南 | 已开启 / 已关闭 |
| 台标管理 | 台标库 | 上传、转码与匹配频道台标 |
| 订阅聚合 | 订阅聚合 | 聚合清单：`<origin>`/playlist.m3u |
| 回看管理 | 回看管理 | 按频道缓冲历史切片，支持时移回看 |
| 视频素材 | 视频素材 | 上传、转换与引用本地视频与字幕 |
| 运行日志 | 运行日志 | 最近 N 行（共 N 行） |

注意两处不一致，这是正确的：

- 导航写「台标管理」，页头写「台标库」。
- 导航写「概览」，页头写「系统概览」。

人工确认：每屏页头都在第一张卡片**上方**，且页头有青色图标。

### 3.3 概览

控制台执行：

```js
JSON.stringify({
  stats: [...document.querySelectorAll('.card .stat .k')].map((e) => e.textContent.trim()),
  sections: [...document.querySelectorAll('.card .section-title')].map((e) => e.textContent.trim()),
})
```

预期：

```json
{"stats":["分组数","频道数","缓存后端","在线观众","缓存磁盘占用","下行 · 回源","上行 · 出网","进程内存","CPU 占用","整盘内存峰值"],"sections":["实时指标","快速操作","系统信息"]}
```

人工确认：

- 页头右侧有 `● 运行 <时长>` 徽章。
- 「下行 · 回源」显示 `0 B`。这是**复刻目标缺陷**，不判为坏（`src/http/routes/panel.rs` 固定返回 0）。
- 快速操作有四个按钮：清空缓存、热重载配置、刷新状态、导出全部 M3U。
- 系统信息最后一行是 `监听地址`，值为 `127.0.0.1:19898`。

### 3.4 分组管理

点导航「分组管理」。

控制台执行：

```js
JSON.stringify({
  actions: [...document.querySelectorAll('.page-head .actions button, .page-head .actions a')]
             .map((e) => e.textContent.trim()),
  groups: [...document.querySelectorAll('.group-card')].map((c) => ({
    head: [...c.querySelector('.group-head').children].map((e) => e.getAttribute('class')),
    name: c.querySelector('.gname')?.textContent.trim(),
    id: c.querySelector('.gid')?.textContent.trim(),
    mode: c.querySelector('.badge.mode')?.textContent.trim(),
    count: c.querySelector('.count')?.textContent.trim(),
    ops: [...c.querySelectorAll('.group-head .ops button, .group-head .ops a')]
           .map((b) => b.title || b.textContent.trim()),
    channels: [...c.querySelectorAll('.ch-row')].map((r) => ({
      kids: [...r.children].map((e) => e.getAttribute('class')),
      main: [...r.querySelector('.ch-main').children].map((e) => e.getAttribute('class')),
      nameBadges: [...r.querySelectorAll('.ch-name .badge')].map((b) => b.textContent.trim()),
      badges: [...r.querySelectorAll('.ch-badges .badge')].map((b) => b.textContent.trim()),
      ops: [...r.querySelectorAll('.ch-ops button')].map((b) => b.title || b.textContent.trim()),
    })),
  })),
})
```

预期，按顺序：

页头操作（`.page-head .actions` 下的按钮与链接，从左到右）：

```json
["M3U 导入","JSON 导入","JSON 覆盖","导出全部","多选","新建分组"]
```

四个分组，顺序 `qa1`、`qa2`、`qa3`、`qa4`，分组头子元素：

```json
["icon folder","gname","gid","ops"]
```

分组操作区，按顺序：

```json
["上移分组","下移分组","编辑","导出该分组的 JSON","删除","频道","打开该分组的 M3U","复制订阅地址"]
```

频道行子元素：

```json
["dot ok","ch-main","ch-ops"]
```

`qa1/c3` 是 `["dot err","ch-main","ch-ops"]`。`dot ok` 与 `dot err` 里带状态，比较时按前缀 `dot` 判。

频道主区子元素：

```json
["ch-name","ch-url","ch-url local","ch-badges"]
```

各频道预期徽章：

| 频道 | 名称区徽章 | 地址下方徽章 |
| --- | --- | --- |
| `qa1/c1` | 无 | `tvg-id`、`Logo`、`预取开启` |
| `qa1/c2` | `常驻` | 无 |
| `qa1/c3` | `异常` | 无 |
| `qa2/d1` | `全轨道` | 无 |
| `qa3/p1` | 无 | 无；本地地址行内多一个 `直通` 徽章 |

各频道预期操作按钮：

| 频道 | 按钮 |
| --- | --- |
| 普通频道 | `上移频道`、`下移频道`、`复制播放地址`、`编辑`、`删除` |
| DASH 频道 | `上移频道`、`下移频道`、`复制播放地址`、`查看轨道`、`编辑`、`删除` |

人工确认：

- 模式徽章屏幕上是大写（`PROXY`、`DASH`），但控制台读到的是小写。CSS `text-transform: uppercase` 负责显示（`frontend/src/style.css`）。
- 本地播放地址前有青色三角 `▶`。它是 CSS 伪元素，`textContent` 里没有。
- `qa3/p1` 的上游地址与本地地址是同一个值。
- `qa4` 显示 `这个分组还没有频道。`
- `qa1/c3` 左侧状态点是红色，其余是绿色。

搜索检查。控制台执行：

```js
(() => {
  const input = document.querySelector('input.search');
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  setter.call(input, 'CCTV');
  input.dispatchEvent(new Event('input', { bubbles: true }));
  return 'ok';
})()
```

预期：只剩 `qa1` 一张卡，且 3 个频道全在。

改成 `zzz` 后预期显示 `没有匹配的分组。`

### 3.5 分组弹窗

点 `qa1` 卡头的 `编辑`。

控制台执行：

```js
JSON.stringify({
  title: document.querySelector('.modal-head h3')?.textContent.trim(),
  labels: [...document.querySelectorAll('.modal .field > label')].map((l) => l.textContent.trim()),
  width: Math.round(document.querySelector('.modal').getBoundingClientRect().width),
  detailsOpen: document.querySelector('.modal details.block')?.open,
  footer: [...document.querySelectorAll('.modal-foot button')].map((b) => b.textContent.trim()),
})
```

预期：

```json
{"title":"编辑分组","labels":["分组 ID","显示名称","工作模式","内容格式","访问范围","代理类型","代理地址","代理用户名","代理密码","请求头","预取覆盖"],"width":540,"detailsOpen":false,"footer":["取消","保存"]}
```

人工确认：

- `分组 ID` 输入框禁用，值为 `qa1`。
- 下方提示「只用于地址与配置键，建成后不能改。」
- 工作模式选中 `Proxy（反向代理）`。
- 内容格式选中 `HLS（m3u8）`。
- 访问范围选中 `公开`。
- 网络配置默认折叠，摘要为「网络配置 · 分组级默认，频道可单独覆盖」。

点开网络配置，确认出现 `代理类型`、`代理地址`、`代理用户名`、`代理密码`、`请求头`、`预取覆盖` 六个字段。

按 `Esc` 关闭。

### 3.6 频道弹窗

点 `qa1/c1` 行的 `编辑`。

控制台执行：

```js
JSON.stringify({
  title: document.querySelector('.modal-head h3')?.textContent.trim(),
  labels: [...document.querySelectorAll('.modal .field > label')].map((l) => l.textContent.trim()),
  details: [...document.querySelectorAll('.modal details.block')].map((d) => d.querySelector('summary').textContent.trim()),
  preview: document.querySelector('.modal .preview')?.textContent.trim(),
})
```

预期：

```json
{"title":"编辑频道","labels":["频道 ID","频道名称","上游 URL","频道 Logo URL","tvg-id","tvg-name","上游类型","代理类型","代理地址","代理用户名","代理密码","请求头","预取覆盖","单次上限（0 = 不限）","并发数（1-32）"],"details":["回看配置","网络配置 · 留空则继承分组配置"],"preview":"http://127.0.0.1:9999/live.m3u8"}
```

末尾两个字段来自 `qa1/c1` 的预取覆盖（`预取开启`）。换一个没有预取覆盖的频道（如 `qa1/c3`）时，标签只有前 13 个。

人工确认：

- `频道 ID` 禁用，值为 `c1`。
- `频道 Logo URL` 值为 `/logo/CCTV1.png`。
- `tvg-id` 值为 `cctv1`，提示「留空则同频道 ID。」
- `上游类型` 选中 `静态 URL`。
- 预览框里有复制按钮。
- `回看配置` 折叠，摘要前有图标。

改 `上游类型` 为 `解析脚本`，确认新增 `脚本`、`脚本参数`、`缓存秒数` 三个字段，且预览文本变成 `python3 resolvers/…`。

改回 `静态 URL`。点开 `回看配置`，确认开关关闭。

按 `Esc` 关闭。

### 3.7 其余十屏

每屏只做一次「有页头 + 有关键卡片」的检查。控制台执行：

```js
(async () => {
  const out = [];
  for (const label of ['缓存状态','系统设置','安全管控','频道热度','定时录制','节目指南','台标管理','订阅聚合','回看管理','视频素材','运行日志']) {
    [...document.querySelectorAll('.sidebar .nav-item')].find((b) => b.textContent.trim() === label)?.click();
    await new Promise((r) => setTimeout(r, 400));
    out.push([label,
      document.querySelector('.page-head .title')?.textContent.trim() ?? null,
      document.querySelectorAll('.content .card').length,
      [...document.querySelectorAll('.content .card .section-title, .content .card h2')]
        .map((e) => e.textContent.trim()).slice(0, 8),
    ]);
  }
  return JSON.stringify(out, null, 1);
})()
```

预期：每屏第一项非空，第二项大于 0。

各屏关键标记：

| 分区 | 必须出现的标记 |
| --- | --- |
| 缓存状态 | 页头按钮 `刷新`、`清空缓存`；表头 `子目录`、`文件数` |
| 系统设置 | 页头按钮 `重新读取`、`保存设置`；卡片含 `服务器`、`面板认证`、`切片伪装` |
| 安全管控 | 页头按钮 `刷新`、`轮换 Token`；卡片 `访问控制`、`播放链接审计`、`IP 统计与封禁` |
| 频道热度 | `运行时长`、`总请求`、`订阅请求`、`切片请求`、`切片命中`、`上游失败` |
| 定时录制 | `新建录制任务`、`录制任务`；表头含 `名称`、`频道`、`时间`、`状态`、`产物`、`操作` |
| 节目指南 | 页头按钮 `浏览页面`、`刷新状态`、`重新生成`、`立即采集` |
| 台标管理 | `上传台标`、`缺台标的频道`；最大边默认 `512`，质量默认 `80` |
| 订阅聚合 | 页头按钮 `复制地址`、`保存`；卡片 `聚合源`、`源规则`、`全局排除` |
| 回看管理 | 表头 `分组`、`频道`、`缓冲`、`操作` |
| 视频素材 | `运行环境`、`ffmpeg` |
| 运行日志 | 级别下拉默认 `全部级别`；行数默认 `200` |

系统设置页另有两条检查：

1. 改任一输入框，页头按钮文本变成 `保存设置 *`。点 `重新读取` 后回落为 `保存设置`。
2. 网格之后固定有 `系统状态` 与 `备份与恢复` 两块。

### 3.8 截图留档

每屏截一张视口图（1440×1000），存到 `docs/tests/runs/<日期>/`。文件名用分区键：

```
dashboard.png  cache.png  groups.png  settings.png  security.png
stats.png      schedule.png  epg.png  logos.png  subs.png
livestream.png assets.png  logs.png
```

截前先 `window.scrollTo(0, 0)`。分区切换不会自动回顶。

## 4. 判定

| 编号 | 检查项 | 通过标准 |
| --- | --- | --- |
| P-01 | 视口 | `[innerWidth, innerHeight, devicePixelRatio]` 等于 `[1440,1000,1]` |
| P-02 | 导航数量与顺序 | 13 项，顺序与 §3.1 完全一致 |
| P-03 | 侧栏宽度 | `224`，误差不超过 1 |
| P-04 | 主内容起点 | 左边界 `224`，误差不超过 1 |
| P-05 | 选中态 | 每屏 `.nav-item.active` 等于刚点的导航标签 |
| P-06 | 页头标题 | 13 屏标题与 §3.2 表一致 |
| P-07 | 页头在主内容顶部 | `.page-head` 的 `y` 小于第一张 `.card` 的 `y` |
| P-08 | 概览区块 | 三个 `.section-title` 为 `实时指标`、`快速操作`、`系统信息` |
| P-09 | 分组页工具栏 | 七个操作，顺序与 §3.4 一致 |
| P-10 | 分组卡头结构 | `["icon folder","gname","gid","ops"]` |
| P-11 | 分组操作按钮 | 八个，顺序与 §3.4 一致 |
| P-12 | 频道行结构 | `["dot","ch-main","ch-ops"]` |
| P-13 | 频道主区结构 | `["ch-name","ch-url","ch-url local","ch-badges"]` |
| P-14 | 频道徽章 | 与 §3.4 表一致 |
| P-15 | 频道操作按钮 | 与 §3.4 表一致；DASH 频道多 `查看轨道` |
| P-16 | 状态点颜色 | `qa1/c3` 为红，其余为绿 |
| P-17 | 空分组文案 | `qa4` 显示 `这个分组还没有频道。` |
| P-18 | 搜索 | 搜 `CCTV` 只剩 `qa1` 且 3 个频道全在；搜 `zzz` 显示 `没有匹配的分组。` |
| P-19 | 分组弹窗字段 | 十一个标签，顺序与 §3.5 一致 |
| P-20 | 分组弹窗宽度 | `540`，居中误差不超过 2 |
| P-21 | 分组弹窗折叠 | 网络配置 `open=false` |
| P-22 | 频道弹窗字段 | 前 13 个标签顺序与 §3.6 一致；有预取覆盖时末尾多 `单次上限（0 = 不限）`、`并发数（1-32）` |
| P-23 | 频道弹窗折叠块 | `回看配置`、`网络配置 · 留空则继承分组配置` |
| P-24 | 脚本模式字段 | 切到 `解析脚本` 后出现 `脚本`、`脚本参数`、`缓存秒数` |
| P-25 | 其余十屏 | 每屏页头非空，卡片数大于 0，关键标记齐全 |
| P-26 | 设置页脏标记 | 改字段后按钮变 `保存设置 *`，`重新读取` 后回落 |
| P-27 | 模式徽章大写 | 计算样式 `text-transform` 为 `uppercase` |
| P-28 | 截图留档 | 13 张视口图齐全 |

通过条件：P-01 至 P-28 全部通过。

## 5. 记录模板

复制到 `docs/tests/runs/<日期>.md`。

```markdown
# 面板逐屏验收记录

## 环境
- 日期：
- 发布 tag：
- `version` 字段（`/panel/api/status`）：
- 二进制 SHA-256：
- 浏览器与版本：
- 视口：innerWidth / innerHeight / devicePixelRatio
- 实例：独立实例（端口 19898）/ 生产实例
- 截图目录：

## 结果
| 编号 | 检查 | 结果 | 实际现象 |
| --- | --- | --- | --- |
| P-01 | 视口 | | |
| P-02 | 导航数量与顺序 | | |
| P-03 | 侧栏宽度 | | |
| P-04 | 主内容起点 | | |
| P-05 | 选中态 | | |
| P-06 | 页头标题 | | |
| P-07 | 页头位置 | | |
| P-08 | 概览区块 | | |
| P-09 | 分组页工具栏 | | |
| P-10 | 分组卡头结构 | | |
| P-11 | 分组操作按钮 | | |
| P-12 | 频道行结构 | | |
| P-13 | 频道主区结构 | | |
| P-14 | 频道徽章 | | |
| P-15 | 频道操作按钮 | | |
| P-16 | 状态点颜色 | | |
| P-17 | 空分组文案 | | |
| P-18 | 搜索 | | |
| P-19 | 分组弹窗字段 | | |
| P-20 | 分组弹窗宽度 | | |
| P-21 | 分组弹窗折叠 | | |
| P-22 | 频道弹窗字段 | | |
| P-23 | 频道弹窗折叠块 | | |
| P-24 | 脚本模式字段 | | |
| P-25 | 其余十屏 | | |
| P-26 | 设置页脏标记 | | |
| P-27 | 模式徽章大写 | | |
| P-28 | 截图留档 | | |

## 结论
- 通过：N 项
- 不通过：N 项（列编号）
- 未验证：N 项（列编号与原因）
```

## 6. 失败时怎么报

先分清三类，处理方式不同。

| 情形 | 判定 | 怎么做 |
| --- | --- | --- |
| JSX 里有渲染代码，但元素缺失或顺序错 | 做坏了 | 按本节报 |
| JSX 里没有渲染代码，且 `PROGRESS.md` §3.5 登记「未做」 | 有意不做 | 记录，不报 |
| JSX 里没有渲染代码，文档也没登记 | 差异未登记 | 记为缺口，报 |

已知的有意不做：

| 项 | 依据 |
| --- | --- |
| 分组卡拖动把手 | `PROGRESS.md` §3.5。重排用上移/下移按钮 |
| 分组级 EPG 地址 | `PROGRESS.md` §3.5。EPG 走 `[[epg.sources]]` |
| 界面国际化 | `PROGRESS.md` §3.3。面板只有中文 |
| 登录页版本号 | 目标有，本实现没有；未登记为有意不做 |

不要因为 CSS 里有 `.grip` 样式就判为已做。JSX 不渲染该元素。

已知的复刻缺陷，不要报：

| 现象 | 说明 |
| --- | --- |
| 概览「下行 · 回源」恒为 `0 B` | 复刻目标缺陷（`src/http/routes/panel.rs` 固定写 0） |
| `share.share_token` 开关无效果 | 实测目标也不控制载荷，已定案不做 |
| 字幕样式表单无播放侧效果 | 本实现没有字幕出口，已定案不做 |

收集证据：

```bash
# 环境
uname -a > /tmp/tv-panel-env.txt
git describe --tags --always --dirty >> /tmp/tv-panel-env.txt
sha256sum /opt/tv-buddy/tv-buddy >> /tmp/tv-panel-env.txt

# 服务与配置
sudo tv-buddy status > /tmp/tv-panel-status.txt 2>&1
sudo tv-buddy log 300 > /tmp/tv-panel-log.txt 2>&1

# 面板数据
B=http://127.0.0.1:19898
T=$(curl -sS -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
     -d '{"username":"admin","password":"admin"}' \
   | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
curl -sS -H "Authorization: Bearer $T" "$B/panel/api/status" > /tmp/tv-panel-status.json
curl -sS -H "Authorization: Bearer $T" "$B/panel/api/config" > /tmp/tv-panel-config.json
curl -sS -H "Authorization: Bearer $T" "$B/panel/api/health" > /tmp/tv-panel-health.json
curl -sS -H "Authorization: Bearer $T" "$B/panel/api/groups/export" > /tmp/tv-panel-groups.json
```

贴配置前删掉敏感值：`panel.password_sha256`、`share.key`、`subtitle.translate.token`、`epg.translate.mtran_token`、各级 `proxy.password`。

收尾，删掉测试数据与独立实例：

```bash
for g in qa1 qa2 qa3 qa4; do
  curl -sS -X DELETE -H "Authorization: Bearer $T" "$B/panel/api/groups/$g"; echo
done
kill "$(cat /tmp/tv-buddy-panel-test/pid)"
rm -rf /tmp/tv-buddy-panel-test /tmp/tv-buddy-origin
```

上报内容必须包含：编号、期望值、实际值、截图文件名、`/tmp/tv-panel-env.txt`、`/tmp/tv-panel-groups.json`。
