//! FutureSDR per-listener DSP pipeline (the `--dsp-backend futuresdr` path).
//!
//! The flowgraph is pure compute + channel I/O so it runs safely on FutureSDR's smol scheduler:
//!
//! ```text
//! ChannelSource<Complex32> ─► Ddc ─► ┌ FM: FmDemod ─► Deemphasis ─► Decimate ┐ ─► ChannelSink<f32>
//!                                    └ AM: AmEnvelope ─────────────► Decimate ┘
//! ```
//!
//! Tokio owns the reactor-bound edges: a *feeder* task converts broadcast I/Q bytes to `Complex32`
//! and pushes them into the source channel; a *drainer* task pulls audio out of the sink channel,
//! applies gain, packs PCM, and writes into the duplex pipe feeding ffmpeg (`stream_and_encode`).

pub mod am_envelope;
pub mod ddc;
pub mod decimate;
pub mod deemphasis;
pub mod fm_demod;

use std::sync::Arc;

use futuresdr::blocks::{ChannelSink, ChannelSource};
use futuresdr::prelude::*;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;

use self::am_envelope::AmEnvelope;
use self::ddc::Ddc;
use self::decimate::Decimate;
use self::deemphasis::Deemphasis;
use self::fm_demod::FmDemod;
use super::demod;
use super::dsp::{AUDIO_DECIMATE, AUDIO_GAIN, AUDIO_RATE, NARROW_RATE};
use super::handler::Mode;
use super::shared_sdr::SharedSdr;

/// Channel depth between the tokio bridge tasks and the flowgraph. Audio is ~48 kHz mono so this is
/// generous; the I/Q side is bounded by broadcast backpressure.
const CHANNEL_CAP: usize = 256;

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// The flowgraph plus its tokio-side bridge endpoints: I/Q input sender and audio output receiver.
type BuiltFlowgraph = (
    Flowgraph,
    mpsc::Sender<Box<[Complex32]>>,
    mpsc::Receiver<Box<[f32]>>,
);

/// Build the per-listener flowgraph and return the bridge channel endpoints.
fn build_flowgraph(
    center_hz: u32,
    freq_hz: u32,
    mode: Mode,
    center_rx: tokio::sync::watch::Receiver<u32>,
) -> Result<BuiltFlowgraph> {
    let mut fg = Flowgraph::new();
    let (iq_tx, iq_rx) = mpsc::channel::<Box<[Complex32]>>(CHANNEL_CAP);
    let (audio_tx, audio_rx) = mpsc::channel::<Box<[f32]>>(CHANNEL_CAP);

    let src = ChannelSource::<Complex32>::new(iq_rx);
    let ddc = Ddc::new(center_hz, freq_hz, center_rx);
    let snk = ChannelSink::<f32>::new(audio_tx);
    let dec = Decimate::new(AUDIO_DECIMATE);

    match mode {
        Mode::Fm => {
            let fm = FmDemod::new();
            let deemph = Deemphasis::new(NARROW_RATE as f32);
            connect!(fg, src > ddc > fm > deemph > dec > snk);
        }
        Mode::Am => {
            let am = AmEnvelope::new();
            connect!(fg, src > ddc > am > dec > snk);
        }
    }

    Ok((fg, iq_tx, audio_rx))
}

/// Start a FutureSDR per-listener pipeline on the shared `runtime`, wiring it to `writer` (the
/// duplex pipe feeding ffmpeg). Spawns the feeder/drainer bridge tasks; the running flowgraph is
/// owned by the drainer and torn down when the stream ends.
pub async fn spawn_listener<W>(
    runtime: &Runtime,
    sdr: Arc<SharedSdr>,
    center_hz: u32,
    freq_hz: u32,
    mode: Mode,
    writer: W,
) -> Result<(), DynError>
where
    W: AsyncWriteExt + Unpin + Send + 'static,
{
    let center_rx = sdr.actual_center_rx();
    let (fg, iq_tx, audio_rx) = build_flowgraph(center_hz, freq_hz, mode, center_rx)
        .map_err(|e| -> DynError { format!("failed to build flowgraph: {e:?}").into() })?;

    let running = runtime
        .start_async(fg)
        .await
        .map_err(|e| -> DynError { format!("failed to start flowgraph: {e:?}").into() })?;

    // The sink's channel sender lives inside the flowgraph and is only dropped when the finished
    // flowgraph is dropped. A supervisor owns `wait_async` so that when the graph terminates (input
    // EOS from the feeder, or a `stop()` from the drainer) the returned flowgraph is dropped, closing
    // the audio channel and letting the drainer observe end-of-stream.
    let handle = running.handle();
    tokio::spawn(async move {
        match running.wait_async().await {
            Ok(_fg) => tracing::debug!("FutureSDR flowgraph terminated"),
            Err(e) => tracing::warn!("FutureSDR flowgraph error: {e:?}"),
        }
    });

    let iq_rx = sdr.subscribe();
    tokio::spawn(feeder(iq_rx, iq_tx));
    tokio::spawn(drainer(audio_rx, writer, handle));

    Ok(())
}

