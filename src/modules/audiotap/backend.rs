/// Selects which per-listener DSP pipeline `handle_audio_request` runs.
///
/// Both pipelines are always compiled in; the choice is made at startup via `--dsp-backend`
/// (env `SDR_DSP_BACKEND`). Hardware acquisition (rtl_tcp → broadcast) is identical for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DspBackend {
    /// Hand-rolled tokio DSP (`dsp::run_ddc_demod`).
    #[default]
    Legacy,
    /// FutureSDR flowgraph (`BroadcastSource → Ddc → demod → … → EncodeSink`).
    FutureSdr,
}

impl DspBackend {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "legacy" => Some(Self::Legacy),
            "futuresdr" | "future-sdr" | "fsdr" => Some(Self::FutureSdr),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::FutureSdr => "futuresdr",
        }
    }
}
