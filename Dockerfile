FROM rust:1.85.0-slim AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --offline

FROM debian:bookworm-slim
RUN useradd --create-home --uid 10001 runner
COPY --from=build /build/target/release/kvrecover /usr/local/bin/kvrecover
USER runner
WORKDIR /work
ENTRYPOINT ["kvrecover"]
