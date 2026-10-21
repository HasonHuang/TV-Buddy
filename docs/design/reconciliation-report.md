# P0 对拍报告（本实现 ↔ 目标 v0.8.2）

- 日期：2026-10-20
- 目标：容器 `iptv82`，二进制 `/app/iptv-proxy`（v0.8.2，linux/amd64）。
- 本实现：`target/release/iptv-proxy`（同机 macOS/arm64，debug 与 release 行为一致）。
- 夹具：同一份伪上游（HLS 媒体清单 + 主清单 + 切片）与同一键面配置；只有 `bind`、`disk_path`、上游主机名不同。
- 工具：`scripts/compare-p0.sh`（启动两侧）、`scripts/compare_p0.py`（比对）。
- 结论：**22 项用例，差异 0**。

## 复现

```bash
scripts/compare-p0.sh
```

脚本会备份容器内配置、装入夹具、重启容器，跑完后恢复原配置并停止容器。

## 用例与结果

| # | 用例 | 目标与本实现 | 结果 |
| --- | --- | --- | --- |
| 1 | `/health` | `200` `ok` | 一致 |
| 2 | 总订阅：状态/类型/形状 | `200`；`application/x-mpegurl; charset=utf-8`；`attachment; filename="playlist.m3u"`；只含 `public` 组，条目带 48 hex Token | 一致 |
| 3 | 总订阅：首行与头行 | `#EXTM3U`，随后 `m3u_header_lines` | 一致 |
| 4 | 分组订阅：五种 `access` | `pg/hd/op` → `200`；`ar/ghost` → `404 text/plain` | 一致 |
| 5 | 入口·媒体清单上游 | `200` `text/plain; charset=utf-8`；`public, max-age=4, s-maxage=4`；正文为上游标签 + 2 条绝对伪装切片 | 一致 |
| 6 | 入口·主清单上游 | `200` `application/vnd.apple.mpegurl`；`no-cache, no-store`；`#EXT-X-VERSION:6`、`#EXT-X-INDEPENDENT-SEGMENTS`、最佳变体属性 | 一致 |
| 7 | 变体清单·媒体清单上游 | `302` → `error_url` | 一致 |
| 8 | 变体清单·主清单上游 | `200` `text/plain; charset=utf-8`；`public, max-age=6, s-maxage=6`；绝对伪装切片 | 一致 |
| 9 | 别名 `index.m3u8` | `200` `text/plain; charset=utf-8` | 一致 |
| 10 | 切片首写 | `200`；MIME 大类 `image/*`；`public, max-age=300` | 一致 |
| 11 | 切片命中 | `200`；`private, max-age=300` | 一致 |
| 12 | 切片字节 | 与上游文件同哈希 | 一致 |
| 13 | 切片 `Range` | `200` 全量（`Range` 被忽略） | 一致 |
| 14 | 未知频道/分组 + 合法 Token | `302` → `not_found_url` | 一致 |
| 15 | 未知频道/分组 + 无 Token | `302` → `token_failure_url` | 一致 |
| 16 | 归档组入口 | `302` → `not_found_url` | 一致 |
| 17 | Token 缺失/错误 | `302` → `token_failure_url` | 一致 |
| 18 | `open` 组免 Token | 无 Token、错 Token、对 Token 均为 `200` | 一致 |
| 19 | `redirect` | `302` 到上游 URL | 一致 |
| 20 | `rewrite` | `200` `text/plain; charset=utf-8`；`no-cache, must-revalidate`；切片绝对化 | 一致 |
| 21 | `passthrough` | 带 Token → `302` 上游；无 Token → `302` | 一致 |
| 22 | 未知路径 | `302` → `not_found_url` | 一致 |

## 对拍中发现并修正的差异

详见 F-018（`docs/findings/dynamic/15-contract-reconciliation.md`）。

| 差异 | 修正 |
| --- | --- |
| 代理入口恒返回自造主清单 | 按上游清单类型分支 |
| `video/index.txt` 在媒体清单上游下也返回 200 | 改 `302 error_url` |
| 入口正文用固定 `BANDWIDTH=2000000`、根相对路径 | 改用最佳变体属性与绝对地址 |
| 清单缓存窗口固定 5 秒 | 改取 `#EXT-X-TARGETDURATION` |
| 切片地址为根相对路径 | 改绝对地址 |
| 未知分组/频道恒跳 `not_found_url` | 无/错 Token 跳 `token_failure_url` |
| 未知路径返回 `404` | 改 `302 not_found_url` |
| `passthrough` 免 Token | 改校验 Token |
| `rewrite` 缓存头用入口值 | 改 `no-cache, must-revalidate` |
| 无 `index.m3u8` 别名 | 已补 |
| 未知面板接口被播放路由吞掉 | 加显式路由 |

## 有意的内部差异（不比对）

| 项 | 说明 |
| --- | --- |
| 伪装前缀 `<P>` | 目标算法未解（F-017）；本实现 `sha256("<gid>:<cid>")[:16]` |
| 伪装扩展名轮换 | 取值不同，但都取自 `[cache.disguise].formats` |
| 缓存文件名 | 目录名一致，文件命名不同 |
| 运维接口响应形状 | 目标只登记存在性（`design/03` §6.4） |
| 清单缺 `TARGETDURATION` | 目标未采证；本实现按 6 秒 |

## 覆盖范围与限制

- 覆盖 P0 数据面：代理链路、六种模式的行为、Token 与 `access`、缓存窗口、切片。
- 未覆盖：EPG、录制、推流、DASH 转封装、面板 UI 逐项交互、`remux`/`dash` 的实际转封装（需 `ffmpeg`）。
- 目标为 linux/amd64，本机跑 arm64 二进制；两侧都只经 HTTP 观测，不比较内部状态。

## P3 复验（2026-10-21）

命令：`scripts/compare-p0.sh`。结果：**22 项，差异 0**。

新增端点：

| 端点 | 结论 |
| --- | --- |
| `GET /subs/status` | 空配置下两侧都返回 `{"sources":[]}`；形状一致 |
| `POST /subs/{groups,preview}` | 目标行为未验；本实现形状自定（`design/03` §6.16） |
| `GET /panel/api/link-info` | 形状一致；`token` 在启用时返回当前值，目标样本为空串（有意差异） |

未重跑：面板 UI 逐项交互、录制与回放（依赖 `ffmpeg` 与运行环境）。
