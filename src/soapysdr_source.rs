#![cfg(feature = "soapy")]

use async_trait::async_trait;
use num_complex::Complex32;
use std::sync::{Arc, Mutex};

use crate::sdr_source::{SdrConfig, SdrSource};

struct SoapyState {
    stream: soapysdr::RxStream<Complex32>,
}

pub struct SoapySdrSource {
    args: String,
    state: Option<Arc<Mutex<SoapyState>>>,
}

impl SoapySdrSource {
    pub fn new(args: String) -> Self {
        Self { args, state: None }
    }
}

#[async_trait]
impl SdrSource for SoapySdrSource {
    async fn configure(
        &mut self,
        cfg: &SdrConfig,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let args = self.args.clone();
        let sample_rate = cfg.sample_rate as f64;
        let center_hz = cfg.center_hz as f64;
        let gain_db = cfg.gain_db as f64;

        let state = tokio::task::spawn_blocking(
            move || -> Result<SoapyState, Box<dyn std::error::Error + Send + Sync>> {
                let dev = soapysdr::Device::new(args.as_str())?;
                dev.set_sample_rate(soapysdr::Direction::Rx, 0, sample_rate)?;
                dev.set_frequency(soapysdr::Direction::Rx, 0, center_hz, soapysdr::Args::new())?;
                dev.set_gain(soapysdr::Direction::Rx, 0, gain_db)?;
                let mut stream = dev.rx_stream::<Complex32>(&[0])?;
                stream.activate(None)?;
                tracing::info!(args, sample_rate, center_hz, gain_db, "SoapySDR configured");
                Ok(SoapyState { stream })
            },
        )
        .await??;

        self.state = Some(Arc::new(Mutex::new(state)));
        Ok(())
    }

    async fn read_chunk(
        &mut self,
        buf: &mut Vec<f32>,
        n_samples: usize,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state = self.state.as_ref().ok_or("soapy not configured")?.clone();

        let samples = tokio::task::spawn_blocking(
            move || -> Result<Vec<Complex32>, Box<dyn std::error::Error + Send + Sync>> {
                let mut guard = state.lock().unwrap();
                let mut local = vec![Complex32::new(0.0, 0.0); n_samples];
                // 1 second timeout in microseconds
                let n = guard.stream.read(&[&mut local], 1_000_000)?;
                local.truncate(n);
                Ok(local)
            },
        )
        .await??;

        buf.clear();
        buf.reserve(samples.len() * 2);
        for c in samples {
            buf.push(c.re);
            buf.push(c.im);
        }
        Ok(())
    }
}
