# SDRTap

A live SDR (Software Defined Radio) tap for the [Zako3](https://zako.ac) Tap Hub. Connects to an `rtl_tcp` server, demodulates FM/AM radio in real-time, and streams Opus audio to listeners through the hub.


## How it works

```
rtl_tcp server
     │  raw I/Q bytes (u8, 2.4 MHz)
     ▼
SharedSdr — broadcasts 100 ms chunks to all active listeners
     │
     ▼
DDC (per listener) — NCO frequency shift + boxcar decimation → 240 kHz
     │
     ▼
Demodulate — FM discriminator or AM envelope detector
     │
     ▼
De-emphasis (FM) + decimate 240 kHz → 48 kHz PCM i16
     │  streaming WAV via tokio duplex pipe
     ▼
ffmpeg — WAV → OGG/Opus
     │
     ▼
Tap Hub → listeners
```

A single `rtl_tcp` connection captures a 2.4 MHz wide slice of spectrum. Each listener tunes to any station within that slice independently, with no additional hardware connections. Audio is never cached — every stream is live.

Audio sources are requested as `MODE:FREQ_MHZ`, e.g. `FM:101.1` or `AM:1.080`.

## Prerequisites

- An RTL-SDR compatible device with [`rtl_tcp`](https://osmocom.org/projects/rtl-sdr/wiki) running
- `ffmpeg` installed and on `PATH` (used for WAV → Opus encoding)
- A Zako3 Tap Hub account (`api.zako.ac` or self-hosted)
- Rust toolchain (for building from source)

## Start with docker compose
in [docker-compose.yml](./docker-compose.yml)
```yml
services:
  sdr-tap:
    build: .
    env_file: .env
    environment:
      SDR_TAP_ID: ${SDR_TAP_ID}
      SDR_API_TOKEN: ${SDR_API_TOKEN}
      SDR_CENTER_MHZ: ${SDR_CENTER_MHZ:-98.0}
      TAPHUB_ENDPOINT: ${TAPHUB_ENDPOINT:-api.zako.ac}
      RTLTCP_HOST: ${RTLTCP_HOST:-localhost}
      RTLTCP_PORT: ${RTLTCP_PORT:-1234}
    network_mode: host
    restart: unless-stopped
```
if you need pre-built image, use `ghcr.io/200mill/tap-sdr`
```diff
services:
  sdr-tap:
-    build: .
+    image: ghcr.io/200mill/tap-sdr
    env_file: .env
    environment:
      SDR_TAP_ID: ${SDR_TAP_ID}
      SDR_API_TOKEN: ${SDR_API_TOKEN}
      SDR_CENTER_MHZ: ${SDR_CENTER_MHZ:-98.0}
      TAPHUB_ENDPOINT: ${TAPHUB_ENDPOINT:-api.zako.ac}
      RTLTCP_HOST: ${RTLTCP_HOST:-localhost}
      RTLTCP_PORT: ${RTLTCP_PORT:-1234}
    network_mode: host
    restart: unless-stopped
```


## Configuration

All configuration is via environment variables. A `.env` file is supported.

| Variable | Required | Default | Description |
|---|---|---|---|
| `SDR_TAP_ID` | yes | — | Unique tap identifier |
| `SDR_API_TOKEN` | yes | — | Authentication token for the hub |
| `SDR_CENTER_MHZ` | yes | — | Center frequency to tune (e.g. `98.0`) |
| `TAPHUB_ENDPOINT` | no | `api.zako.ac` | Tap Hub address |
| `TAPHUB_SERVER_NAME` | no | — | TLS SNI override |
| `TAP_HEALTHCHECK_PORT` | no | — | HTTP health check port (for liveness probes) |
| `RTLTCP_HOST` | no | `localhost` | rtl_tcp server host |
| `RTLTCP_PORT` | no | `1234` | rtl_tcp server port |

Copy `.env.example` to `.env` and fill in the required values.

## Running

Start `rtl_tcp` with your SDR device:

```sh
rtl_tcp -a 0.0.0.0 -p 1234
```

Then run the tap:

```sh
SDR_TAP_ID=my-sdr-tap SDR_API_TOKEN=secret SDR_CENTER_MHZ=98.0 cargo run --release
```

Once connected, listeners can request any station within ±1.05 MHz of `SDR_CENTER_MHZ`.

## Docker

A `Dockerfile` and `docker-compose.yml` are included. Edit `.env` with your credentials, then:

```sh
docker compose up -d
```

The compose file uses `network_mode: host` so the container can reach `rtl_tcp` on the host.

## Supported modes

| Source format | Mode | Notes |
|---|---|---|
| `FM:101.1` | Wideband FM | 75 µs de-emphasis (Americas/Japan) |
| `WFM:101.1` | Wideband FM | alias for FM |
| `AM:1.080` | AM envelope | slow DC tracking |

## Project structure

```
src/
├── main.rs        entry point, env config, tap builder
├── shared_sdr.rs  single rtl_tcp connection, broadcast channel for I/Q chunks
├── sdr.rs         SdrTapHandler — TapHandler implementation, DDC/demod pipeline
├── rtltcp.rs      async rtl_tcp client (TCP, binary protocol)
└── demod.rs       FM/AM demodulator, de-emphasis, decimation, WAV header
references/        reference implementation of the Zako3 SDK tap pattern
```
