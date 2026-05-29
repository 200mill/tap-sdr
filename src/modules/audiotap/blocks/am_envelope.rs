//! AM envelope detector block with slow DC removal.
//!
//! Same formula as `demod::demodulate_am`: magnitude of the I/Q sample minus a slowly-tracked DC
//! level (EMA, τ ≈ 100 ms at 240 kHz input). Stateful across work calls via `dc`.

use futuresdr::runtime::dev::prelude::*;

#[derive(Block)]
pub struct AmEnvelope<I = DefaultCpuReader<Complex32>, O = DefaultCpuWriter<f32>>
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = f32>,
{
    #[input]
    input: I,
    #[output]
    output: O,
    dc: f32,
}

impl AmEnvelope<DefaultCpuReader<Complex32>, DefaultCpuWriter<f32>> {
    pub fn new() -> Self {
        Self {
            input: DefaultCpuReader::default(),
            output: DefaultCpuWriter::default(),
            dc: 0.0,
        }
    }
}

impl Default for AmEnvelope {
    fn default() -> Self {
        Self::new()
    }
}

impl<I, O> Kernel for AmEnvelope<I, O>
where
    I: CpuBufferReader<Item = Complex32>,
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
            let mag = (inp[k].re * inp[k].re + inp[k].im * inp[k].im).sqrt();
            self.dc = self.dc * 0.99997 + mag * 0.00003;
            out[k] = mag - self.dc;
        }

        self.input.consume(m);
        self.output.produce(m);

        if self.input.finished() && m == in_len {
            io.finished = true;
        }

        Ok(())
    }
}
