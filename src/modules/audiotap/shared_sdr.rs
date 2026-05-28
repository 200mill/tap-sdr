use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, broadcast, watch};

use super::rtltcp::RtlTcpClient;

pub const WIDE_SAMPLE_RATE: u32 = 2_400_000;
pub const DEFAULT_GAIN_TENTHS_DB: u32 = 150; // 15.0 dB
const CHUNK_BYTES: usize = WIDE_SAMPLE_RATE as usize / 10 * 2; // 100 ms of I/Q bytes
const RING_CAPACITY: usize = 20; // 2 s of staged I/Q data

struct IqBuffer {
    deque: Mutex<VecDeque<Arc<Vec<u8>>>>,
    notify: Notify,
}

impl IqBuffer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            deque: Mutex::new(VecDeque::with_capacity(RING_CAPACITY)),
            notify: Notify::new(),
        })
    }

    fn push(&self, chunk: Arc<Vec<u8>>) {
        let mut dq = self.deque.lock().unwrap();
        if dq.len() >= RING_CAPACITY {
            dq.pop_front();
        }
        dq.push_back(chunk);
        drop(dq);
        self.notify.notify_one();
    }

    async fn pop(&self) -> Arc<Vec<u8>> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Enable before inspecting the queue so a concurrent push isn't missed.
            notified.as_mut().enable();
            {
                let mut dq = self.deque.lock().unwrap();
                if let Some(chunk) = dq.pop_front() {
                    return chunk;
                }
            }
            notified.await;
        }
    }
}

pub struct SharedSdr {
    desired_tx: watch::Sender<u32>,
    actual_tx: watch::Sender<u32>,
    gain_tx: watch::Sender<u32>,
    pub sample_rate: u32,
    tx: broadcast::Sender<Arc<Vec<u8>>>,
    iq_buffer: Arc<IqBuffer>,
    chunks_pushed: AtomicU64,
    retunes: AtomicU64,
    hub_connected: Arc<AtomicU64>, // 0 = no, non-zero = connected since this epoch ms
}

impl SharedSdr {
    pub fn new(center_hz: u32) -> Arc<Self> {
        let (tx, _) = broadcast::channel(16);
        let (desired_tx, _) = watch::channel(center_hz);
        let (actual_tx, _) = watch::channel(center_hz);
        let (gain_tx, _) = watch::channel(DEFAULT_GAIN_TENTHS_DB);
        Arc::new(Self {
            desired_tx,
            actual_tx,
            gain_tx,
            sample_rate: WIDE_SAMPLE_RATE,
            tx,
            iq_buffer: IqBuffer::new(),
            chunks_pushed: AtomicU64::new(0),
            retunes: AtomicU64::new(0),
            hub_connected: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn center_hz(&self) -> u32 {
        *self.actual_tx.borrow()
    }

    pub fn current_gain_tenths_db(&self) -> u32 {
        *self.gain_tx.borrow()
    }

    pub fn chunks_pushed(&self) -> u64 {
        self.chunks_pushed.load(Ordering::Relaxed)
    }

    pub fn retunes(&self) -> u64 {
        self.retunes.load(Ordering::Relaxed)
    }

    pub fn listener_count(&self) -> usize {
        self.tx.receiver_count()
    }

    pub fn hub_connected_handle(&self) -> Arc<AtomicU64> {
        self.hub_connected.clone()
    }

    pub fn hub_connected(&self) -> bool {
        self.hub_connected.load(Ordering::Relaxed) != 0
    }

    /// Returns a receiver that fires whenever the hardware center frequency is confirmed changed.
    pub fn actual_center_rx(&self) -> watch::Receiver<u32> {
        self.actual_tx.subscribe()
    }

    /// Request a retune to `new_hz` and wait (up to 5 s) for hardware confirmation.
    pub async fn retune(&self, new_hz: u32) {
        let mut rx = self.actual_tx.subscribe();
        self.desired_tx.send(new_hz).ok();
        let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
            loop {
                if *rx.borrow() == new_hz {
                    break;
                }
                if rx.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
    }

    /// Request a tuner gain change (in tenths of a dB, e.g. 150 = 15.0 dB).
    pub fn set_gain(&self, tenths_db: u32) {
        self.gain_tx.send(tenths_db).ok();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Vec<u8>>> {
        self.tx.subscribe()
    }

    pub async fn run(self: Arc<Self>, host: String, port: u16) {
        // Drain the ring buffer and fan out to broadcast receivers.
        let iq_buf = self.iq_buffer.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            loop {
                let chunk = iq_buf.pop().await;
                let _ = tx.send(chunk);
            }
        });

        loop {
            tracing::info!(center_hz = self.center_hz(), "connecting to rtl_tcp");
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
        // Use current desired center / gain so reconnects honour latest values.
        let mut desired_rx = self.desired_tx.subscribe();
        let mut gain_rx = self.gain_tx.subscribe();
        let center_hz = *desired_rx.borrow_and_update();
        let gain = *gain_rx.borrow_and_update();

        let mut client = RtlTcpClient::connect(host, port).await?;
        client.set_sample_rate(self.sample_rate).await?;
        client.set_frequency(center_hz).await?;
        client.set_gain_mode(true).await?;
        client.set_gain(gain).await?;
        client.set_agc_mode(false).await?;
        self.actual_tx.send(center_hz).ok();

        tracing::info!(
            center_hz,
            sample_rate = self.sample_rate,
            chunk_bytes = CHUNK_BYTES,
            gain_db = gain as f32 / 10.0,
            "rtl_tcp configured and streaming"
        );

        let mut buf = vec![0u8; CHUNK_BYTES];
        loop {
            client.read_samples(&mut buf).await?;
            let receivers = self.tx.receiver_count();
            let chunks_sent = self.chunks_pushed.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::trace!(chunks_sent, receivers, "I/Q chunk pushed to ring buffer");
            self.iq_buffer.push(Arc::new(buf.clone()));

            // Check for a pending retune between reads (never mid-buffer).
            if desired_rx.has_changed().unwrap_or(false) {
                let new_hz = *desired_rx.borrow_and_update();
                client.set_frequency(new_hz).await?;
                self.actual_tx.send(new_hz).ok();
                self.retunes.fetch_add(1, Ordering::Relaxed);
                tracing::info!(new_hz, "retuned center frequency");
            }

            if gain_rx.has_changed().unwrap_or(false) {
                let new_gain = *gain_rx.borrow_and_update();
                client.set_gain(new_gain).await?;
                tracing::info!(gain_db = new_gain as f32 / 10.0, "applied new tuner gain");
            }
        }
    }
}
