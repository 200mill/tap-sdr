//! FM broadcast de-emphasis block (75 µs one-pole IIR, Americas/Japan).
//!
//! Same formula as `demod::deemphasis`: y[n] = α·x[n] + (1−α)·y[n−1] with α = 1/(1 + Fs·τ).
//! Stateful across work calls via `prev`.

use futuresdr::runtime::dev::prelude::*;

#[derive(Block)]
pub struct Deemphasis<I = DefaultCpuReader<f32>, O = DefaultCpuWriter<f32>>
where
    I: CpuBufferReader<Item = f32>,
    O: CpuBufferWriter<Item = f32>,
{
    #[input]
    input: I,
    #[output]
    output: O,
    alpha: f32,
    prev: f32,
}

impl Deemphasis<DefaultCpuReader<f32>, DefaultCpuWriter<f32>> {
    pub fn new(sample_rate: f32) -> Self {
        let tau = 75e-6_f32;
        let alpha = 1.0 / (1.0 + sample_rate * tau);
        Self {
            input: DefaultCpuReader::default(),
            output: DefaultCpuWriter::default(),
            alpha,
            prev: 0.0,
        }
    }
}

impl<I, O> Kernel for Deemphasis<I, O>
where
    I: CpuBufferReader<Item = f32>,
    O: CpuBufferWriter<Item = f32>,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _mo: &mut MessageOutputs,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let inp = self.input.slice();
        let out = self.output.slice();
        let in_len = inp.len();
        let m = std::cmp::min(in_len, out.len());

        for k in 0..m {
            self.prev = self.alpha * inp[k] + (1.0 - self.alpha) * self.prev;
            out[k] = self.prev;
        }

        self.input.consume(m);
        self.output.produce(m);

        if self.input.finished() && m == in_len {
            io.finished = true;
        }

        Ok(())
    }
}
