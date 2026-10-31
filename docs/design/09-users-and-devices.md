# F-1 调研：mpd-hls 的用户功能

> 写作风格：ASD-STE100 简化技术写作。一句一个意思。主动语态。用简单现在时。

## 1. 调研对象

| 项 | 内容 |
| --- | --- |
| 服务 | 本机容器 `mpd-hls`，镜像 `charmingcheung000/mpd-hls:latest` |
| 地址 | `localhost:9100`，页面 `/users` |
| 产品名 | CharmingStreamer |
| 实现语言 | Rust。前端 React + Radix + TanStack Query |
| 存储 | **SQLite**，`/app/var/mpd-hls.db` |

调研方法：读数据库副本、读前端产物、读二进制字符串。三项交叉核对。

## 2. 数据模型

数据库有 19 张表。全部同构：

```sql
CREATE TABLE users (
  id TEXT PRIMARY KEY NOT NULL,
  sort_order BIGINT NOT NULL,
  config_json TEXT NOT NULL,   -- 实体转 JSON 存在这里
  updated_at_ms BIGINT NOT NULL
);
CREATE INDEX users_order_idx ON users (sort_order, id);
```

即「一行一个 JSON 文档」。实体字段在 `config_json` 里。

### 2.1 `users`

二进制里的 `struct User with 10 elements`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | UUID |
| `username` | string | 登录名，唯一 |
| `role` | enum | `admin` / `user` |
| `password_hash` | string | **argon2id**。格式 `$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>` |
| `allowed_group_ids` | string[] | 可见分组。空数组 = 全部 |
| `channel_filter_enabled` | bool | 是否启用频道级过滤 |
| `allowed_channel_ids` | string[] | 可见频道。只在上面为真时生效 |
| `web_ui_access` | bool | 能否登录面板 |
| `created_at_ms` | number | |
| `updated_at_ms` | number | |

管理员不看 `allowed_group_ids`。二进制里有 `cleared stale allowed_group_ids entries on user load`：
加载时把已不存在的分组 id 从用户里删掉。

### 2.2 `access_tokens`

`struct AccessToken with 7 elements`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `token` | string | **12 位**字母数字短串，如 `olLmSN5fYpXQ` |
| `user_id` | string | 所属用户 |
| `purpose` | enum | `device` / `web_ui_links` |
| `label` | string? | 设备名称。`web_ui_links` 类固定写 `WebUI channel links` |
| `created_at_ms` | number | |
| `expires_at_ms` | number? | `null` = 永不过期 |
| `last_seen_at_ms` | number? | 最近使用 |

本机实例：3 个用户，7 个 token。`device` 4 个，`web_ui_links` 3 个。

启动时清理过期 token：`purged expired access tokens on startup`。

## 3. API

### 3.1 用户

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| GET | `/api/users` | 列表 |
| POST | `/api/users` | 新建 |
| PUT | `/api/users/:id` | 修改（不含密码） |
| DELETE | `/api/users/:id` | 删除 |
| PUT | `/api/users/:id/password` | 重置密码 |
| GET | `/api/users/:id/export.m3u` | 导出该用户的订阅清单 |
| GET | `/api/users/channel-options` | 分组与频道选项树，供勾选 |

### 3.2 令牌

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| GET | `/api/users/:id/tokens` | 列表 |
| POST | `/api/users/:id/tokens` | 新建 |
| PUT | `/api/users/:id/tokens/:token` | 改名称或有效期 |
| DELETE | `/api/users/:id/tokens/:token` | 撤销 |

### 3.3 订阅入口

| 路径 | 用途 |
| --- | --- |
| `/sub/:token/playlist.m3u` | 用户订阅清单 |
| `/sub/:token/epg.xml` | 独立 EPG |
| `/sub/:token/epg.xml.gz` | 同上，压缩 |

### 3.4 当前登录者

`GET /api/me` 与 `GET /api/me/playlist-link`。

## 4. 面板结构

`/users` 页。两个入口由页面头按钮触发。

### 4.1 用户表

| 列 | 内容 |
| --- | --- |
| 用户名 | |
| 角色 | 管理员 / 普通用户 |
| 可见分组 | 分组名列表，或「全部」 |
| WebUI 登录 | 允许 / 禁止 |
| 创建时间 | |
| 操作 | 导出 m3u、复制 EPG 地址、重置密码、编辑、删除、令牌 |

空态文案：「还没有任何用户」。

### 4.2 新建 / 编辑弹窗

字段：

- 用户名（必填）
- 密码（必填，仅新建）
- 角色：管理员 / 普通用户
- 可见分组（多选。普通用户至少选一个）
- 只开放指定频道（开关。开启后出现频道勾选，带搜索「搜索授权分组内的频道」）
- 允许登录 WebUI（开关）

说明文案：

