# 路由前缀分类 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把接口、播放清单、播放切片、静态资源、页面分别收敛到 `/api`、`/api/play`、`/media`、`/static`、`/page`，并保留旧路径别名一个主版本。

**Architecture:** 先加新路由与路径帮助函数，旧路由暂时保留。随后用最外层路径改写中间件把旧路径映射到新路由，最后删除旧路由挂载。播放面用 `Placement` 枚举强制「清单只进 `/api/play`，切片只进 `/media`」。

**Tech Stack:** Rust、Axum 0.7、React/Vite、现有 `scripts/smoke-*.sh` 冒烟脚本。

**Spec:** `docs/superpowers/specs/2026-11-22-route-prefix-taxonomy-design.md`

## Global Constraints

- 新前缀固定为 `/api`、`/api/panel`、`/api/sub`、`/api/play`、`/media`、`/static`、`/page`。
- `/api/*` 下不得返回切片正文；`/media/*` 下不得返回任何 HLS/DASH 清单。
- 旧路径在本次主版本内必须继续可用。旧路径与新路径的响应内容一致。
- 不改播放令牌、签名、IP 管控、缓存 TTL 的语义。
- 旧 `/logo/*`、`/assets/*` 持久化值必须继续显示。本次不做数据库批量改写。
- 迁移期 CDN 规则是叠加，不替换。未命中路径默认不缓存。
- 每个任务结束前运行：`cargo fmt --check`、`cargo clippy -- -D warnings`、相关测试。
- 前端改动后运行 `scripts/build-frontend.sh`，否则 `rust-embed` 仍用旧产物。
- 提交信息使用 `feat(paths): ...`、`refactor(http): ...`、`docs(paths): ...` 等前缀。

## Review Focus

- `/media/...` 下的 `index.m3u8`、`video/index.txt`、`dash-timeshift/*/playlist*` 必须返回 404；`/api/play/.../<seg>` 也必须返回 404。分类错误会让 CDN 缓存清单或让切片绕过缓存。
- 旧 `/l/<token>/<gid>/<cid>.<ext>` 清单与旧签名路径在迁移期必须由改写层正确分流：清单进 `/api/play`，切片进 `/media`，保留前缀不得被二次改写。
- 签名清单与签名切片必须分开：清单进 `/api/play`，切片进 `/media`；DASH 时移的 `playlist*` 与分片同理。
- 被封 IP 仍能加载 `/page/panel`、`/static/panel/*`、`/api/panel/*`、`/api/unban/*`，否则无法登录解封。
- 旧配置值 `/logo/<file>`、`/assets/convert/...` 必须出现在订阅、台标报告与素材列表中，不能因读时归一化缺失而消失。

---

### Task 1: 路径帮助函数与分类器

**Files:**
- Create: `src/paths.rs`
- Modify: `src/lib.rs`（加 `pub mod paths;`）
- Test: `src/paths.rs` 内的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: 无。
- Produces:
  - `pub const API: &str = "/api";`
  - `pub const PANEL_API: &str = "/api/panel";`
  - `pub const SUB_API: &str = "/api/sub";`
  - `pub const PLAY_API: &str = "/api/play";`
  - `pub const MEDIA: &str = "/media";`
  - `pub const STATIC: &str = "/static";`
  - `pub const PAGE: &str = "/page";`
  - `pub fn panel(path: &str) -> String`
  - `pub fn sub(path: &str) -> String`
  - `pub fn play(path: &str) -> String`
  - `pub fn media(path: &str) -> String`
  - `pub fn static_path(path: &str) -> String`
  - `pub fn page(path: &str) -> String`
  - `pub fn play_url(self_base: &str, gid: &str, cid: &str, slice: &str, rest: &str) -> String`
  - `pub fn media_url(self_base: &str, gid: &str, cid: &str, slice: &str, rest: &str) -> String`
  - `pub fn is_manifest_rest(rest: &str) -> bool`
  - `pub fn normalize_asset_url(path: &str) -> String`

- [ ] **Step 1: 写失败测试**

在 `src/paths.rs` 写以下测试。断言必须直接写出期望字符串。

```rust
#[test]
fn manifest_rest_is_classified() {
    for rest in [
        "index.m3u8",
        "video/index.txt",
        "video_720p/index.txt",
        "video_720p/index.m3u8",
        "dash-timeshift/video_720p/playlist",
        "dash-timeshift/video_720p/playlist.txt",
        "dash-timeshift/video_720p/playlist.m3u8",
    ] {
        assert!(is_manifest_rest(rest), "{rest}");
    }
    for rest in [
        "timeshift/a.ts",
        "mpd/tok/abc/init.mp4",
        "seg_000000000000.png",
        "video_720p/init.webp",
        "dash-timeshift/video_720p/12.m4s",
    ] {
        assert!(!is_manifest_rest(rest), "{rest}");
    }
}

#[test]
fn asset_urls_are_normalized() {
    assert_eq!(normalize_asset_url("/logo/a.png"), "/static/logo/a.png");
    assert_eq!(
        normalize_asset_url("/assets/convert/x/index.m3u8"),
        "/static/assets/convert/x/index.m3u8"
    );
    assert_eq!(normalize_asset_url("/static/logo/a.png"), "/static/logo/a.png");
    assert_eq!(normalize_asset_url("https://up/a.png"), "https://up/a.png");
}

#[test]
fn play_and_media_urls_use_distinct_prefixes() {
    assert_eq!(
        play_url("http://h", "g", "c", "l/1/sig", "index.m3u8"),
        "http://h/api/play/g/c/l/1/sig/index.m3u8"
    );
    assert_eq!(
        media_url("http://h", "g", "c", "l/1/sig", "seg.png"),
        "http://h/media/g/c/l/1/sig/seg.png"
    );
    assert_eq!(
        media_url("http://h/", "g", "c", "", "seg.png"),
        "http://h/media/g/c/seg.png"
    );
    assert_eq!(
        play_url("", "g", "c", "", "index.m3u8"),
        "/api/play/g/c/index.m3u8"
    );
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test paths -v`
Expected: 编译失败，`paths` 模块不存在。

- [ ] **Step 3: 实现路径帮助函数**

`src/paths.rs` 用 `format!` 拼接。`play_url` 与 `media_url` 先 `trim_end_matches('/')`，`slice` 为空时不留双斜杠。`is_manifest_rest` 按 §8.2 的清单形状判断。`normalize_asset_url` 只映射以 `/logo/` 或 `/assets/` 开头的本地路径，其他原样返回。

在 `src/lib.rs` 顶部加：

