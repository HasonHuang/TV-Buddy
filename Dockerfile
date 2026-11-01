# syntax=docker/dockerfile:1

# TV Buddy 容器镜像。
#
# 构建顺序：前端产物 → 静态 musl 二进制与发布包 → 运行镜像。
#
# 目标：
#   runtime     默认。可直接运行的服务镜像。
#   local-test  带发布包与 install.sh 的 Alpine + OpenRC 镜像。
#               供 tests/manual/local/docker-compose.yml 验证安装流程。

ARG ALPINE=alpine:3.21

# --- 1. 前端产物 ---
FROM node:20-alpine AS web
WORKDIR /src/frontend
# 先复制锁文件。依赖不变时复用安装层。
COPY frontend/package.json frontend/pnpm-lock.yaml ./
RUN npm install -g pnpm@9 >/dev/null
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY frontend/ ./
RUN pnpm build

# --- 2. 静态 musl 二进制与发布包 ---
FROM rust:1-alpine AS build
# 面板与管理 CLI 显示的版本。发布时由 CI 或 --build-arg 传入。
ARG TV_BUDDY_VERSION=dev
ENV TV_BUDDY_VERSION=${TV_BUDDY_VERSION}
WORKDIR /src

# 先只编译依赖。源码改动不会让依赖层失效。
COPY Cargo.toml Cargo.lock build.rs ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    mkdir -p src \
 && printf 'fn main() {}' > src/main.rs \
 && : > src/lib.rs \
 && cargo build --release --locked

# 真实源码与前端产物。build.rs 需要 frontend/dist 存在。
# 先删掉占位页，避免它混进 rust-embed。
RUN rm -rf frontend/dist
COPY --from=web /src/frontend/dist ./frontend/dist
COPY src ./src
COPY packaging ./packaging
COPY install.sh ./install.sh

# 编译与打包在同一个 RUN 内完成：target 是缓存挂载，结束后不可见。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
 && version="${TV_BUDDY_VERSION#v}" \
 && stage="/out/tv-buddy-${version}" \
 && mkdir -p /out/bin "$stage" \
 && install -m 0755 target/release/tv-buddy /out/bin/tv-buddy \
 && install -m 0755 target/release/tv-buddy "$stage/tv-buddy" \
 && install -m 0755 packaging/tv-buddy.sh "$stage/tv-buddy.sh" \
 && install -m 0755 install.sh "$stage/install.sh" \
 && install -m 0644 packaging/tv-buddy.service "$stage/tv-buddy.service" \
 && install -m 0755 packaging/tv-buddy.openrc "$stage/tv-buddy.openrc" \
 && tar -C /out -czf /out/tv-buddy.tar.gz "tv-buddy-${version}" \
 && sha256sum /out/tv-buddy.tar.gz > /out/tv-buddy.tar.gz.sha256

# --- 3. 安装流程测试镜像 ---
# 容器启动时执行 install.sh，走一遍真实安装流程。
# 由 tests/manual/local/docker-compose.yml 使用。
FROM alpine-openrc:latest AS local-test
COPY --from=build /out/tv-buddy.tar.gz /out/tv-buddy.tar.gz.sha256 /pkg/
COPY install.sh /pkg/install.sh

# --- 4. 运行镜像 ---
FROM ${ALPINE} AS runtime
ARG TV_BUDDY_VERSION=dev
# 1 表示安装 ffmpeg，供 DASH 录制与素材转换使用。
ARG WITH_FFMPEG=0
LABEL org.opencontainers.image.title="TV Buddy" \
      org.opencontainers.image.description="自托管 IPTV 代理服务" \
      org.opencontainers.image.version="${TV_BUDDY_VERSION}" \
      org.opencontainers.image.source="https://github.com/HasonHuang/TV-Buddy" \
      org.opencontainers.image.licenses="MIT"

# 固定 UID/GID。绑定挂载时便于设置目录属主。
RUN apk add --no-cache ca-certificates tzdata \
 && if [ "$WITH_FFMPEG" = "1" ]; then apk add --no-cache ffmpeg; fi \
 && addgroup -S -g 10001 tvbuddy \
 && adduser -S -D -H -u 10001 -G tvbuddy -h /data -s /sbin/nologin tvbuddy \
 && mkdir -p /data/cache /opt/tv-buddy \
 && chown -R tvbuddy:tvbuddy /data

COPY --from=build /out/bin/tv-buddy /opt/tv-buddy/tv-buddy
RUN chmod 0755 /opt/tv-buddy/tv-buddy

WORKDIR /data
VOLUME ["/data"]
EXPOSE 9900
USER tvbuddy

# 健康检查走本机回环。默认 bind 是 [::]，同时接受 IPv4。
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -q -O /dev/null http://127.0.0.1:9900/health || exit 1

ENTRYPOINT ["/opt/tv-buddy/tv-buddy"]
CMD ["/data/tv-buddy.db"]
