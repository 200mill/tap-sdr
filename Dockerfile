FROM rust:1.95-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev cmake g++ && rm -rf /var/lib/apt/lists/*

# FutureSDR's `futuredsp` dep declares `#![feature(float_algebraic)]` (now stabilized), which errors on
# the stable channel. RUSTC_BOOTSTRAP=1 lets the stable toolchain accept it (mirrors .cargo/config.toml).
ENV RUSTC_BOOTSTRAP=1

WORKDIR /build

# Cache dependencies before copying source
COPY Cargo.toml ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs && cargo build --release; rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ffmpeg ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/sdr-tap /usr/local/bin/sdr-tap

EXPOSE 7871

CMD ["sdr-tap", "audiotap"]