```rust
pub mod paths;
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test paths -v`
Expected: 3 个测试全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/paths.rs src/lib.rs
git commit -m "feat(paths): add route prefix helpers and classifiers"
```

---

### Task 2: 页面与静态资源新路径

**Files:**
- Modify: `src/http.rs`
- Modify: `src/logo.rs`
- Modify: `src/http/routes/logo.rs`
- Modify: `src/http/routes/media.rs`
- Modify: `src/http/routes/stream.rs`
- Modify: `src/http/static_assets.rs`
- Modify: `src/config/model.rs`
- Modify: `src/playlist.rs`
- Modify: `frontend/vite.config.ts`
- Modify: `frontend/src/assets.tsx`
- Modify: `frontend/src/groups.tsx`
- Modify: `frontend/src/logo.tsx`
- Modify: `scripts/smoke-paths.sh`（本任务创建）
- Modify: `scripts/smoke-logo.sh`
- Modify: `scripts/smoke-groups.sh`
- Modify: `scripts/smoke-security.sh`
- Modify: `scripts/smoke-epg.sh`
- Modify: `scripts/smoke-settings.sh`
- Modify: `scripts/verify-release.sh`
- Test: `scripts/smoke-paths.sh`、`scripts/smoke-logo.sh`

**Interfaces:**
- Consumes: `paths::static_path`, `paths::normalize_asset_url`。
- Produces:
  - `GET /page/panel`、`GET /page/panel/` → `static_assets::panel_page`
  - `GET /page/unban` → `static_assets::unban_page`
  - `GET /static/panel/*path` → `static_assets::asset`
  - `GET /static/logo/:file` → `logo::file`
  - `GET /static/assets/*path` → `media::served`

- [ ] **Step 1: 写失败的冒烟断言**

创建 `scripts/smoke-paths.sh`。脚本自起服务，不依赖外部实例：

```bash
#!/usr/bin/env bash
# 路径契约冒烟：自起服务，检查页面、静态资源与旧别名。
set -euo pipefail

cd "$(dirname "$0")/.."

BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19916}
WORK=$(mktemp -d)
CFG="$WORK/config.json"
DB="$WORK/tv-buddy.db"

cat > "$CFG" <<JSON
{
  "server": { "bind": "127.0.0.1:$PORT", "log_level": "warn" },
  "cache": { "disk_path": "$WORK/cache" },
  "auth": { "token_enabled": false }
}
JSON

"$BIN" config import "$CFG" --db "$DB" >/dev/null || exit 1
"$BIN" "$DB" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill "$PID" 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT

B="http://127.0.0.1:$PORT"
for _ in $(seq 1 100); do
    curl -fsS "$B/health" >/dev/null 2>&1 && break
    sleep 0.1
done

check() {
    code=$(curl -s -o /dev/null -w '%{http_code}' "$B$1")
    [ "$code" = "$2" ] || {
        echo "FAIL $1: $code != $2"
        exit 1
    }
}

check /page/panel 200
check /page/unban 200
check /static/panel/panel.html 200
check /logo/does-not-exist.png 404
check /static/logo/does-not-exist.png 404
```

Run: `sh scripts/smoke-paths.sh`
Expected: 新路径返回 404，脚本失败。

- [ ] **Step 2: 添加页面与静态路由**

在 `src/http.rs` 的公开面加：

```rust
.route("/page/panel", get(static_assets::panel_page))
.route("/page/panel/", get(static_assets::panel_page))
.route("/page/unban", get(static_assets::unban_page))
.route("/static/panel/*path", get(static_assets::asset))
.route("/static/logo/:file", get(routes::logo::file))
.route("/static/assets/*path", get(routes::media::served))
```

旧 `/panel`、`/unban`、`/__assets/*`、`/logo/*`、`/assets/*` 暂时保留。

- [ ] **Step 3: 更新前端资源基址、台标 URL 与缓存头**

`frontend/vite.config.ts`：

```ts
base: '/static/panel/',
```

`src/logo.rs` 与 `src/http/routes/logo.rs`：所有 `format!("/logo/...")` 改为 `paths::static_path(&format!("/logo/..."))`，包括变量 `file`、`safe`、`target_name`。

`src/http/routes/logo.rs::file`：响应头从 `public, max-age=86400` 改为 `public, max-age=3600`。台标文件名不带哈希，TTL 要短。

`src/http/routes/logo.rs::match_report`：提取 `fn logo_file(value: &str) -> Option<String>`，内部先 `paths::normalize_asset_url`，再取 `/static/logo/` 之后的部分。报告用它判断已用台标。旧 `/logo/a.png` 与新 `/static/logo/a.png` 都要计入已用，不能算缺失或坏引用。

`src/http/static_assets.rs`：给 `asset` 增加 `public, max-age=31536000, immutable`。`panel_page` 与 `unban_page` 仍用 `no-cache`。`/static/panel/*` 是 Vite 哈希产物，可以长缓存。

- [ ] **Step 4: 更新素材、持久化值与前端前缀**

`src/http/routes/media.rs`：`local` 改为 `paths::static_path(&format!("/assets/convert/{digest}/index.m3u8"))`；`served` 的缓存头从 `public, max-age=3600` 改为 `public, max-age=86400`。`remove_asset_url` 的 `strip_prefix("/assets/")` 改为 `strip_prefix("/static/assets/")`，否则归一化后的值删不掉文件。

`src/config/model.rs::asset_local`：签名改为 `pub fn asset_local(&self, url: &str) -> Option<String>`。查到映射值后调用 `paths::normalize_asset_url`，旧 `/assets/...` 也返回新前缀。更新两个调用点：

- `src/http/routes/stream.rs::redirect_asset`：`let target = config.asset_local(url).unwrap_or_else(|| url.to_string()); redirect(&target)`。
- `src/http/routes/media.rs::remove_asset_url`：`let local = config.asset_local(url);`，之后用 `local.as_deref()`。

`src/playlist.rs::push_entry`：`tvg-logo` 输出前调用 `paths::normalize_asset_url(&logo)`。

`frontend/src/assets.tsx`：`${location.origin}/assets/${name}` 改为 `/static/assets/${name}`；扫描配置时同时接受 `/assets/` 与 `/static/assets/`；后端返回新路径后优先用返回值。

`frontend/src/groups.tsx` 的台标输入占位符改为 `/static/logo/x.png` 或 `https://…`。`frontend/src/logo.tsx` 的「公开地址形如」文案改为 `/static/logo/<文件名>`。

`src/logo.rs` 的 URL 断言同步改为 `/static/logo/...`，例如 `assert_eq!(entry.url, "/static/logo/台标_1.png")`。

`scripts/smoke-logo.sh`：取图缓存头改为 `public, max-age=3600`；订阅台标断言改为 `tvg-logo="/static/logo/cctv1.png"`；增加 `/static/logo/cctv1.png` 200 检查。

`scripts/smoke-groups.sh`：面板页请求改为 `/page/panel`；资源匹配改为 `/static/panel/assets/panel-[A-Za-z0-9_.-]*\.js`。

同一模式还出现在 `scripts/smoke-logo.sh`、`scripts/smoke-security.sh`、`scripts/smoke-epg.sh`、`scripts/smoke-settings.sh`、`scripts/verify-release.sh`。统一把面板页 URL 改为 `/page/panel`，把资源匹配改为 `/static/panel/`。

- [ ] **Step 5: 构建前端并重启服务**

Run: `scripts/build-frontend.sh`
Run: `cargo build`
Run: `sh scripts/smoke-paths.sh`
Expected: 页面与静态资源检查全部通过。

- [ ] **Step 6: 运行单元测试**

Run: `cargo test paths -v`
Run: `cargo test logo -v`
Run: `cargo test playlist -v`
Run: `cargo test config -v`
Expected: PASS。补两个测试：

- `playlist`：频道 `logo` 存 `/logo/a.png` 时，订阅正文含 `tvg-logo="/static/logo/a.png"`。
- `config`：`assets.map` 存 `/assets/convert/x/index.m3u8` 时，`asset_local` 返回 `/static/assets/convert/x/index.m3u8`。
- `logo`：`logo_file("/logo/a.png")` 与 `logo_file("/static/logo/a.png")` 都返回 `Some("a.png")`；外部 URL 返回 `None`。

- [ ] **Step 7: 提交**

```bash
git add src/http.rs src/logo.rs src/http/routes/logo.rs src/http/routes/media.rs src/http/routes/stream.rs src/http/static_assets.rs src/config/model.rs src/playlist.rs frontend/vite.config.ts frontend/src/assets.tsx frontend/src/groups.tsx frontend/src/logo.tsx scripts/smoke-paths.sh scripts/smoke-logo.sh scripts/smoke-groups.sh scripts/smoke-security.sh scripts/smoke-epg.sh scripts/smoke-settings.sh scripts/verify-release.sh
git commit -m "feat(paths): serve pages and static assets from /page and /static"
```

---

### Task 3: 面板 API 双挂载

**Files:**
- Modify: `src/http.rs`
- Modify: `src/http/guard.rs`
- Modify: `src/http/routes/media.rs`
- Modify: `frontend/src/api.ts`
- Modify: `frontend/src/*.tsx`（直接 `fetch('/panel/api...')` 的文件）
- Test: `tests/http_contract.rs` 或 `scripts/smoke-paths.sh`

**Interfaces:**
- Consumes: `paths::PANEL_API`、`paths::panel`。
- Produces:
  - `fn panel_api_routes() -> Router<Arc<AppState>>`
  - `fn panel_relative(path: &str) -> Option<&str>`
  - `/api/panel/*` 与 `/panel/api/*` 使用同一套处理器与嵌套 fallback。

- [ ] **Step 1: 写失败测试**

在 `tests/http_contract.rs` 加：

```rust
#[tokio::test]
async fn panel_api_is_available_under_new_prefix() {
    let fx = fixture().await;
    let session = login(&fx).await;
    let response = fx
        .client
        .get(format!("{}/api/panel/status", fx.base))
        .header("authorization", format!("Bearer {session}"))
        .send()
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn panel_api_unknown_path_returns_json() {
    let fx = fixture().await;
    let session = login(&fx).await;
    let response = fx
        .client
        .get(format!("{}/api/panel/not-a-route", fx.base))
        .header("authorization", format!("Bearer {session}"))
        .send()
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(response.text().await.unwrap().contains("接口不存在"));
}
```

`fixture()` 与 `login()` 都是 `tests/http_contract.rs` 现有的辅助函数。

 Run: `cargo test --test http_contract panel_api_is_available_under_new_prefix -v`
Expected: 404。

- [ ] **Step 2: 提取面板路由**

把 `src/http.rs` 中所有 `/panel/api/...` 路由改写成相对路径，放进：

```rust
fn panel_api_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", post(routes::panel::login))
        // ... 其余面板路由，路径去掉 `/panel/api` 前缀
        .route("/subs/status", get(routes::subs::panel_status))
        .route("/subs/groups", post(routes::subs::panel_groups))
        .route("/subs/preview", post(routes::subs::panel_preview))
        .route("/subs/sync", post(routes::subs::panel_sync))
        .route("/epg/translate/status", get(routes::epg::translate_status))
        .route("/subtitle/list", get(routes::media::list))
        .fallback(routes::panel::api_missing)
}
```

`subs/*`、`epg/translate/*`、`subtitle/*` 原本在 `/panel/api/*` 之外。它们也必须进入面板嵌套路由，才能在 `/api/panel/*` 下提供。旧的 `/subs/*`、`/epg/translate/*`、`/subtitle/*` 路由暂时保留。

在 `router()` 中：

```rust
.nest("/panel/api", panel_api_routes())
.nest("/api/panel", panel_api_routes())
```

删除原来的 `/panel/api/*rest` 兜底路由。`api_missing` 仍返回 JSON 404。

- [ ] **Step 3: 更新守卫路径判断**

`src/http/guard.rs` 增加：

```rust
fn panel_relative(path: &str) -> Option<&str> {
    path.strip_prefix("/panel/api")
        .or_else(|| path.strip_prefix("/api/panel"))
}

fn ip_guard_exempt(path: &str) -> bool {
    path.starts_with("/page")
        || path.starts_with("/static/panel")
        || path.starts_with("/api/panel")
        || path.starts_with("/api/unban")
        || path.starts_with("/panel")
        || path.starts_with("/unban")
}
```

`needs_admin` 与 `feature_of` 先用 `panel_relative` 去掉前缀，再匹配相对子路径。开放列表改为相对路径 `/login`、`/logout`、`/me`；`my/` 前缀对普通用户开放。相对路径以 `subs/`、`subtitle/`、`epg/translate/` 开头的请求需要管理员，并受对应功能开关控制。

`ip_guard` 使用 `ip_guard_exempt(request.uri().path())` 判断是否跳过。

`src/http/guard.rs` 的测试模块补新前缀断言：`!needs_admin("/api/panel/login")`、`needs_admin("/api/panel/config")`、`feature_of("/api/panel/schedule", ...)` 与旧路径结果一致。

```rust
#[test]
fn page_and_auth_recovery_paths_are_ip_guard_exempt() {
    for path in [
        "/page/panel",
        "/static/panel/a.js",
        "/api/panel/status",
        "/api/unban/whoami",
    ] {
        assert!(ip_guard_exempt(path), "{path}");
    }
    for path in ["/api/health", "/media/g/c/seg.png", "/api/play/g/c.m3u8"] {
        assert!(!ip_guard_exempt(path), "{path}");
    }
}
```

`src/http/routes/media.rs::kind_of` 同时识别 `/panel/api/assets/`、`/api/panel/assets/`、`/panel/api/subtitle/`、`/api/panel/subtitle/`。`src/http/routes/media.rs` 里生成的 `download` 路径改为 `/api/panel/subtitle/translate/raw/{name}`。

保留 `needs_admin` 的 `PUBLIC_ADMIN` 与 `feature_of` 的 `/subs/`、`/subtitle/`、`/epg/translate/` 旧分支，直到 Task 10 删除旧路由。新增判断用 `panel_relative` 后的相对路径。

- [ ] **Step 4: 更新前端 API 基址**

`frontend/src/api.ts`：

```ts
const response = await fetch(`/api/panel${path}`, { ... });
```

`apiText` 同样改为 `/api/panel`。运行 `grep -rn '/panel/api' frontend/src`，更新所有代码与注释。至少包括：

- `src/schedule.tsx` 的下载 `fetch`。
- `src/assets.tsx` 的 raw 下载 `href` 与上传辅助 `fetch`。
- `src/ops.tsx` 的备份与恢复 `fetch`。

- [ ] **Step 5: 构建并运行测试**

Run: `scripts/build-frontend.sh`
Run: `cargo test --test http_contract panel_api -v`
Run: `cargo test http::guard -v`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add src/http.rs src/http/guard.rs src/http/routes/media.rs frontend/src/api.ts frontend/src tests/http_contract.rs
git commit -m "feat(panel): mount panel API under /api/panel"
```

---

### Task 4: 公开接口新路径

**Files:**
- Modify: `src/http.rs`
- Modify: `frontend/src/unban.tsx`
- Modify: `Dockerfile`
- Test: `scripts/smoke-paths.sh`

**Interfaces:**
- Consumes: `routes::public::{health,unban,whoami}`、`routes::share::pull`。
- Produces:
  - `GET /api/health`
  - `POST /api/unban`
  - `GET /api/unban/whoami`
  - `GET /api/share/pull`（保持）

- [ ] **Step 1: 写失败断言**

在 `scripts/smoke-paths.sh` 加：

```bash
check /api/health 200
check /api/unban/whoami 401
```

`/api/unban/whoami` 未登录返回 401，具体状态码按现有处理器。

Run: `sh scripts/smoke-paths.sh`
Expected: 新路径失败。

- [ ] **Step 2: 添加路由**

在 `src/http.rs` 加：

```rust
.route("/api/health", get(routes::public::health))
.route("/api/unban", post(routes::public::unban))
.route("/api/unban/whoami", get(routes::public::whoami))
```

`/api/share/pull` 已经是规范路径，不要重复注册。旧 `/health`、`/unban`、`/unban/whoami` 暂时保留。

- [ ] **Step 3: 更新解封页面与健康检查**

`frontend/src/unban.tsx`：

```ts
fetch('/api/unban/whoami', ...)
fetch('/api/unban', ...)
<a href="/page/panel">前往面板登录</a>
```

`Dockerfile` 健康检查改为 `http://127.0.0.1:9900/api/health`。

- [ ] **Step 4: 构建并运行**

Run: `scripts/build-frontend.sh`
Run: `cargo build`
Run: `sh scripts/smoke-paths.sh`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add src/http.rs frontend/src/unban.tsx Dockerfile scripts/smoke-paths.sh
git commit -m "feat(api): add public API routes under /api"
```

---

### Task 5: 播放面 Placement 门禁与新路由

**Files:**
- Modify: `src/http/routes/stream.rs`
- Modify: `src/http/routes/admin.rs`
- Modify: `src/http.rs`
- Modify: `scripts/smoke-m2.sh`
- Test: `src/http/routes/admin.rs`（单元测试）
- Test: `scripts/smoke-m2.sh`

**Interfaces:**
- Consumes: `paths::is_manifest_rest`。
- Produces:
  - `enum Placement { Play, Media, Legacy }`
  - `async fn play_wildcard(...)`
  - `async fn media_wildcard(...)`
  - `async fn signed_play(...)`
  - `async fn signed_media(...)`
  - 新路由：`/api/play/:gid/:file`、`/api/play/:gid/:cid/*path`、`/api/play/:gid/:cid/l/:exp/:sig/*path`、`/api/play/token/:token/:gid/:file`、`/api/play/token/:token/:gid/:cid/*path`、`/media/:gid/:cid/*path`、`/media/:gid/:cid/l/:exp/:sig/*path`

- [ ] **Step 1: 写失败断言**

`scripts/smoke-m2.sh` 已有 `p1/c1` 夹具。`PLAYTOKEN` 在 `# --- Token ---` 段生成，新断言必须放在该段之后：

```bash
check "新路径入口 200" "200" "$(status "$B/api/play/p1/c1.m3u8?token=$PLAYTOKEN")"
check "/media 清单 404" "404" "$(status "$B/media/p1/c1/index.m3u8")"
check "/api/play 切片 404" "404" "$(status "$B/api/play/p1/c1/seg_000000000000.png")"
```

Run: `sh scripts/smoke-m2.sh`
Expected: 新路径断言失败。

- [ ] **Step 2: 加 Placement 参数**

`src/http/routes/stream.rs`：

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum Placement { Play, Media, Legacy }
```

`serve_rest` 增加 `placement: Placement` 参数。开头用门禁函数：

```rust
fn placement_allows(placement: Placement, rest: &str) -> bool {
    match placement {
        Placement::Play => paths::is_manifest_rest(rest),
        Placement::Media => !paths::is_manifest_rest(rest),
        Placement::Legacy => true,
    }
}

if !placement_allows(placement, rest) {
    return (StatusCode::NOT_FOUND, "not found").into_response();
}
```

在 `stream.rs` 测试模块加：

```rust
#[test]
fn placement_rejects_manifests_under_media_and_segments_under_play() {
    assert!(!placement_allows(Placement::Media, "index.m3u8"));
    assert!(!placement_allows(Placement::Play, "seg_000000000000.png"));
    assert!(placement_allows(Placement::Play, "video/index.txt"));
    assert!(placement_allows(Placement::Media, "timeshift/a.ts"));
    assert!(placement_allows(Placement::Legacy, "index.m3u8"));
}
```

`wildcard`、`signed` 调用 `serve_rest(..., Placement::Legacy)`。

- [ ] **Step 3: 加新处理器与路由**

新增处理器，签名如下：

```rust
pub async fn play_wildcard(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((gid, cid, rest)): Path<(String, String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response

pub async fn media_wildcard(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((gid, cid, rest)): Path<(String, String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response

pub async fn signed_play(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((gid, cid, exp, sig, rest)): Path<(String, String, String, String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response

pub async fn signed_media(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((gid, cid, exp, sig, rest)): Path<(String, String, String, String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response
```

`play_wildcard` 与 `media_wildcard` 只把 `serve_rest` 的 placement 改为 `Play` 或 `Media`。`signed_play` 与 `signed_media` 先 `sign::verify`，再调用 `serve_rest`。

在 `src/http.rs` 加路由：

```rust
.route("/api/play/:gid/:file", get(routes::stream::entry))
.route("/api/play/:gid/:cid/*path", get(routes::stream::play_wildcard))
.route("/api/play/:gid/:cid/l/:exp/:sig/*path", get(routes::stream::signed_play))
.route("/api/play/token/:token/:gid/:file", get(routes::stream::token_entry))
.route("/api/play/token/:token/:gid/:cid/*path", get(routes::stream::token_alias))
.route("/media/:gid/:cid/*path", get(routes::stream::media_wildcard))
.route("/media/:gid/:cid/l/:exp/:sig/*path", get(routes::stream::signed_media))
```

- [ ] **Step 4: 更新请求统计路径解析**

`src/http/routes/admin.rs` 的 `group_of` 与 `channel_of` 只认旧路径。`/media/g1/c1/...` 会把 `media` 当分组；`/api/play/...` 会被 `api` 排除。加统一入口：

```rust
/// 把播放路径还原为计数用的 `{gid}/{cid}/...` 形状。
fn stats_rest(path: &str) -> Option<&str> {
    if let Some(rest) = path.strip_prefix("/media/") {
        return Some(rest);
    }
    if let Some(rest) = path.strip_prefix("/api/play/") {
        return (!rest.starts_with("token/")).then_some(rest);
    }
    let rest = path.trim_start_matches('/');
    (!rest.is_empty()).then_some(rest)
}
```

`group_of` 与 `channel_of` 都先用 `stats_rest(path)?` 取首段与第二段。`group_of` 的保留首段加 `static` 与 `page`。`/api/play/token/...` 是令牌入口，返回 `None`，与旧 `/l/...` 一样不归组。

加单元测试：

```rust
#[cfg(test)]
mod tests {
    use super::{channel_of, group_of};

    #[test]
    fn stats_parse_new_playback_paths() {
        assert_eq!(group_of("/media/g1/c1/seg.png").as_deref(), Some("g1"));
        assert_eq!(channel_of("/media/g1/c1/seg.png", "g1").as_deref(), Some("g1/c1"));
        assert_eq!(group_of("/api/play/g1/c1/index.m3u8").as_deref(), Some("g1"));
        assert_eq!(channel_of("/api/play/g1/c1/index.m3u8", "g1").as_deref(), Some("g1/c1"));
        assert_eq!(group_of("/api/play/g1/c1.m3u8").as_deref(), Some("g1"));
        assert_eq!(channel_of("/api/play/g1/c1.m3u8", "g1"), None);
        assert_eq!(group_of("/api/play/token/tk/g1/c1/index.m3u8"), None);
    }

    #[test]
    fn stats_skip_reserved_prefixes() {
        for path in ["/api/sub/t/playlist.m3u", "/static/panel/x.js", "/page/panel", "/api/panel/config", "/api/health", "/api/unban", "/"] {
            assert_eq!(group_of(path), None, "{path}");
        }
    }

    #[test]
    fn stats_keep_legacy_paths() {
        assert_eq!(group_of("/g1/c1/seg.png").as_deref(), Some("g1"));
        assert_eq!(channel_of("/g1/c1/seg.png", "g1").as_deref(), Some("g1/c1"));
    }
}
```

- [ ] **Step 5: 运行门禁与统计测试**

Run: `cargo test stats_ -v`
Run: `cargo test placement -v`
Run: `cargo test --test http_contract stream -v`
Run: `sh scripts/smoke-m2.sh`
Expected: 统计单测通过；`/api/play` 清单 200；`/media` 清单 404；`/api/play` 切片 404。

- [ ] **Step 6: 提交**

```bash
git add src/http/routes/stream.rs src/http/routes/admin.rs src/http.rs scripts/smoke-m2.sh
git commit -m "feat(stream): enforce play/media route placement"
```

---

### Task 6: 代理、remux、回看 URL 切换

**Files:**
- Modify: `src/http/routes/stream.rs`
- Modify: `src/proxy/media.rs`
- Modify: `src/remux.rs`
- Modify: `src/timeshift.rs`
- Modify: `scripts/smoke-m2.sh`
- Modify: `scripts/smoke-m3.sh`
- Modify: `tests/http_contract.rs`
- Test: `scripts/smoke-m2.sh`
- Test: `scripts/smoke-m3.sh`

**Interfaces:**
- Consumes: `paths::play`、`paths::media`。
- Produces:
  - `fn play_base(self_base: &str, gid: &str, cid: &str, slice: &str) -> String`
  - 清单内切片地址为 `/media/...`；变体清单地址为 `/api/play/...`。

- [ ] **Step 1: 写失败断言**

`scripts/smoke-m2.sh` 的入口段所有切片地址正则改为 `/media/p1/c1/`：

- `check "切片地址绝对"` 的正则。
- `FIRST_URL` 的 `grep -oE`。
- `P=$(grep -oE ...)`、`P_OPEN`、`P2` 三处片段提取。前缀变长后 `${P:7:16}` 偏移失效，改用 `sed -E 's|.*/([0-9a-f]{16})_.*|\1|'`。

