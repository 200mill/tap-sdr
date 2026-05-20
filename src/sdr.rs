use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use zako3_tap_sdk::{
    AttachedMetadata, AudioCachePolicy, AudioCacheType, AudioMetadata, AudioMetadataSuccessMessage,
    AudioRequestSuccessMessage, AudioSource, AudioStreamSender, TapError, TapHandler,
};

use crate::demod;
use crate::shared_sdr::{SharedSdr, WIDE_SAMPLE_RATE};

const DDC_DECIMATE: usize = 10;
const NARROW_RATE: u32 = WIDE_SAMPLE_RATE / DDC_DECIMATE as u32; // 240 000 Hz
const AUDIO_DECIMATE: usize = 5;
const AUDIO_RATE: u32 = NARROW_RATE / AUDIO_DECIMATE as u32; // 48 000 Hz
const PIPE_CAPACITY: usize = 512 * 1024;
const AUDIO_GAIN: f32 = 0.1; // -20 dB

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Fm,
    Am,
}

fn parse_source(source: &AudioSource) -> Option<(Mode, u32)> {
    let s = source.as_str();
    let (mode_str, freq_str) = s.split_once(':')?;
    let freq_mhz: f64 = freq_str.parse().ok()?;
    let freq_hz = (freq_mhz * 1_000_000.0).round() as u32;
    let mode = match mode_str.to_ascii_uppercase().as_str() {
        "FM" | "WFM" => Mode::Fm,
        "AM" => Mode::Am,
        _ => return None,
    };
    Some((mode, freq_hz))
}

fn title_for(source: &AudioSource) -> String {
    if let Some((mode, freq_hz)) = parse_source(source) {
        let freq_mhz = freq_hz as f64 / 1_000_000.0;
        format!("{:?} {:.3} MHz", mode, freq_mhz)
    } else {
        source.as_str().to_string()
    }
}

pub struct SdrTapHandler {
    pub sdr: Arc<SharedSdr>,
}

#[async_trait::async_trait]
impl TapHandler for SdrTapHandler {
    async fn handle_audio_metadata_request(
        &self,
        source: AudioSource,
    ) -> Result<AudioMetadataSuccessMessage, TapError> {
        tracing::debug!(source = source.as_str(), "metadata request");
        Ok(AudioMetadataSuccessMessage {
            metadatas: vec![AudioMetadata::Title(title_for(&source))],
            cache: AudioCachePolicy {
                cache_type: AudioCacheType::None,
                ttl_seconds: Some(0),
            },
        })
    }

    async fn handle_audio_request(
        &self,
        source: AudioSource,
        stream: AudioStreamSender,
    ) -> Result<AudioRequestSuccessMessage, TapError> {
        let (mode, freq_hz) = parse_source(&source)
            .ok_or_else(|| TapError::Permanent(format!("invalid source: {}", source.as_str())))?;

        let center_hz = self.sdr.center_hz;
        let half_bw = WIDE_SAMPLE_RATE as i64 / 2 - 150_000;
        let offset = freq_hz as i64 - center_hz as i64;
        if offset.abs() > half_bw {
            return Err(TapError::Permanent(format!(
                "{:.3} MHz is outside the tuned bandwidth (center {:.3} MHz ± {:.3} MHz)",
                freq_hz as f64 / 1e6,
                center_hz as f64 / 1e6,
                half_bw as f64 / 1e6,
            )));
        }

        tracing::info!(
            source = source.as_str(),
            freq_hz,
            ?mode,
            center_hz,
            offset,
            "starting SDR stream"
        );

        let rx = self.sdr.subscribe();
        let (mut writer, reader) = tokio::io::duplex(PIPE_CAPACITY);

        tokio::spawn(async move {
            match run_ddc_demod(center_hz, freq_hz, mode, rx, &mut writer).await {
                Ok(()) => tracing::info!(freq_hz, ?mode, "SDR stream ended"),
                Err(e) => tracing::error!(freq_hz, ?mode, "SDR stream error: {e}"),
            }
        });

        stream.unreliable_only();

        tokio::spawn(async move {
            match stream_and_encode(reader, stream).await {
                Ok(frames) => tracing::info!(frames, "stream encoder finished"),
                Err(e) => tracing::error!("stream encoder error: {e}"),
            }
        });

        Ok(AudioRequestSuccessMessage {
            cache: AudioCachePolicy {
                cache_type: AudioCacheType::None,
                ttl_seconds: Some(0),
            },
            duration_secs: None,
            metadatas: AttachedMetadata::UseCached,
        })
    }
}

async fn stream_and_encode(
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    stream: AudioStreamSender,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    use std::process::Stdio;
    use tokio_stream::StreamExt as _;

    let mut ffmpeg = tokio::process::Command::new("ffmpeg")
        .args([
            "-v",
            "quiet",
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
            "20000",
            "-flush_packets",
            "1",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let mut ffmpeg_in = ffmpeg.stdin.take().unwrap();
    let ffmpeg_out = ffmpeg.stdout.take().unwrap();

    let mut reader = reader;
    tokio::spawn(async move {
        tokio::io::copy(&mut reader, &mut ffmpeg_in).await.ok();
    });

    let mut ogg_reader = ogg::reading::async_api::PacketReader::new(ffmpeg_out);
    let mut frame_index = 0u64;

    while let Some(result) = ogg_reader.next().await {
        match result {
            Ok(packet) => {
                if packet.data.starts_with(b"OpusHead") || packet.data.starts_with(b"OpusTags") {
                    continue;
                }
                let data = bytes::Bytes::copy_from_slice(&packet.data);
                if !stream.send_opus_frame(frame_index, data).await {
                    tracing::debug!(frame_index, "client disconnected, stopping encoder");
                    break;
                }
                frame_index += 1;
            }
            Err(e) => {
                tracing::warn!("ogg packet read error: {e}");
                break;
            }
        }
    }

    Ok(frame_index)
}

async fn run_ddc_demod(
    center_hz: u32,
    freq_hz: u32,
    mode: Mode,
    mut rx: broadcast::Receiver<Arc<Vec<f32>>>,
    writer: &mut (impl AsyncWriteExt + Unpin),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // NCO: rotates by exp(-j·2π·offset·n/Fs) to shift requested station to DC
    let offset_hz = freq_hz as i64 - center_hz as i64;
    let phase_step = -2.0 * std::f64::consts::PI * offset_hz as f64 / WIDE_SAMPLE_RATE as f64;
    let step_cos = phase_step.cos() as f32;
    let step_sin = phase_step.sin() as f32;
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
        let raw = match rx.recv().await {
            Ok(chunk) => chunk,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("listener lagged by {n} I/Q chunks");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::debug!("broadcast channel closed, ending DDC/demod loop");
                break;
            }
        };

        let mut narrow = Vec::with_capacity(raw.len() / 2 / DDC_DECIMATE);

        for c in raw.chunks_exact(2) {
            let si = c[0];
            let sq = c[1];

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
