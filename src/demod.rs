/// Convert raw rtl_tcp u8 I/Q bytes to normalized f32 pairs.
/// Each byte is offset by 127.5: 0 → -1.0, 127 ≈ 0.0, 255 → +1.0.
pub fn convert_iq(raw: &[u8]) -> Vec<(f32, f32)> {
    raw.chunks_exact(2)
        .map(|c| {
            let i = (c[0] as f32 - 127.5) / 127.5;
            let q = (c[1] as f32 - 127.5) / 127.5;
            (i, q)
        })
        .collect()
}

/// FM discriminator using the cross-product / dot-product method.
/// Returns instantaneous frequency deviation normalized to [-1, 1] (approximately).
pub fn demodulate_fm(samples: &[(f32, f32)], prev: &mut (f32, f32)) -> Vec<f32> {
    samples
        .iter()
        .map(|&curr| {
            let dot = curr.0 * prev.0 + curr.1 * prev.1;
            let cross = curr.1 * prev.0 - curr.0 * prev.1;
            *prev = curr;
            cross.atan2(dot)
        })
        .collect()
}

/// AM envelope detector with DC removal via a simple running-mean subtraction.
pub fn demodulate_am(samples: &[(f32, f32)], dc: &mut f32) -> Vec<f32> {
    samples
        .iter()
        .map(|&(i, q)| {
            let mag = (i * i + q * q).sqrt();
            // Slow DC tracking (τ ≈ 100 ms at 240 kHz input)
            *dc = *dc * 0.99997 + mag * 0.00003;
            mag - *dc
        })
        .collect()
}

/// Apply FM broadcast de-emphasis (75 µs time constant, Americas/Japan).
/// One-pole IIR: y[n] = α·x[n] + (1−α)·y[n−1]
pub fn deemphasis(samples: &[f32], sample_rate: f32, prev: &mut f32) -> Vec<f32> {
    let tau = 75e-6_f32;
    let alpha = 1.0 / (1.0 + sample_rate * tau);
    samples
        .iter()
        .map(|&x| {
            *prev = alpha * x + (1.0 - alpha) * *prev;
            *prev
        })
        .collect()
}

/// Decimate by taking every `factor`-th sample after a simple boxcar (moving-average) FIR.
pub fn decimate(samples: &[f32], factor: usize) -> Vec<f32> {
    samples
        .chunks(factor)
        .map(|chunk| chunk.iter().sum::<f32>() / chunk.len() as f32)
        .collect()
}

/// Convert normalized f32 PCM samples to interleaved i16 LE bytes.
pub fn pcm_to_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Write a WAV header suitable for streaming (data chunk size = 0xFFFF_FFFF).
pub fn streaming_wav_header(sample_rate: u32, channels: u16) -> Vec<u8> {
    let byte_rate = sample_rate * channels as u32 * 2; // 16-bit = 2 bytes/sample
    let block_align = channels * 2;
    let mut h = Vec::with_capacity(44);

    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // RIFF chunk size (streaming)
    h.extend_from_slice(b"WAVE");

    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&sample_rate.to_le_bytes());
    h.extend_from_slice(&byte_rate.to_le_bytes());
    h.extend_from_slice(&block_align.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    h.extend_from_slice(b"data");
    h.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // data chunk size (streaming)

    h
}