在 `# --- Token ---` 段生成 `PLAYTOKEN` 后加：

```bash
BODY=$(curl -s "$B/api/play/p1/c1.m3u8?token=$PLAYTOKEN")
printf '%s' "$BODY" | grep -q "/media/p1/c1/" || {
    echo "FAIL manifest does not use /media"
    exit 1
}
```

Run: `sh scripts/smoke-m2.sh`
Expected: 清单仍输出旧 `/p1/c1/...`，失败。

- [ ] **Step 2: 拆分清单基址与媒体前缀**

`src/http/routes/stream.rs`：

```rust
fn play_base(self_base: &str, gid: &str, cid: &str, slice: &str) -> String
```

`play_base` 先算 `let suffix = if slice.is_empty() { String::new() } else { format!("/{slice}") };`，再拼 `format!("{}{}", self_base.trim_end_matches('/'), paths::play(&format!("/{gid}/{cid}{suffix}")))`。

`src/proxy/media.rs::rewrite_media`：用 `let dir_slice = if slice.is_empty() { String::new() } else { format!("/{slice}") };`（不要叫 `suffix`，会遮蔽同名函数），再把 `dir` 改为 `format!("{root}{}", crate::paths::media(&format!("/{gid}/{cid}{dir_slice}")))`。伪装切片地址因此变为 `/media/{gid}/{cid}/...`。单测（约 478、499 行）同步改为 `http://self/media/g1/c1/...`。

