# 三步使用 pixman 和 o11 推流 mytvsuper（长期更新）

> **原文来源**：<https://linux.do/t/topic/1177960#p-10381941-mytvsuper-token-1>
> **作者**：chenyifaer（linux.do）
> **发布时间**：2025-11-17 00:35 UTC
> **抓取时间**：2026-11-07
> **抓取方式**：Discourse JSON 接口 `/t/topic/1177960.json`（帖 1 正文）
> **说明**：以下正文按原文顺序整理为 Markdown。文字未改写。仅转换 HTML 标签。原文标签为「纯水」「影视」。
> **帖子回复**：帖 2（Tim_Jiao，2025-11-17）表示收藏；帖 4（brucelau1987，2026-02-12）询问「还能使用吗？」。截至抓取时，作者未回复可用性。

---

【手工打字声明】

这个文章综合了各路大佬的文章，亲自操作并且填坑，力求看了就能上手操作，步骤只有三个，很简单！（需要略懂命令行操作docker）

### 步骤一：获取 MyTVSuper Token

这是访问 MyTVSuper 服务的第一步。

#### 1.1 注册 MyTVSuper 账户

- 访问 [MyTVSuper 官网](https://www.mytvsuper.com)。

- **注意：** 注册 MyTVSuper 账户需要**香港IP地址**和**香港手机号码**才能成功接收验证码并完成注册。

#### 1.2 获取 Session Token

- 注册完毕后登录，在**香港IP地址**的浏览器中访问以下 API 地址：

```auto
https://www.mytvsuper.com/api/auth/getSession/self/?sub=1
```

- 页面会返回一串 JSON 数据。查找并复制 `user:token` 的值，将其保存下来备用。

### 步骤二：部署并启动 Pixman

`pixman` 的作用是根据你的 MyTVSuper Token 生成包含直播流地址和解密密钥的 M3U 播放列表。

#### 2.1 安装 Docker 并配置 Pixman

在任何能访问 MyTVSuper 服务的机器上安装 Docker。**此机器的 IP 地址不限于香港。**

- 在选定的目录下，创建 `.env` 和 `docker-compose.yml` 文件。

- **创建 `.env` 文件**

```bash
echo "MYTVSUPER_TOKEN=xxxxx" > .env
```

请将 `xxxxx` 替换为您在步骤一中获取的 `user:token` 值。

- **创建 `docker-compose.yml` 文件**

```yaml
services:
  pixman:
    image: pixman/pixman
    restart: always
    container_name: pixman
    environment:
      - MYTVSUPER_TOKEN # Docker Compose 会从 .env 文件中获取这个变量的值
    ports:
      - "5050:5000"
```

- 在包含这两个文件的目录下，运行 Docker Compose 启动 Pixman 服务：

```bash
docker compose up -d
```

#### 2.2 测试 Pixman 服务

服务启动后，你可以通过 `curl` 命令测试是否能获取到 M3U 播放列表。

- 打开终端，执行以下命令：

```bash
curl http://你的机器IP:5050/mytvsuper.m3u
```

请将 `你的机器IP` 替换为运行 Docker 机器的实际 IP 地址。

- 如果一切正常，您应该会看到类似以下的输出（包含多个电视台的地址和解密信息）：

```m3u
#EXTM3U
#EXTM3U x-tvg-url="https://assets.livednow.com/epg.xml"
#EXTINF:-1 tvg-id="翡翠台" tvg-name="翡翠台" tvg-logo="https://assets.livednow.com/logo/翡翠台.png" group-title="綜合", 翡翠台
#KODIPROP:inputstream.adaptive.manifest_type=mpd
#KODIPROP:inputstream.adaptive.license_type=clearkey
#KODIPROP:inputstream.adaptive.license_key=aaaa:bbbb  <-- 这个 aaaa:bbbb 是我们需要的信息
http://你的机器IP:5050/mytvsuper/J
```

请注意 `license_key=aaaa:bbbb` 和 `http://你的机器IP:5050/mytvsuper/J`（或其他频道对应的地址），这些信息在下一步配置 `o11` 时会用到。将这些地址保存备用。

### 步骤三：部署并启动 O11

`o11` 的原理是通过 `pixman` 获得的加密前地址，将视频流解密后中转给你，成为没有加密的视频流。

#### 3.1 O11 运行环境要求

- **IP 地址限制：** 由于 MyTVSuper 的 CDN 限制，`o11` 必须运行在**香港地址的机器上**，或者通过**香港的代理服务器**运行在任意机器上。

- **架构限制：** `o11` 当前只有 `x86` 架构的 Docker 镜像，**不能在 ARM 架构的机器上运行**。

#### 3.2 部署 O11

在满足上述条件的机器上，创建 `docker-compose.yml` 文件：

```yaml
version: '3.8' 
services:
  o11:
    image: wechatofficial/o11:latest
    container_name: o11
    restart: always
    ports:
      - "5500:1234"
```

在包含此文件的目录下，运行 Docker Compose 启动 O11 服务：

```bash
docker compose up -d
```

#### 3.3 快速配置 O11

- 打开浏览器，访问 `http://你的香港机器IP:5500` （或代理地址）。

- **用户配置 (Users)**

- 在左侧导航栏选择 **Users**。

- 点击 **“Add User”**。

- 在 **“Username”** 栏输入用户名。

- 在 **“Password”** 栏设置密码。

- 勾选 **“Admin”** 和 **“Web Access”**。

- 最后点击 **“Save Users”** 保存配置。

- **电视节目添加 (Config)**

- 在左侧导航栏选择 **Config**。

- 在 **“New Provider”** 右侧的空白位置输入一个列表名称（例如 `MyTVSuper`），然后按回车键。

- 在该 Provider 下，点击 **“Add Stream”** 右侧的空白位置，输入直播源名称（例如 `翡翠台`），然后按回车键。

- 此时页面下方会出现刚添加的直播源名称。

- **“Mode”** 保持为 `live`。

- 在 **“Manifest”** 空白位置填入从 Pixman 获取的直播源地址。

**重要提示：** 即使 Pixman 和 O11 在同一台机器上运行，也不能使用 `localhost`（因为在docker中运行）。你需要填写 Pixman 服务的实际 IP 地址和端口。

例如：`http://你的Pixman机器IP:5050/mytvsuper/J`

- 在 **“Keys/CDM Scripts”** 填入 Pixman 输出中 `license_key` 的值，也就是 `aaaa:bbbb`。

- 在 **“Options”** 中，建议勾选 `On demand`（需要观看时才中转）和 `Speed up`（加速）。

- 要添加更多的频道，请重复此步骤。

#### 3.4 观看电视

- 配置完成后，在 O11 Web 界面的顶部导航栏点击 **“Playlist”**。

- 点击下拉小三角，选择 **“Full”**。

- 您将获得一个播放列表链接（通常是 `http://你的香港机器IP:5500/x.m3u`）。

- 将这个 `x.m3u` 链接导入到支持 M3U 播放列表的播放器（如 IPTV 播放器、Kodi、VLC 等）即可开始观看 MyTVSuper 的直播节目。
