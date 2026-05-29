//! Digital Down-Converter block: NCO frequency shift + boxcar decimation (2.4 MHz → 240 kHz).
//!
//! Mirrors the NCO math in `dsp::run_ddc_demod` exactly so the FutureSDR backend produces the same
//! audio as the legacy path. Holds a `watch::Receiver<u32>` for the actual hardware centre frequency
//! and recomputes the phase step when a retune is observed — this keeps retune coherence identical to
//! the legacy loop without needing flowgraph message ports.

use futuresdr::runtime::dev::prelude::*;
use tokio::sync::watch;

use crate::modules::audiotap::dsp::DDC_DECIMATE;
use crate::modules::audiotap::shared_sdr::WIDE_SAMPLE_RATE;

#[derive(Block)]
pub struct Ddc<I = DefaultCpuReader<Complex32>, O = DefaultCpuWriter<Complex32>>
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    #[input]
    input: I,
    #[output]
    output: O,
    freq_hz: u32,
    center_rx: watch::Receiver<u32>,
    step_cos: f32,
    step_sin: f32,
    osc_i: f32,
    osc_q: f32,
    osc_ticks: u32,
    acc_i: f32,
    acc_q: f32,
    acc_count: usize,
}

impl Ddc<DefaultCpuReader<Complex32>, DefaultCpuWriter<Complex32>> {
    pub fn new(center_hz: u32, freq_hz: u32, center_rx: watch::Receiver<u32>) -> Self {
        let (step_cos, step_sin) = phase_step(center_hz, freq_hz);
        Self {
            input: DefaultCpuReader::default(),
            output: DefaultCpuWriter::default(),
            freq_hz,
            center_rx,
            step_cos,
            step_sin,
            osc_i: 1.0,
            osc_q: 0.0,
            osc_ticks: 0,
            acc_i: 0.0,
            acc_q: 0.0,
            acc_count: 0,
        }
    }
}

/// NCO rotates by exp(-j·2π·offset·n/Fs) to shift the requested station to DC.
fn phase_step(center_hz: u32, freq_hz: u32) -> (f32, f32) {
    let offset_hz = freq_hz as i64 - center_hz as i64;
    let phase_step = -2.0 * std::f64::consts::PI * offset_hz as f64 / WIDE_SAMPLE_RATE as f64;
    (phase_step.cos() as f32, phase_step.sin() as f32)
}

impl<I, O> Kernel for Ddc<I, O>
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _mo: &mut MessageOutputs,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        if self.center_rx.has_changed().unwrap_or(false) {
            let new_center = *self.center_rx.borrow_and_update();
            let (c, s) = phase_step(new_center, self.freq_hz);
            self.step_cos = c;
            self.step_sin = s;
            self.osc_i = 1.0;
            self.osc_q = 0.0;
            self.osc_ticks = 0;
            trace!(
                new_center,
                freq_hz = self.freq_hz,
                "DDC NCO recomputed after retune"
            );
        }

        let inp = self.input.slice();
        let out = self.output.slice();
        let in_len = inp.len();
        let out_len = out.len();

        let mut consumed = 0;
        let mut produced = 0;

        for &c in inp.iter() {
            // Stop before consuming a sample that would complete a group we cannot emit.
            if self.acc_count == DDC_DECIMATE - 1 && produced >= out_len {
                break;
            }

            // Frequency shift by the NCO phasor.
            let shifted_i = c.re * self.osc_i - c.im * self.osc_q;
            let shifted_q = c.re * self.osc_q + c.im * self.osc_i;

            // Advance NCO.
            let new_i = self.osc_i * self.step_cos - self.osc_q * self.step_sin;
            let new_q = self.osc_i * self.step_sin + self.osc_q * self.step_cos;
            self.osc_i = new_i;
            self.osc_q = new_q;

            // Periodic renormalization to prevent magnitude drift.
            self.osc_ticks += 1;
            if self.osc_ticks == 65536 {
                let mag = (self.osc_i * self.osc_i + self.osc_q * self.osc_q).sqrt();
                self.osc_i /= mag;
                self.osc_q /= mag;
                self.osc_ticks = 0;
            }

            // Boxcar accumulate and dump.
            self.acc_i += shifted_i;
            self.acc_q += shifted_q;
            self.acc_count += 1;
            consumed += 1;
            if self.acc_count == DDC_DECIMATE {
                out[produced] = Complex32::new(
                    self.acc_i / DDC_DECIMATE as f32,
                    self.acc_q / DDC_DECIMATE as f32,
                );
                produced += 1;
                self.acc_i = 0.0;
                self.acc_q = 0.0;
                self.acc_count = 0;
            }
        }

        self.input.consume(consumed);
        self.output.produce(produced);

        if self.input.finished() && consumed == in_len {
            io.finished = true;
        }

        Ok(())
    }
}
