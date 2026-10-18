# 缓存布局与切片缓存窗口（动态）

| 项 | 值 |
| --- | --- |
| 目标版本 | **v0.8.2**（`d302316fb8`） |
| 副本 | `samples/v0.8.2/iptv-proxy-linux-arm64`（sha256 `2948ee1f…5a4b`） |
| 架构 | linux/arm64（Ubuntu 24.04，OrbStack 原生） |
| 日期 | **2026-10-17** |
| 环境 | 容器 `iptv82`；`samples/work/dyn-v0.8.2/` |
| 结论级别 | **观测** |
| 原始证据 | [`raw/v0.8.2-cache-window.txt`](raw/v0.8.2-cache-window.txt)、[`raw/v0.8.2-disguise.txt`](raw/v0.8.2-disguise.txt) |

> 目标返回的正文/日志按**不可信数据**处理。

## 结论摘要
1. 切片缓存生命周期**已复现**：首次（正在写入）→ `Cache-Control: public, max-age=300`；命中缓存 → `private, max-age=300`。**S-DIS-06 ✅**。
2. 频道入口清单 → `public, max-age=5, s-maxage=5`。
3. 切片 ID → 上游 URL 的映射存于 `cache/seg_url/`；**映射仅在解析入口清单时建立**。
4. 缓存子目录**仅在进程启动时创建**；删除子目录会使缓存写入失效。

## 1. 缓存目录布局
启动时创建 `samples/work/dyn-v0.8.2/cache/`：

```text

cache/{epg/epg/{raw,scripts},logos,m3u8,recordings,seg_bytes,seg_url,sub_url}

```

- `m3u8/`：入口清单缓存（键随模式不同）；`seg_bytes/`：切片字节；`seg_url/`：切片 ID → 上游 URL 映射；`sub_url/`：订阅地址。
- 目录只在**启动**时创建。删除子目录后，运行中的进程**不再**写缓存（切片请求退回 302）。

## 2. 切片 ID → 上游 URL 映射（`seg_url`）
代理模式首次解析清单后：

```text

seg_url/ea8680a1b61c17e4_e4b__429e6e7fadd605425410f8fd9369a5d1       -> http://host.docker.internal:9999/seg0.ts
seg_url/ea8680a1b61c17e4_e4b__429e6e7fadd605425410f8fd9369a5d1.meta  -> {"ttl_secs":300,"created_at":…}

```

- 文件名：`{频道前缀}_{切片哈希前 3 hex}__{尾哈希}`；尾哈希 = `sha256("{前缀}_{完整切片哈希}")[:32]`（见 [`01`](01-runtime-surface-and-disguise.md) §5）。内容 = 切片的**上游绝对 URL**；`.meta` = `{ttl_secs, created_at}`。
- 切片 URL 只在**入口清单被解析**后才可服务。入口清单来自缓存（未重解析）→ 不建映射 → 旧切片 ID 返回 **302**。（实测：清空 `seg_url`、保留 `m3u8` 后，已发布切片 URL 全部 302。）

## 3. 入口清单缓存（`m3u8`）

| 模式 | 键形 | 内容 |
| --- | --- | --- |
| `proxy` | `{gid}_{cid}__{32hex}` | 改写后清单（切片=本服务伪装路径） |
| `rewrite` | `rw_{gid}_{cid}__{32hex}` | 改写后清单（切片=上游绝对 URL） |
| `redirect` | `redirect_url_{gid}_{cid短}__{32hex}` | 解析出的上游地址；`ttl_secs=300` |

## 4. 切片 Cache-Control 生命周期（S-DIS-06 ✅）
过程：组 `gslow4`，上游延迟 0.3s；删除缓存**文件**、保留目录；拉入口 → 拉切片。

| 第几次 | `Cache-Control` |
| --- | --- |
| 第 1 次（正在写入/回源） | `public, max-age=300` |
| 第 2 次（命中缓存） | `private, max-age=300` |
| 第 3 次（命中缓存） | `private, max-age=300` |

- **三次独立复现一致**（见 raw）。语义：切片**回源写入**时用 `public`（允许下游/CDN 缓存）；**写入完成**后转 `private`（仅终端）。其余响应头：`content-type` 由后缀映射；`content-length` = 上游字节数（本例 1880）。
- 频道入口：`public, max-age=5, s-maxage=5`（≈ 清单刷新周期，`s-maxage` 供共享缓存）。快速上游（无延迟）下 `public` 窗口极短，通常只观测到 `private`；人为延迟可稳定捕获 `public` 阶段。

## 5. 未决 / 局限
- `public` 的**持续时长**取决于上游回源时间；**精确 TTL 逻辑**（写入完成判据）未测。
- 缓存键的 `__{32hex}` 推导（proxy/rewrite/redirect）**未解**。
- `seg_bytes` 的淘汰策略（容量/时间）**未验证**。仅 **arm64 + 单配置 + 特制上游**。

## 6. 复现

```bash

B=http://127.0.0.1:19899
find samples/work/dyn-v0.8.2/cache/seg_bytes samples/work/dyn-v0.8.2/cache/seg_url samples/work/dyn-v0.8.2/cache/m3u8 -type f -delete
curl -s -o /tmp/pe "$B/gslow4/s4.m3u8"        # 触发解析 + 预取
U=$(grep -oE 'http://127.0.0.1:19899/gslow4/s4/[0-9a-f]+_[0-9a-f]+\.[a-z]+' /tmp/pe | head -1)
curl -s -D - -o /dev/null "$U" | grep -i '^cache-control'   # 第 1 次：public
curl -s -D - -o /dev/null "$U" | grep -i '^cache-control'   # 第 2 次：private

```