`master_entry` 的 `/video/index.txt` 用 `play_base`。`media_entry` 的切片地址由 `proxy::rewrite_media` 生成，见下一句。`proxy_mpd` 的 BaseURL 用 `local_mpd_prefix`。该函数先算 `let tail = if slice.is_empty() { format!("/{gid}/{cid}/mpd/") } else { format!("/{gid}/{cid}/{slice}/mpd/") };`，再返回 `format!("{}{}", self_base.trim_end_matches('/'), crate::paths::media(&tail))`；单测（约 2090 行）改为 `http://self/media/g/c/mpd/`。

`scripts/smoke-m3.sh` 的 `proxy MPD 改写 BaseURL` 正则改为 `<BaseURL>http://127.0.0.1:$PORT/media/mp/m1/mpd/...`。

- [ ] **Step 3: 改 remux 与 timeshift**

`src/remux.rs::playlist` 的 `dir` 改为 `crate::paths::media(&format!("/{gid}/{cid}{suffix}"))`，其中 `suffix` 为空或 `format!("/{slice}")`。

`src/timeshift.rs::playlist` 的 `dir` 改为 `crate::paths::media(&format!("/{gid}/{cid}{suffix}/timeshift"))`。

同文件单测同步：`remux.rs` 约 622、637 行改为 `/media/g/c/...`；`timeshift.rs` 约 536、537、543 行改为 `/media/g/c/...`。