- 新建：「用户名与密码均为必填；普通用户至少需要一个可见分组。」
- 编辑：「修改用户名、角色或可见分组。密码请通过「重置密码」按钮修改。」
- WebUI 开关：「登录后只能查看授权频道、EPG，并复制安全播放链接。」
- 频道过滤：「开启后，频道列表、EPG、订阅与已有播放链接都只允许勾选的频道。」

### 4.3 令牌弹窗

字段：

- 设备名称（`label`）
- 有效期：`1h` / `6h` / `12h` / `7d` / `30d` / `90d` / `1y` / `2y` / `3y` / 永不过期 / 自定义
- 列表：每个令牌显示订阅地址，可复制、可撤销

订阅地址即 `/sub/<token>/playlist.m3u`。

## 5. 权限语义

| 行为 | 管理员 | 普通用户 |
| --- | --- | --- |
| 登录面板 | 是 | 仅 `web_ui_access=true` |
| 看分组 / 频道 | 全部 | 仅 `allowed_group_ids`；开了频道过滤再按 `allowed_channel_ids` |
| 看 EPG | 全部 | 同上范围 |
| 复制播放链接 | 是 | 是，只限授权频道 |
| 管理用户与令牌 | 是 | 否 |
| 订阅清单 | 全部 | 仅授权范围 |

关键约束：**过滤是「已有播放链接也失效」**，不是只隐藏列表项。频道过滤开启后，旧链接同样受检。

## 6. 与本实现的差距

| 项 | mpd-hls | 本实现现状 |
| --- | --- | --- |
| 账号 | 多用户，角色二分 | 单账号 `[panel].username`，SHA-256 口令 |
| 口令散列 | argon2id | SHA-256 十六进制 |
| 会话 | 内存 + 会话表 | 内存 `SessionStore` |
| 设备令牌 | 12 位短串，带用途与有效期 | 只有全局 `[auth].token` |
| 订阅入口 | `/sub/:token/playlist.m3u` | 同左，按用户可见范围过滤 |
| 每用户 EPG | `/sub/:token/epg.xml(.gz)` | 同左 |
| 面板分权 | 管理员 / 普通用户两级 | 同左：普通用户只见「我的订阅」 |
| 可见范围 | 分组 + 频道两级 | 同左 |
| 存储 | SQLite | 单库 `tv-buddy.db` |

## 7. SQLite 评估

### 7.1 结论

**建议用 SQLite，但不是「必须」。**

功能上全部能用 JSON 文件实现。决定因素是 `last_seen_at_ms`。

### 7.2 对比

| 方案 | 写入 | 并发 | 迁移 | 依赖 |
| --- | --- | --- | --- | --- |
| 写进配置文件 | 每次改用户都整份回写 | 需全局锁 | 改文件结构要迁移 | 无 |
| 独立 JSON 文件 | 整份回写 | 需全局锁 | 手写迁移 | 无 |
| SQLite | 单行 `UPDATE` | 事务 | 版本号与建表语句 | `rusqlite` + bundled |

### 7.3 为什么是 SQLite

1. **`last_seen_at_ms` 是高频写**。每次播放都可能更新一行。整份回写 JSON 会把每屏切片请求变成一次全量落盘。
2. **原子性**。删用户要同时删其令牌。JSON 方案要两文件一致，进程崩溃会半途而废。
3. **未来扩展**。设备数、令牌数会增长。SQLite 不需要先全读再全写。
4. 目标实现同样用 SQLite。表结构可以直接对齐，便于日后比对。

用户与令牌和配置**同一个库文件**：`<安装目录>/tv-buddy.db`（2026-10-31 配置也迁入 SQLite）。

### 7.4 若选 JSON 的退路

也可以做：`<cache>/users.json` 存用户，`<cache>/tokens.json` 存令牌，`last_seen` 只在内存累计、每分钟刷盘一次。代价是掉电会丢最近一分钟的活跃时间，且删除要保证两文件一致。

### 7.5 依赖验证（2026-10-30 实跑）

用发布构建相同的镜像 `rust:1-alpine` 实测 `rusqlite 0.41` + `bundled`：

| 项 | 结果 |
| --- | --- |
| 镜像自带 C 编译器 | 是。`/usr/bin/cc`、`/usr/bin/gcc` |
| amd64 编译 | 通过，21 秒 |
| arm64 编译 | 通过，8 秒 |
| 链接方式 | `statically linked`（`file` 确认） |

结论：不需要改 `scripts/build-release.sh`。镜像已带 gcc，`bundled` 直接可用。

新增依赖数：22 个 crate（含 `libsqlite3-sys`）。

`argon2` 用 Rust 实现，与 musl 无冲突，未实测。

一次性前置动作：在宿主机跑一次 `cargo fetch`，把新 crate 落进 `~/.cargo/registry`。
发布构建用 `--offline`，缓存缺包会失败。

## 8. 1:1 复刻的落点

