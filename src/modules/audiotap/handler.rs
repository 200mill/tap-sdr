use std::sync::Arc;
use std::sync::atomic::Ordering;
use zako3_tap_sdk::{
    AttachedMetadata, AudioCachePolicy, AudioCacheType, AudioMetadata, AudioMetadataSuccessMessage,
    AudioRequestSuccessMessage, AudioSource, AudioStreamSender, TapError, TapHandler,
};

use super::dsp::{PIPE_CAPACITY, run_ddc_demod, stream_and_encode};
use super::shared_sdr::{SharedSdr, WIDE_SAMPLE_RATE};

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
        // Mark hub as connected on first interaction.
        self.sdr
            .hub_connected_handle()
            .store(1, Ordering::Relaxed);
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
        self.sdr
            .hub_connected_handle()
            .store(1, Ordering::Relaxed);

        let (mode, freq_hz) = parse_source(&source)
            .ok_or_else(|| TapError::Permanent(format!("invalid source: {}", source.as_str())))?;

        let mut center_hz = self.sdr.center_hz();
        let half_bw = WIDE_SAMPLE_RATE as i64 / 2 - 150_000;
        let offset = freq_hz as i64 - center_hz as i64;
        if offset.abs() > half_bw {
            tracing::info!(freq_hz, center_hz, "frequency out of range, retuning");
            self.sdr.retune(freq_hz).await;
            center_hz = self.sdr.center_hz();
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
        let retune_rx = self.sdr.actual_center_rx();
        let (mut writer, reader) = tokio::io::duplex(PIPE_CAPACITY);

        tokio::spawn(async move {
            match run_ddc_demod(center_hz, freq_hz, mode, rx, &mut writer, retune_rx).await {
                Ok(()) => tracing::info!(freq_hz, ?mode, "DDC/demod task ended cleanly"),
                Err(e) => tracing::error!(freq_hz, ?mode, "DDC/demod task error: {e:?}"),
            }
        });

        stream.unreliable_only();

        tokio::spawn(async move {
            match stream_and_encode(reader, stream).await {
                Ok(frames) => tracing::info!(frames, "stream encoder finished"),
                Err(e) => tracing::error!("stream encoder error: {e:?}"),
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
