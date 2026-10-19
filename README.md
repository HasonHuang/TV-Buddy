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
scripts/smoke-m0.sh
```

## 目录

```text
src/
  main.rs          启动与日志
  state.rs         共享状态（ArcSwap 配置快照）
  config/          配置模型、加载、校验、原子保存
  auth/            面板会话（播放 Token/封禁/限流在后续里程碑）
  http/            路由、提取器、错误映射、内嵌静态资源
frontend/          Vite + React + TS 面板（4 页）
scripts/           构建与冒烟测试脚本
```

## 已知限制（M0）

- 播放面（订阅、频道播放、切片伪装、缓存）未接入，在 M1–M3 实现。
- 面板只有登录、状态、配置读写；其余接口在 E3 定义后补齐。
- 前端 4 页中，`/epg`、`/logo`、`/unban` 目前是骨架页。
