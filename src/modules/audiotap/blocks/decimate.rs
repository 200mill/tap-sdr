//! Boxcar decimation block (240 kHz → 48 kHz at factor 5).
//!
//! Same boxcar moving-average as `demod::decimate`, but accumulates across work calls so streaming
//! chunk boundaries don't matter. Emits one averaged sample per `factor` inputs (remainder carried).

use futuresdr::runtime::dev::prelude::*;

#[derive(Block)]
pub struct Decimate<I = DefaultCpuReader<f32>, O = DefaultCpuWriter<f32>>
where
    I: CpuBufferReader<Item = f32>,
    O: CpuBufferWriter<Item = f32>,
{
    #[input]
    input: I,
    #[output]
    output: O,
    factor: usize,
    acc: f32,
    count: usize,
}

impl Decimate<DefaultCpuReader<f32>, DefaultCpuWriter<f32>> {
    pub fn new(factor: usize) -> Self {
        assert!(factor >= 1, "decimation factor must be >= 1");
        Self {
            input: DefaultCpuReader::default(),
            output: DefaultCpuWriter::default(),
            factor,
            acc: 0.0,
            count: 0,
        }
    }
}

impl<I, O> Kernel for Decimate<I, O>
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
        let out_len = out.len();

        let mut consumed = 0;
        let mut produced = 0;

        for &x in inp.iter() {
            if self.count == self.factor - 1 && produced >= out_len {
                break;
            }
            self.acc += x;
            self.count += 1;
            consumed += 1;
            if self.count == self.factor {
                out[produced] = self.acc / self.factor as f32;
                produced += 1;
                self.acc = 0.0;
                self.count = 0;
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
