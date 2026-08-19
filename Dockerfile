FROM rust:1.97-slim-bookworm AS builder
WORKDIR /src
# Copy only build inputs so doc/fixture edits don't invalidate the layer.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY templates ./templates
COPY static ./static
COPY migrations ./migrations
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 app && mkdir /data && chown app /data
WORKDIR /app
COPY --from=builder /src/target/release/racetoturin /usr/local/bin/racetoturin
COPY live ./live
ENV RTT_BIND=0.0.0.0:8080 RTT_DB=/data/racetoturin.db
EXPOSE 8080
USER app
CMD ["racetoturin"]
