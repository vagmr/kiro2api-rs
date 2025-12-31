FROM rust:1.84-slim AS builder
WORKDIR /src
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
LABEL "language"="rust"
LABEL "framework"="axum"
WORKDIR /app

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

# 创建数据目录
RUN mkdir -p /app/data

COPY --from=builder /src/target/release/kiro-rs /app/kiro-rs

# 数据目录挂载点
VOLUME ["/app/data"]

EXPOSE 8080

ENV DATA_DIR=/app/data

CMD ["/app/kiro-rs"]