`tests/http_contract.rs::slice_signature_path_token_and_rejections`：

- 主清单请求改为 `/api/play/token/{token}/pm/c1.m3u8`。
- 切片路径现在是 `http://host/media/pm/c1/l/<exp>/<sig>/<name>`，`split('/')` 为 10 段。`parts[8]` 是签名，`parts[9]` 是文件名。
- 无签名请求改为 `format!("{}/media/pm/c1/unsigned.png", fixture.base)`。

同文件的其他断言：

- `assert_disguised` 的前缀改为 `{base}/media/{gid}/c1/`。
- `master_upstream_entry_is_synthetic_master_playlist` 的变体地址断言改为 `{base}/api/play/pm/c1/video/index.txt`。

订阅正文的 `/l/` 断言属于 Task 8，本任务不改。

- [ ] **Step 4: 运行测试**

Run: `cargo test remux -v`
Run: `cargo test timeshift -v`
Run: `cargo test --test http_contract slice_signature -v`
Run: `sh scripts/smoke-m2.sh`
Run: `sh scripts/smoke-m3.sh`
Expected: 清单切片地址以 `/media/` 开头；MPD BaseURL 使用 `/media/`；签名清单与切片分开。

- [ ] **Step 5: 提交**

```bash
git add src/http/routes/stream.rs src/proxy/media.rs src/remux.rs src/timeshift.rs scripts/smoke-m2.sh scripts/smoke-m3.sh tests/http_contract.rs
git commit -m "feat(stream): generate /media segment URLs"
```

---

### Task 7: DASH 清单与切片 URL 切换

