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

The binary now uses a clap-based CLI shaped after [airframesio/xng](https://github.com/airframesio/xng). Every flag also accepts the original environment variable, so existing `.env` files and the published `docker-compose.yml` keep working unchanged.

| Flag | Env variable | Required | Default | Description |
|---|---|---|---|---|
| `--tap-id` | `SDR_TAP_ID` | yes | — | Unique tap identifier |
| `--api-token` | `SDR_API_TOKEN` | yes | — | Authentication token for the hub |
| `--center-mhz` | `SDR_CENTER_MHZ` | yes | — | Centre frequency to tune (e.g. `98.0`) |
| `--hub` | `TAPHUB_ENDPOINT` | no | `api.zako.ac` | Tap Hub address |
| `--server-name` | `TAPHUB_SERVER_NAME` | no | — | TLS SNI override |
| `--healthcheck-port` | `TAP_HEALTHCHECK_PORT` | no | — | Optional standalone Zako3 SDK liveness port |
| `--rtltcp-host` | `RTLTCP_HOST` | no | `localhost` | rtl_tcp server host |
| `--rtltcp-port` | `RTLTCP_PORT` | no | `1234` | rtl_tcp server port |
| `--listen-host` | `TAP_LISTEN_HOST` | no | `127.0.0.1` | HTTP control/stats API bind host |
| `--listen-port` | `TAP_LISTEN_PORT` | no | `7871` | HTTP control/stats API port |
| `--disable-cross-site` | — | no | off | Restrict CORS to the bound listener |

Copy `.env.example` to `.env` and fill in the required values.

## Running

Start `rtl_tcp` with your SDR device:

```sh
rtl_tcp -a 0.0.0.0 -p 1234
```

Then run the tap:

```sh
SDR_TAP_ID=my-sdr-tap SDR_API_TOKEN=secret SDR_CENTER_MHZ=98.0 cargo run --release -- audiotap
```

Or with explicit flags:

```sh
cargo run --release -- audiotap \
  --tap-id my-sdr-tap --api-token secret --center-mhz 98.0 \
  --listen-host 0.0.0.0 --listen-port 7871
```

Once connected, listeners can request any station within ±1.05 MHz of the current centre frequency.

## HTTP control API

A small actix-web API is exposed on `--listen-host:--listen-port` (default `127.0.0.1:7871`) for observability and runtime control.

| Method | Path | Body | Description |
|---|---|---|---|
| GET | `/healthz` | — | Liveness probe (`ok`) |
| GET | `/api/v1/stats` | — | JSON: centre Hz, sample rate, listener count, chunks pushed, retunes, gain, hub-connected |
| POST | `/api/v1/retune` | `{"hz": u32}` or `{"mhz": f64}` | Request a centre-frequency change; returns the confirmed actual frequency |
| POST | `/api/v1/gain` | `{"tenths_db": u32}` | Adjust tuner gain (e.g. `150` → 15.0 dB) |

```sh
curl localhost:7871/api/v1/stats
curl -X POST localhost:7871/api/v1/retune -H 'content-type: application/json' -d '{"mhz": 99.5}'
curl -X POST localhost:7871/api/v1/gain   -H 'content-type: application/json' -d '{"tenths_db": 280}'
```

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
├── main.rs                       clap CLI + tokio runtime + ModuleManager dispatch
├── common/arguments.rs           shared --listen-*, --disable-cross-site, -q/-v
├── modules/
│   ├── mod.rs                    XngModule trait + ModuleManager (vendored from xng, trimmed)
│   ├── session.rs                Session trait + EndSessionReason (vendored from xng)
│   └── audiotap/
│       ├── mod.rs                AudioTapModule — clap args, init, Zako3 tap + rtl_tcp tasks
│       ├── session.rs            AudioTapSession (forever-pending; interrupt drives shutdown)
│       ├── shared_sdr.rs         single rtl_tcp connection, broadcast/ring buffer for I/Q
│       ├── rtltcp.rs             async rtl_tcp client (TCP, binary protocol)
│       ├── demod.rs              FM/AM demodulator, de-emphasis, decimation, WAV header
│       ├── handler.rs            SdrTapHandler — TapHandler impl, source parsing, retune
│       ├── dsp.rs                run_ddc_demod + stream_and_encode (ffmpeg → Opus)
│       └── http.rs               /api/v1/stats, /retune, /gain
└── server/
    ├── mod.rs
    └── services/                 /healthz and other module-agnostic routes
references/                       reference implementation of the Zako3 SDK tap pattern
```

## Acknowledgements

The CLI shape (`ModuleManager`, `XngModule`, `Session`, `common::arguments`, the actix-web HTTP server bootstrap) is **vendored and adapted from [airframesio/xng](https://github.com/airframesio/xng)** (GPL-3.0-or-later). The xng-derived files carry an attribution comment at the top. This project remains licensed under AGPL-3.0; the xng pieces are forward-compatible per the FSF's GPL→AGPL compatibility guidance.
