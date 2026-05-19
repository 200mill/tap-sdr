use tokio::io::AsyncWriteExt;
use zako3_tap_sdk::{
    AttachedMetadata, AudioCachePolicy, AudioCacheType, AudioMetadata,
    AudioMetadataSuccessMessage, AudioRequestSuccessMessage, AudioSource, AudioStreamSender,
    TapError, TapHandler, encode::decode_and_stream,
};

use crate::demod;
use crate::rtltcp::RtlTcpClient;

const SAMPLE_RATE: u32 = 240_000;
const AUDIO_RATE: u32 = 48_000;
const DECIMATE: usize = (SAMPLE_RATE / AUDIO_RATE) as usize; // 5
const IQ_CHUNK_BYTES: usize = SAMPLE_RATE as usize / 10 * 2; // 100 ms of I/Q bytes
const PIPE_CAPACITY: usize = 512 * 1024;

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
    pub host: String,
    pub port: u16,
}

#[async_trait::async_trait]
impl TapHandler for SdrTapHandler {
    async fn handle_audio_metadata_request(
        &self,
        source: AudioSource,
    ) -> Result<AudioMetadataSuccessMessage, TapError> {
        Ok(AudioMetadataSuccessMessage {
            metadatas: vec![AudioMetadata::Title(title_for(&source))],
            cache: AudioCachePolicy {
                cache_type: AudioCacheType::ARHash,
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
            .ok_or_else(|| TapError::Fatal(format!("invalid source: {}", source.as_str())))?;

        tracing::info!(source = source.as_str(), freq_hz, ?mode, "starting SDR stream");

        let host = self.host.clone();
        let port = self.port;

        // Async pipe: SDR decoder writes WAV, decode_and_stream reads it
        let (mut writer, reader) = tokio::io::duplex(PIPE_CAPACITY);

        tokio::spawn(async move {
            if let Err(e) = run_sdr(host, port, freq_hz, mode, &mut writer).await {
                tracing::error!("SDR task ended: {e}");
            }
        });

        decode_and_stream(reader, stream)
            .await
            .map_err(|e| TapError::Retriable(e.to_string()))?;

        Ok(AudioRequestSuccessMessage {
            // Live audio — no caching
            cache: AudioCachePolicy {
                cache_type: AudioCacheType::ARHash,
                ttl_seconds: Some(0),
            },
            duration_secs: None,
            metadatas: AttachedMetadata::UseCached,
        })
    }
}

async fn run_sdr(
    host: String,
    port: u16,
    freq_hz: u32,
    mode: Mode,
    writer: &mut (impl AsyncWriteExt + Unpin),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut client = RtlTcpClient::connect(&host, port).await?;
    client.set_sample_rate(SAMPLE_RATE).await?;
    client.set_frequency(freq_hz).await?;
    client.set_agc_mode(true).await?;

    // Write streaming WAV header (data size = 0xFFFF_FFFF tells ffmpeg to read until EOF)
    let header = demod::streaming_wav_header(AUDIO_RATE, 1);
    writer.write_all(&header).await?;

    let mut raw = vec![0u8; IQ_CHUNK_BYTES];
    let mut prev_iq = (1.0f32, 0.0f32);
    let mut fm_deemph = 0.0f32;
    let mut am_dc = 0.0f32;

    loop {
        client.read_samples(&mut raw).await?;

        let iq = demod::convert_iq(&raw);

        let audio = match mode {
            Mode::Fm => {
                let demodulated = demod::demodulate_fm(&iq, &mut prev_iq);
                let deemphasized = demod::deemphasis(&demodulated, SAMPLE_RATE as f32, &mut fm_deemph);
                demod::decimate(&deemphasized, DECIMATE)
            }
            Mode::Am => {
                let demodulated = demod::demodulate_am(&iq, &mut am_dc);
                demod::decimate(&demodulated, DECIMATE)
            }
        };

        let pcm = demod::pcm_to_bytes(&audio);
        writer.write_all(&pcm).await?;
    }
}