**Files:**
- Modify: `src/dash_hls.rs`
- Modify: `src/dash_timeshift.rs`
- Modify: `scripts/smoke-m3.sh`
- Test: `src/dash_hls.rs`、`src/dash_timeshift.rs` 单测
- Test: `scripts/smoke-m3.sh`

**Interfaces:**
- Consumes: `paths::play_url`、`paths::media_url`。
- Produces:
  - 轨道清单 `index.txt` / `index.m3u8` 使用 `/api/play/...`。
  - 初始化段、媒体段使用 `/media/...`。
  - `dash-timeshift` 清单返回的 `EXT-X-MAP` 与切片使用 `/media/...`。

- [ ] **Step 1: 改失败测试**

`src/dash_hls.rs` 现有断言 `signed.contains("http://self/g/c/l/123/abc/video_720p/index.txt")`。改为：

```rust
assert!(signed.contains("http://self/api/play/g/c/l/123/abc/video_720p/index.txt"));
```

同文件同步：

- 约 1244 行音频轨道清单改为 `/api/play/...`。
- 约 1272 行 `EXT-X-MAP` 改为 `http://self/media/g/c/video_720p/init.webp`。
- 约 1712 行切片改为 `/media/g/c/video_720p/1_`。

媒体段断言改为：

```rust
assert!(signed.contains("http://self/media/g/c/l/123/abc/video_720p/"));
```

`src/dash_timeshift.rs` 的 playlist 断言改为清单内切片以 `/media/` 开头。

`scripts/smoke-m3.sh`：

```bash
check "dash 轨道" "1" "$(grep -c '/api/play/dh/c6/video_360p/index.txt' "$WORK/body")"
check "dash 媒体清单切片地址" "1" \
  "$(curl -s "$B/api/play/dh/c6/video_360p/index.txt" | grep -c '/media/dh/c6/video_360p/')"
```

Run: `cargo test dash_hls -v`
Run: `cargo test dash_timeshift -v`
Expected: FAIL，仍是旧路径。

- [ ] **Step 2: 改用统一 URL 帮助函数**

`src/dash_hls.rs` 的两个本地函数都改为委托统一帮助函数：

- `media_url(self_base, gid, cid, track, slice)` 生成轨道清单地址，调用 `crate::paths::play_url(self_base, gid, cid, slice, &format!("{track}/index.txt"))`。
- `media_file_url(self_base, gid, cid, rest, slice)` 生成初始化段与切片地址，调用 `crate::paths::media_url(self_base, gid, cid, slice, rest)`。

函数名保留，避免改所有调用点。不要再手写 `/api/play` 或 `/media` 前缀。

- [ ] **Step 3: 改 DASH 时移**

`src/dash_timeshift.rs::playlist` 的 `dir` 改为 `crate::paths::media(&format!("/{gid}/{cid}{suffix}/dash-timeshift"))`，`suffix` 为空或 `format!("/{slice}")`。清单正文本身由 `/api/play` 路由返回，不需要改路由。单测约 1019–1028 行改为 `/media/g/c/dash-timeshift/...`；签名值改为 `/media/g/c/l/123/abc/dash-timeshift/...`。

- [ ] **Step 4: 运行测试**

Run: `cargo test dash_hls -v`
Run: `cargo test dash_timeshift -v`
Run: `sh scripts/smoke-m3.sh`
Expected: PASS；DASH 主清单使用 `/api/play/`，媒体清单切片使用 `/media/`。

- [ ] **Step 5: 提交**

```bash
git add src/dash_hls.rs src/dash_timeshift.rs scripts/smoke-m3.sh
git commit -m "feat(dash): split manifest and media URLs"
```

---

### Task 8: 订阅新路径与条目 URL

**Files:**
- Modify: `src/http.rs`
- Modify: `src/http/routes/sub.rs`
- Modify: `src/playlist.rs`
- Modify: `src/http/routes/users.rs`
- Modify: `scripts/smoke-m1.sh`
- Test: `tests/http_contract.rs` 或 `scripts/smoke-paths.sh`
- Test: `scripts/smoke-m1.sh`

**Interfaces:**
- Consumes: `paths::sub`、`paths::play`。
- Produces:
  - `GET /api/sub/:token/playlist.m3u`
  - `GET /api/sub/:token/:gid/playlist.m3u`
  - `GET /api/sub/:token/epg.xml`
  - `GET /api/sub/:token/epg.xml.gz`
  - `playlist::entry_url` 输出 `/api/play/token/<token>/<gid>/<cid>.<ext>`。
  - `users.rs` 的 `playlist_url` / `epg_url` 输出 `/api/sub/...`。

- [ ] **Step 1: 写失败断言**

在 `tests/http_contract.rs` 加：

```rust
#[tokio::test]
async fn subscription_uses_new_paths() {
    let fx = fixture().await;
    let response = fx
        .client
        .get(format!("{}/api/sub/{}/playlist.m3u", fx.base, fx.token))
        .send()
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("读正文");
    assert!(body.contains(&format!("/api/play/token/{}/", fx.token)));
    assert!(!body.contains("/l/"));
}
```

Run: `cargo test --test http_contract subscription_uses_new_paths -v`
Expected: 404 或仍输出 `/l/`。

- [ ] **Step 2: 加订阅路由**

在 `src/http.rs` 加：

```rust
.route("/api/sub/:token/playlist.m3u", get(routes::sub::playlist))
.route("/api/sub/:token/:gid/playlist.m3u", get(routes::sub::group_playlist))
.route("/api/sub/:token/epg.xml", get(routes::sub::epg))
.route("/api/sub/:token/epg.xml.gz", get(routes::sub::epg_gz))
```

旧 `/sub/...` 暂时保留。

- [ ] **Step 3: 改生成路径**

`src/playlist.rs`：

```rust
format!(
    "{root}{}",
    crate::paths::play(&format!("/token/{token}/{}/{}.{ext}", group.id, channel.id))
)
```

EPG 路径改为 `crate::paths::sub(&format!("/{token}/epg.xml.gz"))`。

`src/http/routes/users.rs`：

```rust
"playlist_url": crate::paths::sub(&format!("/{}/playlist.m3u", token.token)),
"epg_url": crate::paths::sub(&format!("/{}/epg.xml", token.token)),
```

`playlist_link` 与 `epg_link` 同步。

`scripts/smoke-m1.sh` 的订阅段改为新路径：

- `SUB="$B/api/sub/$PTOK/playlist.m3u"`，`GSUB="$B/api/sub/$PTOK/pub/playlist.m3u"`。
- EPG 首行断言改为 `http://127.0.0.1:$PORT/api/sub/$PTOK/epg.xml.gz`。
- 四处 `/l/$PTOK/` 改为 `/api/play/token/$PTOK/`。
- 增加旧订阅别名检查：`check "旧订阅别名 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' "$B/sub/$PTOK/playlist.m3u")"`。

`tests/http_contract.rs::slice_signature_path_token_and_rejections` 的订阅正文断言改为 `/api/play/token/{token}/pg/c1.m3u8`。运行 `grep -n '/l/{}/' tests/http_contract.rs`，把其余订阅正文断言（约在 851、879、909、2335 行）一并改为 `/api/play/token/{}/`。同时更新：

