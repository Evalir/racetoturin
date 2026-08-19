FROM rust:1.97-slim-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 app && mkdir /data && chown app /data
WORKDIR /app
COPY --from=builder /src/target/release/racetoturin /usr/local/bin/racetoturin
COPY fixtures ./fixtures
COPY config ./config
ENV RTT_BIND=0.0.0.0:8080 RTT_DB=/data/racetoturin.db
EXPOSE 8080
USER app
CMD ["racetoturin"]
