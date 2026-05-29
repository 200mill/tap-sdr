use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, watch};
use zako3_tap_sdk::AudioStreamSender;

use super::demod;
use super::handler::Mode;
use super::shared_sdr::WIDE_SAMPLE_RATE;

pub const DDC_DECIMATE: usize = 10;
pub const NARROW_RATE: u32 = WIDE_SAMPLE_RATE / DDC_DECIMATE as u32; // 240 000 Hz
pub const AUDIO_DECIMATE: usize = 5;
pub const AUDIO_RATE: u32 = NARROW_RATE / AUDIO_DECIMATE as u32; // 48 000 Hz
pub const PIPE_CAPACITY: usize = 512 * 1024;
pub const AUDIO_GAIN: f32 = 0.1; // -20 dB

pub async fn stream_and_encode(
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    stream: AudioStreamSender,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    use std::process::Stdio;
    use std::time::Instant;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio_stream::StreamExt as _;

    let spawn_started = Instant::now();
    let mut ffmpeg = tokio::process::Command::new("ffmpeg")
        .args([
            "-loglevel",
            "warning",
            "-fflags",
            "+nobuffer",
            "-i",
            "pipe:0",
            "-vn",
            "-c:a",
            "libopus",
            "-f",
            "ogg",
            "-page_duration",
            "1",
            "-flush_packets",
            "1",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut ffmpeg_in = ffmpeg.stdin.take().unwrap();
    let ffmpeg_out = ffmpeg.stdout.take().unwrap();
    let ffmpeg_err = ffmpeg.stderr.take().unwrap();

    tokio::spawn(async move {
        let mut lines = BufReader::new(ffmpeg_err).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => tracing::warn!(target: "ffmpeg.stderr", "{line}"),
                Ok(None) => {
                    tracing::info!(target: "ffmpeg.stderr", "ffmpeg stderr closed");
                    break;
                }
                Err(e) => {
                    tracing::warn!(target: "ffmpeg.stderr", "read error: {e}");
                    break;
                }
            }
        }
    });

    let mut reader = reader;
    tokio::spawn(async move {
        match tokio::io::copy(&mut reader, &mut ffmpeg_in).await {
            Ok(n) => tracing::info!(bytes = n, "PCM→ffmpeg copy ended"),
            Err(e) => tracing::warn!("PCM→ffmpeg copy error: {e:?}"),
        }
    });

    let mut ogg_reader = ogg::reading::async_api::PacketReader::new(ffmpeg_out);
    let mut frame_index = 0u64;
    let mut first_packet = true;

    let exit_reason: &'static str = loop {
        match ogg_reader.next().await {
            Some(Ok(packet)) => {
                if packet.data.starts_with(b"OpusHead") || packet.data.starts_with(b"OpusTags") {
                    continue;
                }
                if first_packet {
                    tracing::info!(
                        elapsed_ms = spawn_started.elapsed().as_millis() as u64,
                        bytes = packet.data.len(),
                        "first opus packet from ffmpeg"
                    );
                    first_packet = false;
                }
                let data = bytes::Bytes::copy_from_slice(&packet.data);
                if !stream.send_opus_frame(frame_index, data).await {
                    break "hub_disconnected";
                }
                frame_index += 1;
                if frame_index.is_multiple_of(250) {
                    tracing::debug!(
                        frame_index,
                        elapsed_ms = spawn_started.elapsed().as_millis() as u64,
                        "encode heartbeat"
                    );
                }
            }
            Some(Err(e)) => {
                tracing::warn!("ogg packet read error: {e:?}");
                break "ogg_read_error";
            }
            None => break "ffmpeg_eof",
        }
    };

    tracing::info!(
        frame_index,
        elapsed_ms = spawn_started.elapsed().as_millis() as u64,
        exit_reason,
        "encoder loop exited"
    );

    Ok(frame_index)
}