- `src/playlist.rs` 的 EPG 首行测试（约 403 行）改为 `http://host/api/sub/tok/epg.xml.gz`。
- `tests/http_contract.rs` 的 EPG 首行断言（约 1532 行）改为 `/api/sub/{token}/epg.xml.gz`。
- `tests/http_contract.rs` 的 `playlist_url` 断言（约 1061 行）改为 `/api/sub/{token}/playlist.m3u`。
- `src/playlist.rs` 的 `entry_url` 断言（约 353、384、430、450 行）改为 `http://host/api/play/token/...`。运行 `grep -n '/l/' src/playlist.rs` 确认没有遗漏。

- [ ] **Step 4: 运行测试**

Run: `cargo test --test http_contract subscription -v`
Run: `sh scripts/smoke-m1.sh`
Run: `sh scripts/smoke-paths.sh`
Expected: 新订阅路径 200；条目含 `/api/play/token/`；旧订阅别名 200。

- [ ] **Step 5: 提交**

```bash
git add src/http.rs src/http/routes/sub.rs src/playlist.rs src/http/routes/users.rs scripts/smoke-m1.sh tests/http_contract.rs
git commit -m "feat(sub): move subscriptions to /api/sub"
```

---

### Task 9: 旧路径改写中间件

**Files:**
- Create: `src/http/legacy.rs`
- Modify: `src/http.rs`
- Test: `src/http/legacy.rs` 单测 + `scripts/smoke-paths.sh`

**Interfaces:**
- Consumes: `paths::is_manifest_rest`。
- Produces:
  - `pub fn rewrite(method: &Method, path: &str) -> Option<String>`
  - `pub async fn middleware(request: Request, next: Next) -> Response`

- [ ] **Step 1: 写失败测试**

在 `src/http/legacy.rs` 写表驱动测试，逐条覆盖 §8.2：

```rust
#[test]
fn legacy_prefixes_are_rewritten() {
    let get = Method::GET;
    let post = Method::POST;
    assert_eq!(rewrite(&get, "/health").as_deref(), Some("/api/health"));
    assert_eq!(rewrite(&get, "/panel/api/status").as_deref(), Some("/api/panel/status"));
    assert_eq!(rewrite(&get, "/panel/api").as_deref(), Some("/api/panel"));
    assert_eq!(rewrite(&get, "/subs/status").as_deref(), Some("/api/panel/subs/status"));
    assert_eq!(
        rewrite(&get, "/epg/translate/status").as_deref(),
        Some("/api/panel/epg/translate/status")
    );
    assert_eq!(
        rewrite(&get, "/subtitle/list").as_deref(),
        Some("/api/panel/subtitle/list")
    );
    assert_eq!(rewrite(&get, "/sub/t/playlist.m3u").as_deref(), Some("/api/sub/t/playlist.m3u"));
    assert_eq!(
        rewrite(&get, "/sub/t/g/playlist.m3u").as_deref(),
        Some("/api/sub/t/g/playlist.m3u")
    );
    assert_eq!(
        rewrite(&get, "/l/t/g/c.m3u8").as_deref(),
        Some("/api/play/token/t/g/c.m3u8")
    );
    assert_eq!(rewrite(&get, "/logo/a.png").as_deref(), Some("/static/logo/a.png"));
    assert_eq!(rewrite(&get, "/assets/x/index.m3u8").as_deref(), Some("/static/assets/x/index.m3u8"));
    assert_eq!(rewrite(&get, "/__assets/x.js").as_deref(), Some("/static/panel/x.js"));
    assert_eq!(rewrite(&get, "/unban/whoami").as_deref(), Some("/api/unban/whoami"));
    assert_eq!(rewrite(&get, "/unban").as_deref(), Some("/page/unban"));
    assert_eq!(rewrite(&post, "/unban").as_deref(), Some("/api/unban"));
    assert_eq!(rewrite(&get, "/panel").as_deref(), Some("/page/panel"));
    assert_eq!(rewrite(&get, "/panel/").as_deref(), Some("/page/panel/"));
}

#[test]
fn playback_shapes_are_split_by_manifest_predicate() {
    let get = Method::GET;
    assert_eq!(
        rewrite(&get, "/g/c/index.m3u8").as_deref(),
        Some("/api/play/g/c/index.m3u8")
    );
    assert_eq!(
        rewrite(&get, "/g/c/l/1/sig/index.txt").as_deref(),
        Some("/api/play/g/c/l/1/sig/index.txt")
    );
    assert_eq!(
        rewrite(&get, "/g/c/l/1/sig/seg.png").as_deref(),
        Some("/media/g/c/l/1/sig/seg.png")
    );
    assert_eq!(
        rewrite(&get, "/g/c/seg.png").as_deref(),
        Some("/media/g/c/seg.png")
    );
    assert_eq!(
        rewrite(&get, "/g/c.m3u8").as_deref(),
        Some("/api/play/g/c.m3u8")
    );
    assert_eq!(
        rewrite(&get, "/g/c/video/index.txt").as_deref(),
        Some("/api/play/g/c/video/index.txt")
    );
    assert_eq!(
        rewrite(&get, "/g/c/dash-timeshift/v/playlist.m3u8").as_deref(),
        Some("/api/play/g/c/dash-timeshift/v/playlist.m3u8")
    );
    assert_eq!(
        rewrite(&get, "/g/c/dash-timeshift/v/12.m4s").as_deref(),
        Some("/media/g/c/dash-timeshift/v/12.m4s")
    );
}

#[test]
fn new_prefixes_and_guidance_paths_are_not_rewritten() {
    let get = Method::GET;
    for path in ["/api/share/pull", "/media/g/c/seg.png", "/static/logo/a.png", "/page/panel", "/"] {
        assert_eq!(rewrite(&get, path), None, "{path}");
    }
    assert_eq!(rewrite(&get, "/g/playlist.m3u"), None);
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test http::legacy -v`
Expected: 编译失败，模块不存在。

- [ ] **Step 3: 实现改写函数与中间件**

`rewrite` 按 §8.2 的顺序实现。先处理前缀，再检查保留前缀，最后判播放形状。两段路径只有第二段是入口文件名时才改写：扩展名为 `m3u8`、`mpd`、`txt`，或没有扩展名。`/<gid>/playlist.m3u` 保留不改写。`middleware` 只替换 `path_and_query` 的 path 部分，保留 query。用 `request.uri().clone().into_parts()` 与 `Uri::from_parts`。

在 `src/http.rs` 顶部加 `mod legacy;`。在 `router()` 的最外层加：

```rust
.layer(axum::middleware::from_fn(legacy::middleware))
```

它必须是最后一个 `.layer`，确保先于 IP 管控、功能开关与管理员检查执行。

- [ ] **Step 4: 运行测试与冒烟**

Run: `cargo test http::legacy -v`
Run: `sh scripts/smoke-paths.sh`
Expected: 全部通过。旧 `/health`、`/panel`、`/sub/...`、`/g/c.m3u8` 仍可用。

- [ ] **Step 5: 提交**

```bash
git add src/http/legacy.rs src/http.rs
git commit -m "feat(paths): rewrite legacy paths to new prefixes"
```

---

### Task 10: 删除旧路由挂载

**Files:**
- Modify: `src/http.rs`
- Modify: `scripts/smoke-paths.sh`
- Modify: `tests/http_contract.rs`
- Test: `scripts/smoke-paths.sh`、`tests/http_contract.rs`

