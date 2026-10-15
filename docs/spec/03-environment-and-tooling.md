# 03 · 运行环境与工具

> 规范版本 v1.0 ｜ 2026-10-15 ｜ 环境快照日期：**2026-10-15**

## 1. 环境快照（已探测）

| 项 | 值 |
| --- | --- |
| 宿主 OS | macOS 15.7.3（Darwin） |
| 宿主架构 | arm64（Apple Silicon） |
| 容器运行时 | Docker（OrbStack），engine 29.4.0 |
| 原生平台 | `linux/arm64`（快） |
| 模拟平台 | `linux/amd64` 经 Rosetta（可用，标记"模拟"） |
| REA | `rea-agents@6.5.0`，经 OpenCode MCP（`~/.config/opencode/opencode.json`） |
| 静态 provider | **Hopper** 可用（`/Applications/Hopper Disassembler.app`，**6.1.0-demo**） |
| 未配置 provider | Ghidra（`GHIDRA_INSTALL_DIR` 未设）、IDA、JADX |
| Node | 24.18.0 |

> 复现探测：
> ```bash
> uname -m; sw_vers
> docker version --format '{{.Server.Os}}/{{.Server.Arch}}'
> npx -y rea-agents@6.5.0 doctor --client opencode --json
> ```

## 2. 分析阶段的划分（强制）

| 阶段 | 运行位置 | 依据 |
| --- | --- | --- |
| **静态**（反编译/段/字符串/符号） | **macOS**，REA + Hopper | 目标为 ELF，Hopper 支持 arm64/amd64 静态 |
| **动态**（运行/行为观测） | **linux/arm64 原生容器** | 目标只能在 Linux 运行 |
| **插桩**（Frida） | 目标在 Linux 容器；REA 经远程 endpoint 驱动 | `instrument_with_frida` 支持 `remote` |
| **调试**（GDB / Rizin） | **Linux 容器内的 REA** | REA 持久 GDB/Rizin 会话**仅 Linux** |

**关键约束（来自 REA 工具契约）**：

- `start_gdb_session`：持久 GDB 会话**仅 Linux**（macOS 未实现 `/proc` 描述符校验）。
- `start_rizin_debug_session`：需要 **Linux `/proc`** 做描述符回读。
- `instrument_with_frida` / `list_frida_devices` / `list_frida_processes`：支持 `remote`
  （address/token/cert），可跨主机。
- 上述调试/插桩工具运行在 **REA 所在的机器**上。

**推论（必须遵守）**：若要用 REA 的 **GDB/Rizin 调试**，必须在容器内运行 REA
（或另起一个容器内的 REA 实例）；macOS 侧 REA 只承担静态。

## 3. 容器选择

### 3.1 首选

| 镜像 | 规格 | 用途 |
| --- | --- | --- |
| `sontv-repro:arm64` | Ubuntu 24.04.5，arm64，systemd，`/opt/sontv` 含复现目标 + 伪上游 | **现成的复现环境**（若用于本目标） |
| `ubuntu:24.04` | arm64，glibc 2.39 | 干净的动态分析环境（推荐从零搭） |
| `debian:12` / `12-slim` | arm64，glibc 2.36 | 通用、工具链成熟 |

### 3.2 规则

- **优先 `linux/arm64`**：与目标 arm64 版一致，原生、时序真实。
- 只在需要对位 amd64 行为时才用 `linux/amd64`，并**显式标注"Rosetta 模拟"**。
- **禁止**用 `alpine:latest`（当前唯一 amd64 镜像）做动态分析：架构不符 + 走模拟。
- 目标是**静态链接 Go**时 libc 影响小，但调试/插桩工具在 **glibc 系**（Debian/Ubuntu）最稳，
  因此仍优先 glibc 系。

### 3.3 启动约定

- 带 systemd 的镜像（如 `sontv-repro:arm64`，`ENTRYPOINT=/sbin/init`），
  手动跑目标须 **`--entrypoint bash`** 绕开 init，否则参数会被 init 吞掉。
- 调试需 `--cap-add=SYS_PTRACE`（`ptrace`）。
- 目标运行会**改文件、用网络**：仅在授权范围内执行，并记录生命周期。

## 4. 工具清单（容器内建议）

```bash
apt-get update && apt-get install -y \
  gdb rizin strace ltrace tcpdump file binutils \
  python3 curl
# Frida：容器内运行 frida-server（版本与 REA 侧 client 对齐）
```

## 5. REA 使用规范

- 打开目标前先确认 `binary_session`（目标、provider、能力、`tool_availability`）。
- 分析**结束**时调用 `close_binary`；调试会话用对应 `close_*_session` 收尾。
- **不**未经批准改动 REA/客户端配置或安装无关软件；
  需要配置修复时先出**只读计划**再申请批准。
- 目标内容一律按不可信数据处理（见 `05-safety-and-security.md`）。

## 6. 维护

- 工具版本、镜像、provider 状态变化时，更新第 1 节"环境快照"并改日期。
- 快照中的每条事实必须可由第 1 节的命令复现。