pub async fn run_ddc_demod(
    center_hz: u32,
    freq_hz: u32,
    mode: Mode,
    mut rx: broadcast::Receiver<Arc<Vec<u8>>>,
    writer: &mut (impl AsyncWriteExt + Unpin),
    mut retune_rx: watch::Receiver<u32>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // NCO: rotates by exp(-j·2π·offset·n/Fs) to shift requested station to DC
    let offset_hz = freq_hz as i64 - center_hz as i64;
    let phase_step = -2.0 * std::f64::consts::PI * offset_hz as f64 / WIDE_SAMPLE_RATE as f64;
    let mut step_cos = phase_step.cos() as f32;
    let mut step_sin = phase_step.sin() as f32;
    let mut osc_i = 1.0f32;
    let mut osc_q = 0.0f32;
    let mut osc_ticks = 0u32;

    // Boxcar accumulator for DDC decimation (2.4 MHz → 240 kHz)
    let mut acc_i = 0.0f32;
    let mut acc_q = 0.0f32;
    let mut acc_count = 0usize;

    let mut prev_iq = (1.0f32, 0.0f32);
    let mut fm_deemph = 0.0f32;
    let mut am_dc = 0.0f32;

    let header = demod::streaming_wav_header(AUDIO_RATE, 1);
    writer.write_all(&header).await?;

    loop {
        let raw: Arc<Vec<u8>> = tokio::select! {
            result = rx.recv() => match result {
                Ok(chunk) => chunk,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("listener lagged by {n} I/Q chunks");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::debug!("broadcast channel closed, ending DDC/demod loop");
                    break;
                }
            },
            _ = retune_rx.changed() => {
                let new_center = *retune_rx.borrow_and_update();
                let new_offset = freq_hz as i64 - new_center as i64;
                let ps = -2.0 * std::f64::consts::PI * new_offset as f64 / WIDE_SAMPLE_RATE as f64;
                step_cos = ps.cos() as f32;
                step_sin = ps.sin() as f32;
                osc_i = 1.0;
                osc_q = 0.0;
                osc_ticks = 0;
                tracing::info!(new_center, freq_hz, "DDC NCO recomputed after retune");
                continue;
            }
        };

        let mut narrow = Vec::with_capacity(raw.len() / 2 / DDC_DECIMATE);

        for c in raw.chunks_exact(2) {
            let si = (c[0] as f32 - 127.5) / 127.5;
            let sq = (c[1] as f32 - 127.5) / 127.5;

            // Frequency shift
            let shifted_i = si * osc_i - sq * osc_q;
            let shifted_q = si * osc_q + sq * osc_i;

            // Advance NCO
            let new_i = osc_i * step_cos - osc_q * step_sin;
            let new_q = osc_i * step_sin + osc_q * step_cos;
            osc_i = new_i;
            osc_q = new_q;

            // Periodic renormalization to prevent magnitude drift
            osc_ticks += 1;
            if osc_ticks == 65536 {
                let mag = (osc_i * osc_i + osc_q * osc_q).sqrt();
                osc_i /= mag;
                osc_q /= mag;
                osc_ticks = 0;
            }

            // Boxcar accumulate and dump
            acc_i += shifted_i;
            acc_q += shifted_q;
            acc_count += 1;
            if acc_count == DDC_DECIMATE {
                narrow.push((acc_i / DDC_DECIMATE as f32, acc_q / DDC_DECIMATE as f32));
                acc_i = 0.0;
                acc_q = 0.0;
                acc_count = 0;
            }
        }

        let audio = match mode {
            Mode::Fm => {
                let demodulated = demod::demodulate_fm(&narrow, &mut prev_iq);
                let deemphasized =
                    demod::deemphasis(&demodulated, NARROW_RATE as f32, &mut fm_deemph);
                demod::decimate(&deemphasized, AUDIO_DECIMATE)
            }
            Mode::Am => {
                let demodulated = demod::demodulate_am(&narrow, &mut am_dc);
                demod::decimate(&demodulated, AUDIO_DECIMATE)
            }
        };

        let audio: Vec<f32> = audio.iter().map(|s| s * AUDIO_GAIN).collect();
        let pcm = demod::pcm_to_bytes(&audio);
        writer.write_all(&pcm).await?;
    }

    Ok(())
}
