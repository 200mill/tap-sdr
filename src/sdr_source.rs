use async_trait::async_trait;

pub struct SdrConfig {
    pub sample_rate: u32,
    pub center_hz: u32,
    pub gain_db: f32,
}

#[async_trait]
pub trait SdrSource: Send + 'static {
    async fn configure(
        &mut self,
        cfg: &SdrConfig,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Fill `buf` with `n_samples` interleaved f32 I/Q pairs, normalized to [-1.0, 1.0].
    async fn read_chunk(
        &mut self,
        buf: &mut Vec<f32>,
        n_samples: usize,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