**Interfaces:**
- Consumes: `legacy::middleware`。
- Produces: 旧路径只由改写中间件提供，不再重复注册。

- [ ] **Step 1: 加旧路径回归断言**

在 `scripts/smoke-paths.sh` 加：

```bash
check /health 200
check /panel 200
check /unban 200
check /logo/does-not-exist.png 404
check /api/health 200
check /page/panel 200
```

旧 `/panel/api/*` 需要会话，旧 `/sub/*` 与旧播放路径需要夹具。它们由 `smoke-m1.sh` 与 `smoke-m2.sh` 覆盖：`smoke-m1.sh` 仍用旧 `/panel/api` 登录并访问旧 `/sub` 别名；`smoke-m2.sh` 仍用旧 `/p1/c1.m3u8` 与旧切片路径。

Run: `sh scripts/smoke-paths.sh`
Expected: 当前全部通过（旧路由还在）。

- [ ] **Step 2: 删除旧路由**

从 `src/http.rs` 删除：

- `/health`
- `/unban/whoami` 与整个 `/unban` route。GET 页面与 POST 接口都由改写中间件提供。
- `/panel`、`/panel/`
- `/panel/api` 的旧 `nest`
- `/logo/:file`
- `/assets/*path`
- `/__assets/*path`
- `/sub/:token/...`
- `/l/:token/...`
- `/:gid/:cid/l/:exp/:sig/*path`
- `/:gid/:file`
- `/:gid/:cid/*path`
- `/subs/...`
- `/epg/translate/...`
- `/subtitle/...`

保留：

- `/` 首页跳转
- `/playlist.m3u`、`/:gid/playlist.m3u` 的 404 指路
- 所有新路由

- [ ] **Step 3: 加旧/新路径一致性测试**

在 `tests/http_contract.rs` 加 `legacy_paths_match_new_paths`。逐对请求并比较状态码、正文与 `cache-control`：

- `/health` 与 `/api/health`。
- `/panel` 与 `/page/panel`。
- `/panel/api/status` 与 `/api/panel/status`（带 `login(&fixture)` 的会话）。
- `/sub/{token}/playlist.m3u` 与 `/api/sub/{token}/playlist.m3u`。
- `/pm/c1.m3u8?token={token}` 与 `/api/play/pm/c1.m3u8?token={token}`。

再从主清单取第一个 `/media/` 切片地址，把 `/media/` 替换为 `/` 得到旧地址；两个地址的正文与状态码必须一致。

- [ ] **Step 4: 运行回归**

Run: `cargo test --test http_contract -v`
Run: `sh scripts/smoke-paths.sh`
Run: `sh scripts/smoke-m1.sh`
Run: `sh scripts/smoke-m2.sh`
Expected: 旧路径与新路径全部通过。

- [ ] **Step 5: 提交**

```bash
git add src/http.rs scripts/smoke-paths.sh tests/http_contract.rs
git commit -m "refactor(http): remove legacy route mounts"
```

---

### Task 11: 文档、CDN 规则与完整验收

**Files:**
- Modify: `docs/deploy/04-cdn.md`
- Modify: `docs/design/03-http-contract.md`
- Modify: `docs/tests/06-cdn-console.md`
- Modify: `README.md`
- Modify: `docs/deploy/01-install-linux.md`
- Modify: `docs/deploy/02-docker.md`
- Modify: `docs/deploy/03-nginx.md`
- Modify: `docs/deploy/05-operations.md`
- Modify: `install.sh`（NGINX 安装完成提示）
- Modify: `packaging/tv-buddy.sh`（`url` 命令输出）
- Modify: `scripts/nginx/default.conf`（注释中的路径示例）
- Modify: `scripts/smoke-cli.sh`（`url` 输出断言）
- Modify: `scripts/smoke-paths.sh`（加入全量检查）

**Interfaces:**
- Consumes: 前 10 个任务的最终路径。
- Produces: 运维文档与新 CDN 规则一致；冒烟脚本覆盖新/旧路径。

- [ ] **Step 1: 更新 CDN 文档**

`docs/deploy/04-cdn.md` 写入迁移期与迁移后规则。示例接口路径一并改为 `/api/panel/*`，包括缓存统计与 `reload`。

迁移期必须明确写「现有规则 A/B 全部保留，新增规则叠加」。现有 A/B 已覆盖 `/panel*`、`/unban*`、`/sub/*`、`/subs/*`、`/epg*`、`/subtitle/*`、`/health`、旧清单与旧切片。不要删除。

新增规则：

```text
不缓存：/api/* /page/* /l/* /logo/api/*
缓存：/media/* /static/panel/* /static/logo/* /static/assets/*
兜底：/* 不缓存
```

迁移后：

```text
不缓存：/api/* /page/* /
缓存：/media/* /static/panel/* /static/logo/* /static/assets/*
```

同时更新验证 URL：清单 `/api/play/...`，切片 `/media/...`，面板 `/page/panel`，台标 `/static/logo/...`。

- [ ] **Step 2: 更新 HTTP 契约与 README**

`docs/design/03-http-contract.md` 的路由表改为新路径，并加「旧路径别名」小节。

`README.md` 更新：

- 面板地址：`http://<host>:9900/page/panel`
- 订阅地址：`http://<host>:9900/api/sub/<令牌>/playlist.m3u`
- 健康检查：`/api/health`

- [ ] **Step 3: 更新部署文档**

`01-install-linux.md`、`02-docker.md`、`03-nginx.md`、`05-operations.md` 中的面板、健康检查、订阅与播放验证地址改为新路径。保留旧路径说明一句。`scripts/nginx/default.conf` 的注释示例同步改为 `/page/panel`、`/api/sub/...`、`/api/health`。

`install.sh` 的 NGINX 安装完成提示改为 `http://<主机>/page/panel`。`packaging/tv-buddy.sh` 的 `url` 命令输出改为 `/page/panel`；健康检查改用 `/api/health`。`scripts/smoke-cli.sh` 的 `url` 断言改为 `*":$NEW_PORT/page/panel"*`。

- [ ] **Step 4: 更新 CDN 验收测试**

`docs/tests/06-cdn-console.md` 的规则 A/B 改为新规则。迁移期章节保留旧规则，并注明「一个主版本后删除」。

- [ ] **Step 5: 全量验证**

Run:

```bash
scripts/build-frontend.sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
sh scripts/smoke-paths.sh
sh scripts/smoke-m0.sh
sh scripts/smoke-m1.sh
sh scripts/smoke-m2.sh
sh scripts/smoke-m3.sh
sh scripts/smoke-m4.sh
sh scripts/smoke-cli.sh
```

期望：全部通过。`smoke-paths.sh` 覆盖新页面、静态资源与旧页面别名；`smoke-m1.sh` 覆盖订阅与旧订阅别名；`smoke-m2.sh` 覆盖新清单/切片、`/media` 清单 404、`/api/play` 切片 404；`smoke-m3.sh` 覆盖 DASH 与 MPD。需要 ffmpeg 或外网的脚本按现有说明跳过。

- [ ] **Step 6: 提交**

```bash
git add docs README.md install.sh packaging/tv-buddy.sh scripts/nginx/default.conf scripts/smoke-cli.sh scripts/smoke-paths.sh
git commit -m "docs(paths): align deployment and CDN rules with new prefixes"
```