/// Convert raw rtl_tcp u8 I/Q bytes into normalized `Complex32` and push into the source channel.
async fn feeder(mut rx: broadcast::Receiver<Arc<Vec<u8>>>, iq_tx: mpsc::Sender<Box<[Complex32]>>) {
    loop {
        match rx.recv().await {
            Ok(chunk) => {
                let mut samples = Vec::with_capacity(chunk.len() / 2);
                for c in chunk.chunks_exact(2) {
                    let i = (c[0] as f32 - 127.5) / 127.5;
                    let q = (c[1] as f32 - 127.5) / 127.5;
                    samples.push(Complex32::new(i, q));
                }
                if iq_tx.send(samples.into_boxed_slice()).await.is_err() {
                    tracing::debug!("FutureSDR source channel closed, ending feeder");
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("listener lagged by {n} I/Q chunks");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::debug!("broadcast channel closed, ending feeder");
                break;
            }
        }
    }
}

/// Pull demodulated audio out of the sink channel, apply gain, pack PCM, and write a streaming WAV
/// stream into `writer`. On writer failure (hub disconnect) it stops the flowgraph via `handle`.
async fn drainer<W>(audio_rx: mpsc::Receiver<Box<[f32]>>, mut writer: W, handle: FlowgraphHandle)
where
    W: AsyncWriteExt + Unpin + Send + 'static,
{
    let header = demod::streaming_wav_header(AUDIO_RATE, 1);
    if writer.write_all(&header).await.is_err() {
        tracing::debug!("writer closed before WAV header, stopping flowgraph");
        let _ = handle.stop().await;
        return;
    }

    loop {
        match audio_rx.recv().await {
            Some(buf) => {
                let gained: Vec<f32> = buf.iter().map(|s| s * AUDIO_GAIN).collect();
                let pcm = demod::pcm_to_bytes(&gained);
                if writer.write_all(&pcm).await.is_err() {
                    tracing::debug!("writer closed, stopping flowgraph");
                    let _ = handle.stop().await;
                    break;
                }
            }
            None => {
                tracing::debug!("audio channel closed, ending drainer");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audiotap::shared_sdr::WIDE_SAMPLE_RATE;

    /// Drive the FM flowgraph with a synthetic complex tone offset from centre and confirm
    /// demodulated audio flows through every block (DDC → FmDemod → Deemphasis → Decimate) and is
    /// finite. This exercises the custom blocks, `connect!`, the shared smol runtime, and the
    /// kanal channel bridges end-to-end without hardware or the hub.
    #[tokio::test]
    async fn fm_flowgraph_produces_finite_audio() {
        unsafe {
            std::env::set_var("FUTURESDR_CTRLPORT_ENABLE", "false");
        }
        let runtime = Runtime::new();

        let center_hz = 98_000_000u32;
        let freq_hz = 98_100_000u32; // +100 kHz offset
        let (_tx, center_rx) = tokio::sync::watch::channel(center_hz);

        let (fg, iq_tx, audio_rx) =
            build_flowgraph(center_hz, freq_hz, Mode::Fm, center_rx).expect("build flowgraph");
        let running = runtime.start_async(fg).await.expect("start flowgraph");

        // Tone at centre + offset + 1 kHz so the residual after the DDC is a constant non-zero
        // frequency, yielding a constant non-zero FM-demodulated level.
        let n = WIDE_SAMPLE_RATE as usize / 5; // 0.2 s
        let tone_hz = (freq_hz - center_hz + 1_000) as f64;
        let mut samples = Vec::with_capacity(n);
        for k in 0..n {
            let phase = 2.0 * std::f64::consts::PI * tone_hz * k as f64 / WIDE_SAMPLE_RATE as f64;
            samples.push(Complex32::new(phase.cos() as f32, phase.sin() as f32));
        }
        iq_tx
            .send(samples.into_boxed_slice())
            .await
            .expect("send iq");
        drop(iq_tx); // signal end-of-stream so the graph terminates

        // Supervisor: when the graph finishes, drop the returned flowgraph to close the sink sender.
        tokio::spawn(async move {
            let _fg = running.wait_async().await;
        });

        // Collect audio until the graph finishes (with a safety timeout).
        let collected = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut out = Vec::new();
            while let Some(buf) = audio_rx.recv().await {
                out.extend_from_slice(&buf);
            }
            out
        })
        .await
        .expect("flowgraph drained before timeout");

        // 2.4 MHz → /10 (DDC) → /5 (audio) = 48 kHz; 0.2 s ≈ 9600 samples (allow filter transient).
        assert!(
            collected.len() > 5_000,
            "expected ~9600 audio samples, got {}",
            collected.len()
        );
        assert!(
            collected.iter().all(|s| s.is_finite()),
            "all audio samples must be finite"
        );
        let peak = collected.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(
            peak > 1e-4,
            "expected non-zero demodulated audio, peak={peak}"
        );
    }
}
