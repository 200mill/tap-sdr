# SDRTap

A live SDR (Software Defined Radio) tap for the [Zako3](https://zako.ac) Tap Hub. Connects to an SDR device, demodulates FM/AM radio in real-time, and streams Opus audio to listeners through the hub.

Supports **RTL-SDR via `rtl_tcp`** out of the box, and any hardware supported by **SoapySDR** (HackRF, USRP, AirSpy, LimeSDR, …) when built with `--features soapy`.


## How it works

```
SDR device (rtl_tcp or SoapySDR)
     │  raw I/Q f32, 2.4 MHz
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

A single SDR connection captures a 2.4 MHz wide slice of spectrum. Each listener tunes to any station within that slice independently, with no additional hardware connections. Audio is never cached — every stream is live.

Audio sources are requested as `MODE:FREQ_MHZ`, e.g. `FM:101.1` or `AM:1.080`.

## Prerequisites

- An SDR device (see [SDR drivers](#sdr-drivers) below)
- `ffmpeg` installed and on `PATH` (used for WAV → Opus encoding)
- A Zako3 Tap Hub account (`api.zako.ac` or self-hosted)
- Rust toolchain (for building from source)

## SDR drivers

### rtl_tcp (default)

Any RTL-SDR compatible device (RTL2832U chipset) via the [`rtl_tcp`](https://osmocom.org/projects/rtl-sdr/wiki) network server. No extra build dependencies.

```sh
rtl_tcp -a 0.0.0.0 -p 1234
cargo run --release
```

Set `SDR_DRIVER=rtltcp` (or omit it — this is the default) and configure `RTLTCP_HOST` / `RTLTCP_PORT`.

### SoapySDR

Supports virtually any SDR hardware: HackRF, USRP, AirSpy, LimeSDR, RTL-SDR (via SoapySDR driver), and more. Requires `libsoapysdr-dev` and the relevant device driver installed on the host.

```sh
cargo build --release --features soapy
```

Then set `SDR_DRIVER=soapy` and `SOAPY_ARGS` to identify your device:

| Hardware | `SOAPY_ARGS` |
|---|---|
| RTL-SDR | `driver=rtlsdr` |
| HackRF One | `driver=hackrf` |
| USRP | `driver=uhd,serial=xxx` |
| AirSpy | `driver=airspy` |
| LimeSDR | `driver=lime` |


## Configuration

All configuration is via environment variables. A `.env` file is supported.

| Variable | Required | Default | Description |
|---|---|---|---|
| `SDR_TAP_ID` | yes | — | Unique tap identifier |
| `SDR_API_TOKEN` | yes | — | Authentication token for the hub |
| `SDR_CENTER_MHZ` | yes | — | Center frequency to tune (e.g. `98.0`) |
| `SDR_DRIVER` | no | `rtltcp` | SDR backend: `rtltcp` or `soapy` |
| `SDR_GAIN_DB` | no | `15.0` | Tuner gain in dB |
| `TAPHUB_ENDPOINT` | no | `api.zako.ac` | Tap Hub address |
| `TAPHUB_SERVER_NAME` | no | — | TLS SNI override |
| `TAP_HEALTHCHECK_PORT` | no | — | HTTP health check port (for liveness probes) |
| `RTLTCP_HOST` | no | `localhost` | rtl_tcp server host (`SDR_DRIVER=rtltcp`) |
| `RTLTCP_PORT` | no | `1234` | rtl_tcp server port (`SDR_DRIVER=rtltcp`) |
| `SOAPY_ARGS` | when `soapy` | — | SoapySDR device args (e.g. `driver=hackrf`) |

Copy `.env.example` to `.env` and fill in the required values.

## Running

### rtl_tcp

Start `rtl_tcp` with your device:

```sh
rtl_tcp -a 0.0.0.0 -p 1234
```

Then run the tap:

```sh
SDR_TAP_ID=my-sdr-tap SDR_API_TOKEN=secret SDR_CENTER_MHZ=98.0 cargo run --release
```

### SoapySDR

```sh
SDR_TAP_ID=my-sdr-tap SDR_API_TOKEN=secret SDR_CENTER_MHZ=98.0 \
  SDR_DRIVER=soapy SOAPY_ARGS="driver=hackrf" \
  cargo run --release --features soapy
```

Once connected, listeners can request any station within ±1.05 MHz of `SDR_CENTER_MHZ`.

## Start with docker compose

In [docker-compose.yml](./docker-compose.yml):
```yml
services:
  sdr-tap:
    build: .
    env_file: .env
    environment:
      SDR_TAP_ID: ${SDR_TAP_ID}
      SDR_API_TOKEN: ${SDR_API_TOKEN}
      SDR_CENTER_MHZ: ${SDR_CENTER_MHZ:-98.0}
      SDR_DRIVER: ${SDR_DRIVER:-rtltcp}
      SDR_GAIN_DB: ${SDR_GAIN_DB:-15.0}
      TAPHUB_ENDPOINT: ${TAPHUB_ENDPOINT:-api.zako.ac}
      RTLTCP_HOST: ${RTLTCP_HOST:-localhost}
      RTLTCP_PORT: ${RTLTCP_PORT:-1234}
    network_mode: host
    restart: unless-stopped
```

To use the pre-built image (`ghcr.io/200mill/tap-sdr`), replace `build: .` with `image: ghcr.io/200mill/tap-sdr`. Note: the pre-built image only includes the default `rtltcp` driver. Build from source with `--features soapy` for SoapySDR support.

## Supported modes

| Source format | Mode | Notes |
|---|---|---|
| `FM:101.1` | Wideband FM | 75 µs de-emphasis (Americas/Japan) |
| `WFM:101.1` | Wideband FM | alias for FM |
| `AM:1.080` | AM envelope | slow DC tracking |

## Project structure

```
src/
├── main.rs              entry point, env config, driver selection, tap builder
├── sdr_source.rs        SdrSource trait — common interface for all SDR backends
├── rtltcp.rs            RTL-TCP client + RtlTcpSource (default driver)
├── soapysdr_source.rs   SoapySdrSource (--features soapy)
├── shared_sdr.rs        single SDR connection, broadcast channel for I/Q chunks
├── sdr.rs               SdrTapHandler — TapHandler impl, DDC/demod pipeline
└── demod.rs             FM/AM demodulator, de-emphasis, decimation, WAV header
references/              reference implementation of the Zako3 SDK tap pattern
```
