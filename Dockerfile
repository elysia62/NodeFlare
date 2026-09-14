# syntax=docker/dockerfile:1

ARG RUST_IMAGE=rust:1-alpine
ARG ALPINE_IMAGE=alpine:latest

FROM oven/bun:1.4.0@sha256:5ff609364c049b54eb0ff560ec96319729a972078ef2c755d758f0c6ef89c2d6 AS frontend
WORKDIR /build
COPY package.json bun.lock ./
COPY frontend/package.json ./frontend/package.json
RUN bun install --frozen-lockfile
COPY frontend/ ./frontend/
ARG NODEFLARE_VERSION=1.0.0
ENV VITE_NODEFLARE_VERSION=${NODEFLARE_VERSION}
RUN bun run --cwd frontend build

FROM ${RUST_IMAGE} AS backend
SHELL ["/bin/ash", "-eo", "pipefail", "-c"]
# Package versions follow security updates for the selected Alpine branch.
# hadolint ignore=DL3018
RUN apk add --no-cache build-base
WORKDIR /build
COPY backend/ ./backend/
COPY shared/ ./shared/
ARG NODEFLARE_VERSION=1.0.0
ARG TARGETARCH
ENV NODEFLARE_VERSION=${NODEFLARE_VERSION}
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,id=nodeflare-musl-target-${TARGETARCH},target=/build/backend/target,sharing=locked \
    rust_target="$(rustc -vV | sed -n 's/^host: //p')" \
    && RUSTFLAGS="-C target-feature=+crt-static" cargo build --manifest-path backend/Cargo.toml --release --locked --target "$rust_target" \
    && cp "backend/target/$rust_target/release/nodeflare" /usr/local/bin/nodeflare \
    && readelf -l /usr/local/bin/nodeflare > /tmp/nodeflare-program-headers \
    && ! grep -q INTERP /tmp/nodeflare-program-headers

FROM ${ALPINE_IMAGE}
# hadolint ignore=DL3018
RUN apk add --no-cache ca-certificates \
    && addgroup -S -g 10001 nodeflare \
    && adduser -S -D -H -u 10001 -G nodeflare -s /sbin/nologin nodeflare \
    && mkdir -p /etc/nodeflare \
    && chmod 700 /etc/nodeflare \
    && chown nodeflare:nodeflare /etc/nodeflare
COPY --from=backend /usr/local/bin/nodeflare /usr/local/bin/nodeflare
COPY --from=frontend /build/frontend/dist/ /opt/nodeflare/share/frontend/
COPY --from=frontend /build/frontend/admin-dist/ /opt/nodeflare/share/admin/
COPY LICENSE /usr/share/doc/nodeflare/LICENSE
WORKDIR /etc/nodeflare
USER 10001:10001
VOLUME ["/etc/nodeflare"]
EXPOSE 2206
ENTRYPOINT ["/usr/local/bin/nodeflare"]
CMD ["--config", "/etc/nodeflare/config.toml"]
