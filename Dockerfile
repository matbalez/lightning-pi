FROM rust:1.95-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY skills ./skills
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/app/target cargo build --locked --release --bin lightning-pi && cp target/release/lightning-pi /lightning-pi

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates gosu && rm -rf /var/lib/apt/lists/* && useradd --uid 10001 --no-create-home app
COPY --from=build /lightning-pi /usr/local/bin/lightning-pi
COPY entrypoint.sh /usr/local/bin/entrypoint
RUN chmod +x /usr/local/bin/entrypoint
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/entrypoint"]