| 层 | 做法 |
| --- | --- |
| 数据 | 单库 `tv-buddy.db`。表 `users`、`access_tokens`，字段按 §2 |
| 口令 | 用 `argon2` 校验。迁移时把现有 `[panel].password_sha256` 转成一个 admin 用户，口令不变 |
| 会话 | 登录成功签发会话。会话绑定 `user_id` |
| 令牌 | 12 位短串。播放与订阅按 401 语义分流 |
| 订阅入口 | 新增 `/sub/:token/playlist.m3u` 与 `.xml` / `.xml.gz` |
| 过滤 | 分组级 + 频道级两级。作用范围：清单、EPG、播放链接 |
| 面板 | 新增第 14 个分区「用户与设备」。表格列与弹窗字段按 §4 |

需要你确认的三点：

1. 口令散列是否从 SHA-256 换到 argon2id。换则旧口令要按第一次登录时重算一次。
2. 现有全局 `[auth].token` 保留还是废弃。保留则两套并存，废弃要写迁移说明。
3. 迁移后 `[panel].username` / `password_sha256` 是否保留为兜底登录。

## 9. 实现进度

### 9.1 已完成（2026-10-30）

| 层 | 落点 | 状态 |
| --- | --- | --- |
| 数据层 | `src/users/{model,password,store}.rs` | SQLite，表结构与 §2 一致 |
| 口令 | argon2id。不支持旧散列格式 | 完成 |
| 首次安装 | 无用户时建 `admin`，随机 16 位口令写入 `<cache>/admin_initial_credentials.txt`（0600） | 完成 |
| 登录 | `POST /panel/api/login` 查 `users` 表，会话绑定用户 | 完成 |
| 用户 API | `/panel/api/users` 全套，含 `channel-options`、`password` | 完成 |
| 令牌 API | `/panel/api/users/:id/tokens` 全套 | 完成 |
| 当前用户 | `GET /panel/api/me` | 完成 |
| 订阅入口 | `/sub/:token/playlist.m3u`，按可见范围过滤 | 完成 |
| 播放鉴权 | `?token=` 改为查 `access_tokens`，并校验用户可见该频道 | 完成 |
| 废弃全局令牌 | 删除 `/security/token` 与 `rotate`、`PlayTokenStore`、分享里的令牌同步。共享载荷与 link-info 的 token 字段恒为空 | 完成 |
| 面板分区 | 第 14 个分区「用户与设备」。列表 + 新建/编辑弹窗 + 设备令牌弹窗 | 完成 |
| 禁止全局订阅 | `/playlist.m3u` 与 `/{gid}/playlist.m3u` 返回 `404` 文本 | 完成（`v2026.7`） |
| 每用户 EPG | `/sub/:token/epg.xml(.gz)`；订阅头改指自己；`epg::load_generated_cached` 缓存解析结果 | 完成（`v2026.7`） |
| 面板分权 | `guard::admin_gate`；`PanelUser` 每次请求重读角色 | 完成（`v2026.7`） |
| 「我的订阅」页 | 普通用户登录后的唯一分区；订阅地址、EPG 地址、可见频道 | 完成（`v2026.7`） |
| 令牌维护 | 启动清理过期令牌；每 60 秒把 `last_seen_at_ms` 落盘 | 完成（`v2026.7`） |

### 9.2 待办

| # | 项 | 说明 |
| --- | --- | --- |
| 1 | ~~EPG 用户隔离~~ | 完成（`v2026.7`）：`/sub/:token/epg.xml(.gz)` 已实现，订阅头改指自己 |
| 2 | 面板分权 | 完成（`v2026.7`）：`guard::admin_gate` 拦非管理员的 `/panel/api/*`；普通用户只见「我的订阅」 |
| 3 | 管理 CLI | 完成：改为调用 `tv-buddy passwd` 写入 `users` 表，不再读 `panel username` / `password_sha256` |
| 5 | Docker 集成冒烟 | `smoke-dash-timeshift-docker.sh` 在容器内起服务后清单请求无响应。待查 |
| 6 | 切片级鉴权 | 切片、DASH、回看路径不校验令牌。撤销令牌只挡住入口，已开始的会话仍能拉切片 |

### 9.3 行为变化

**订阅只有一个入口**：`/sub/:token/playlist.m3u`。
全局 `/playlist.m3u` 与 `/{gid}/playlist.m3u` 返回 `404` 文本，指路用户订阅。

**EPG 跟随令牌**：`/sub/:token/epg.xml(.gz)` 只含该用户可见频道的节目。
公开 `/epg.xml[.gz]` 仍在，供面板与旧客户端使用。

**面板按角色分权**：普通用户只看到「我的订阅」页（订阅地址、EPG 地址、授权频道）。
其余 `/panel/api/*` 返回 `403`。角色每次请求从库重读，降级立即生效。

**归档组停播**：`access = "archived"` 的分组不出现在任何订阅与每用户 EPG 里。
