# ── Frontend build stage ──────────────────────────────────────────────
FROM node:22-alpine AS frontend
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# ── Rust build stage ─────────────────────────────────────────────────
FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev nodejs npm
WORKDIR /app

# Cache dependency build
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && \
    cargo build --release --target "$(uname -m)-unknown-linux-musl" 2>/dev/null || true && \
    rm -rf src

# Copy source and pre-built frontend
COPY . .
COPY --from=frontend /app/frontend/dist frontend/dist

# Build the final binary with bundled-ui, skip the npm build (already done)
ENV CODEX_WEB_SKIP_UI_BUILD=1
RUN cargo build --release --features bundled-ui --target "$(uname -m)-unknown-linux-musl" && \
    cp "target/$(uname -m)-unknown-linux-musl/release/codex-web" /codex-web

# ── Minimal runtime image ───────────────────────────────────────────
FROM alpine:3.21
RUN apk add --no-cache openssh
COPY --from=builder /codex-web /usr/local/bin/codex-web
EXPOSE 3000
ENTRYPOINT ["codex-web"]
