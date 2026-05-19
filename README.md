# SDRTap

A live SDR (Software Defined Radio) tap for the [Zako3](https://zako.ac) Tap Hub. Connects to an `rtl_tcp` server, demodulates FM/AM radio in real-time, and streams audio to listeners through the hub.

## How it works

```
rtl_tcp server
     │  raw I/Q bytes (u8, 240 kHz)
     ▼
RtlTcpClient
     │  convert + demodulate (FM discriminator / AM envelope)
     ▼
de-emphasis → decimate 240k→48k → PCM i16
     │  streaming WAV via tokio duplex pipe
     ▼
decode_and_stream  (zako3-tap-sdk, ffmpeg WAV→OGG/Opus)
     │
     ▼
Tap Hub → listeners
```

Audio sources are specified as `MODE:FREQ_MHZ`, e.g. `FM:101.1` or `AM:1.080`. Live audio is never cached.

## Prerequisites

- An RTL-SDR compatible device with [`rtl_tcp`](https://osmocom.org/projects/rtl-sdr/wiki) running
- Access to a Zako3 Tap Hub (self-hosted or `api.zako.ac`)
- Rust toolchain

## Configuration

All configuration is via environment variables (a `.env` file is supported).

| Variable | Required | Default | Description |
|---|---|---|---|
| `SDR_TAP_ID` | yes | — | Unique tap identifier |
| `SDR_API_TOKEN` | yes | — | Authentication token for the hub |
| `TAPHUB_ENDPOINT` | no | `api.zako.ac` | Tap Hub address |
| `TAPHUB_SERVER_NAME` | no | — | TLS SNI override |
| `TAP_HEALTHCHECK_PORT` | no | — | HTTP health check port |
| `RTLTCP_HOST` | no | `localhost` | rtl_tcp server host |
| `RTLTCP_PORT` | no | `1234` | rtl_tcp server port |

## Running

Start `rtl_tcp` with your SDR device:

```sh
rtl_tcp -a 0.0.0.0 -p 1234
```

Then run the tap:

```sh
SDR_TAP_ID=my-sdr-tap SDR_API_TOKEN=secret cargo run --release
```

Once registered, request a source like `FM:101.1` through the Tap Hub to start streaming.

## Project structure

```
src/
├── main.rs      entry point, env config, tap builder
├── sdr.rs       SdrTapHandler — TapHandler implementation
├── rtltcp.rs    async rtl_tcp client (TCP, binary protocol)
└── demod.rs     FM/AM demodulator, decimator, WAV header writer
references/      Google TTS tap — reference implementation of the SDK pattern
```

## Supported modes

| Source format | Mode | Notes |
|---|---|---|
| `FM:101.1` | Wideband FM | 75 µs de-emphasis (Americas/Japan) |
| `WFM:101.1` | Wideband FM | alias for FM |
| `AM:1.080` | AM envelope | DC-coupled with slow tracking |
