# IPTV Proxy（兼容重实现）

按逆向文档重实现 IPTV Proxy。目标：与 **v0.8.2**（linux-arm64 / linux-amd64）**drop-in 兼容**。

| 文档 | 内容 |
| --- | --- |
| [`docs/spec/`](docs/spec/) | 目标规格（`S-*`）与验证基线 |
| [`docs/findings/`](docs/findings/) | 逆向证据（`F-*`、`D-*`） |
| [`docs/design/`](docs/design/) | 本实现的方案（选型、架构、UI 参考） |
| [`PROGRESS.md`](PROGRESS.md) | 唯一进度权威 |

## 构建

```bash
# 1) 前端（Vite + React + TS → frontend/dist）
scripts/build-frontend.sh

# 2) 后端（Rust）
cargo build --release

# 交叉编译（静态 musl）
rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
```

## 运行

```bash
target/release/iptv-proxy config.toml
```

默认监听 `[::]:19899`。面板入口 `http://<host>:19899/panel`，默认口令 `admin` / `admin`。

## 测试

```bash
scripts/smoke-m0.sh   # 健康检查、登录、状态、配置读写、鉴权、兜底
scripts/smoke-m1.sh   # 分组/频道 CRUD、导出、订阅与 access 过滤、播放 Token
scripts/smoke-m2.sh   # 播放链路、切片伪装、缓存窗口、302 占位（含伪上游）
```

## 接口（已实现）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/health` | `200 text/plain` `ok` |
| `GET` | `/playlist.m3u` | 总订阅（只含 `access = "public"` 的组） |
| `GET` | `/{gid}/playlist.m3u` | 分组订阅（归档组 `404`） |
| `GET` | `/{gid}/{cid}.m3u8`（`.mpd`） | 入口清单（`public, max-age=5, s-maxage=5`） |
| `GET` | `/{gid}/{cid}/video/index.txt` | 媒体清单（`text/plain`，切片地址已伪装） |
| `GET` | `/{gid}/{cid}/<P>_<S>.<ext>` | 伪装切片（忽略 `Range`） |
| `POST` | `/panel/api/login` | 登录换取会话 token |
| `GET`/`PUT` | `/panel/api/config` | 读取 / 全量保存配置 |
| `GET` | `/panel/api/groups/export` | 导出全部分组（数组） |
| `GET` | `/panel/api/groups/{gid}/export` | 导出单个分组 |
| `POST` | `/panel/api/groups` | 新建分组 |
| `PUT`/`DELETE` | `/panel/api/groups/{gid}` | 局部更新 / 删除分组 |
| `POST` | `/panel/api/groups/{gid}/channels` | 新建频道 |
| `PUT`/`DELETE` | `/panel/api/groups/{gid}/channels/{cid}` | 局部更新 / 删除频道 |

## 目录

```text
src/
  main.rs          启动与日志
  state.rs         共享状态（ArcSwap 配置快照 + 改动串行化 + 缓存目录）
  config/          配置模型、加载、校验、原子保存
  playlist.rs      M3U 订阅渲染
  proxy/           清单改写、切片索引与字节缓存
  auth/            面板会话、播放 Token
  http/            路由、提取器、错误映射、内嵌静态资源
frontend/          Vite + React + TS 面板（4 页）
scripts/           构建与冒烟测试脚本
```

## 已知限制（M2）

- 仅 `proxy` 链路可用；`remux`/`dash` 暂走同一链路。`redirect`/`rewrite`/`passthrough` 在 M3 实现（`passthrough` 目前只在订阅里给上游原始地址）。
- 主清单上游按绝对地址透传，不递归伪装；`remux` 预热 `503`、`dash` 就绪判定未实现。
- 缓存无容量上限（与目标一致，见 F-015）；`cache/flush` 接口在 M2 之后补。
- 面板只实现概览与分组管理；其余分区显示为「待实现」。
- `/epg`、`/logo`、`/unban` 仍是骨架页；EPG 未接入，订阅不输出 `x-tvg-url`。
