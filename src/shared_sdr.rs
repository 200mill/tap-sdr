use std::sync::Arc;
use tokio::sync::broadcast;

use crate::sdr_source::{SdrConfig, SdrSource};

pub const WIDE_SAMPLE_RATE: u32 = 2_400_000;
const CHUNK_SAMPLES: usize = WIDE_SAMPLE_RATE as usize / 10; // 100 ms of I/Q samples

pub struct SharedSdr {
    pub center_hz: u32,
    pub sample_rate: u32,
    gain_db: f32,
    tx: broadcast::Sender<Arc<Vec<f32>>>,
}

impl SharedSdr {
    pub fn new(center_hz: u32, gain_db: f32) -> Arc<Self> {
        let (tx, _) = broadcast::channel(16);
        Arc::new(Self {
            center_hz,
            sample_rate: WIDE_SAMPLE_RATE,
            gain_db,
            tx,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Vec<f32>>> {
        self.tx.subscribe()
    }

    pub async fn run(self: Arc<Self>, mut source: Box<dyn SdrSource>) {
        loop {
            tracing::info!(center_hz = self.center_hz, "connecting to SDR");
            match self.run_inner(&mut *source).await {
                Ok(()) => tracing::warn!("SDR connection closed"),
                Err(e) => tracing::error!("SDR error: {e}"),
            }
            tracing::info!("reconnecting to SDR in 2s");
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }

    async fn run_inner(
        &self,
        source: &mut dyn SdrSource,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let cfg = SdrConfig {
            sample_rate: self.sample_rate,
            center_hz: self.center_hz,
            gain_db: self.gain_db,
        };
        source.configure(&cfg).await?;
        tracing::info!(
            center_hz = self.center_hz,
            sample_rate = self.sample_rate,
            gain_db = self.gain_db,
            "SDR configured and streaming"
        );

        let mut buf = Vec::with_capacity(CHUNK_SAMPLES * 2);
        let mut chunks_sent = 0u64;
        loop {
            source.read_chunk(&mut buf, CHUNK_SAMPLES).await?;
            let receivers = self.tx.receiver_count();
            tracing::trace!(chunks_sent, receivers, "I/Q chunk broadcast");
            let _ = self.tx.send(Arc::new(buf.clone()));
            chunks_sent += 1;
        }
    }
}
