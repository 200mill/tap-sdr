//! FM discriminator block (cross-product / dot-product method).
//!
//! Same formula as `demod::demodulate_fm`: returns instantaneous frequency deviation (atan2 of the
//! conjugate product of consecutive samples). Stateful across work calls via `prev`.

use futuresdr::runtime::dev::prelude::*;

#[derive(Block)]
pub struct FmDemod<I = DefaultCpuReader<Complex32>, O = DefaultCpuWriter<f32>>
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = f32>,
{
    #[input]
    input: I,
    #[output]
    output: O,
    prev: Complex32,
}

impl FmDemod<DefaultCpuReader<Complex32>, DefaultCpuWriter<f32>> {
    pub fn new() -> Self {
        Self {
            input: DefaultCpuReader::default(),
            output: DefaultCpuWriter::default(),
            prev: Complex32::new(1.0, 0.0),
        }
    }
}

impl Default for FmDemod {
    fn default() -> Self {
        Self::new()
    }
}

impl<I, O> Kernel for FmDemod<I, O>
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
            let curr = inp[k];
            let dot = curr.re * self.prev.re + curr.im * self.prev.im;
            let cross = curr.im * self.prev.re - curr.re * self.prev.im;
            out[k] = cross.atan2(dot);
            self.prev = curr;
        }

        self.input.consume(m);
        self.output.produce(m);

        if self.input.finished() && m == in_len {
            io.finished = true;
        }

        Ok(())
    }
}
