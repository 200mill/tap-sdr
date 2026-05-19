use std::sync::Arc;
use tokio::sync::broadcast;

use crate::rtltcp::RtlTcpClient;

pub const WIDE_SAMPLE_RATE: u32 = 2_400_000;
const CHUNK_BYTES: usize = WIDE_SAMPLE_RATE as usize / 10 * 2; // 100 ms of I/Q bytes

pub struct SharedSdr {
    pub center_hz: u32,
    pub sample_rate: u32,
    tx: broadcast::Sender<Arc<Vec<u8>>>,
}

impl SharedSdr {
    pub fn new(center_hz: u32) -> Arc<Self> {
        let (tx, _) = broadcast::channel(16);
        Arc::new(Self {
            center_hz,
            sample_rate: WIDE_SAMPLE_RATE,
            tx,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Vec<u8>>> {
        self.tx.subscribe()
    }

    pub async fn run(self: Arc<Self>, host: String, port: u16) {
        loop {
            tracing::info!(center_hz = self.center_hz, "connecting to rtl_tcp");
            match self.run_inner(&host, port).await {
                Ok(()) => tracing::warn!("rtl_tcp connection closed"),
                Err(e) => tracing::error!("rtl_tcp error: {e}"),
            }
            tracing::info!("reconnecting to rtl_tcp in 2s");
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }

    async fn run_inner(
        &self,
        host: &str,
        port: u16,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut client = RtlTcpClient::connect(host, port).await?;
        client.set_sample_rate(self.sample_rate).await?;
        client.set_frequency(self.center_hz).await?;
        client.set_gain_mode(true).await?;
        client.set_gain(150).await?; // 15.0 dB
        client.set_agc_mode(false).await?;
        tracing::info!(
            center_hz = self.center_hz,
            sample_rate = self.sample_rate,
            chunk_bytes = CHUNK_BYTES,
            gain_db = 15.0,
            "rtl_tcp configured and streaming"
        );

        let mut buf = vec![0u8; CHUNK_BYTES];
        let mut chunks_sent = 0u64;
        loop {
            client.read_samples(&mut buf).await?;
            let receivers = self.tx.receiver_count();
            tracing::trace!(chunks_sent, receivers, "I/Q chunk broadcast");
            let _ = self.tx.send(Arc::new(buf.clone()));
            chunks_sent += 1;
        }
    }
}
