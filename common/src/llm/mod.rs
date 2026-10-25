//! Tiny Llama-2 inference (Karpathy's llama2.c TinyStories models) with
//! int8 weights, sized for the ESP32-P4's 32MB PSRAM.

mod convert;
mod helper;
mod model;
mod sampler;
mod tokenizer;

pub use convert::convert_llama2c;
pub use model::{as_bytes_mut, bench, read_header, Config, Transformer, Weights, HEADER_LEN};
pub use sampler::Sampler;
pub use tokenizer::{Tokenizer, Utf8Stream, BOS, EOS};

/// Generates up to `steps` tokens (prompt included) and passes the text to
/// `emit` as it is produced; `emit` returns false to stop early. Returns the
/// number of forward passes run.
pub fn generate(
    t: &mut Transformer,
    tok: &Tokenizer,
    sampler: &mut Sampler,
    prompt: &str,
    steps: usize,
    mut emit: impl FnMut(&str) -> bool,
) -> usize {
    let prompt = tok.encode(prompt, true);
    let steps = steps.min(t.config().seq_len);
    let mut text = Utf8Stream::default();
    let mut token = prompt[0];
    let mut pos = 0;
    while pos < steps {
        let logits = t.forward(token, pos);
        let next = match prompt.get(pos + 1) {
            Some(&p) => p,
            None => sampler.sample(logits),
        };
        pos += 1;
        // TinyStories models separate stories with BOS.
        if next == BOS || next == EOS {
            break;
        }
        let mut more = true;
        text.push(tok.decode(token, next), |s: &str| more &= emit(s));
        if !more {
            break;
        }
        token = next;
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: Config =
        Config { dim: 64, hidden_dim: 128, n_layers: 2, n_heads: 4, n_kv_heads: 2, vocab_size: 300, seq_len: 16 };

    struct Rng(u64);
    impl Rng {
        fn f(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.5
        }
        fn v(&mut self, n: usize) -> Vec<f32> {
            (0..n).map(|_| self.f()).collect()
        }
    }

    /// fp32 weights in llama2.c checkpoint order.
    struct Fp32 {
        emb: Vec<f32>,
        rms_att: Vec<f32>,
        wq: Vec<f32>,
        wk: Vec<f32>,
        wv: Vec<f32>,
        wo: Vec<f32>,
        rms_ffn: Vec<f32>,
        w1: Vec<f32>,
        w2: Vec<f32>,
        w3: Vec<f32>,
        rms_final: Vec<f32>,
    }

    fn random_model(c: &Config) -> (Fp32, Vec<u8>) {
        let mut r = Rng(42);
        let (l, d, kv, h) = (c.n_layers, c.dim, c.kv_dim(), c.hidden_dim);
        let ones = |n: usize, r: &mut Rng| (0..n).map(|_| 1.0 + r.f() * 0.2).collect::<Vec<_>>();
        let m = Fp32 {
            emb: r.v(c.vocab_size * d),
            rms_att: ones(l * d, &mut r),
            wq: r.v(l * d * d),
            wk: r.v(l * kv * d),
            wv: r.v(l * kv * d),
            wo: r.v(l * d * d),
            rms_ffn: ones(l * d, &mut r),
            w1: r.v(l * h * d),
            w2: r.v(l * d * h),
            w3: r.v(l * h * d),
            rms_final: ones(d, &mut r),
        };
        let mut file = Vec::new();
        for v in [c.dim, c.hidden_dim, c.n_layers, c.n_heads, c.n_kv_heads, c.vocab_size, c.seq_len] {
            file.extend((v as i32).to_le_bytes());
        }
        let freq = vec![0f32; c.seq_len * c.head_size()];
        for t in [
            &m.emb, &m.rms_att, &m.wq, &m.wk, &m.wv, &m.wo, &m.rms_ffn, &m.w1, &m.w2, &m.w3, &m.rms_final, &freq,
        ] {
            file.extend(t.iter().flat_map(|f| f.to_le_bytes()));
        }
        (m, file)
    }

    /// Straight fp32 port of llama2.c `forward`, run over a whole sequence.
    fn reference(m: &Fp32, c: &Config, tokens: &[u32]) -> Vec<Vec<f32>> {
        let (d, kv, hs, hid) = (c.dim, c.kv_dim(), c.head_size(), c.hidden_dim);
        let mm = |w: &[f32], x: &[f32], rows: usize| -> Vec<f32> {
            (0..rows).map(|i| (0..x.len()).map(|j| w[i * x.len() + j] * x[j]).sum()).collect()
        };
        let rms = |x: &[f32], w: &[f32]| -> Vec<f32> {
            let ss = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32 + 1e-5).sqrt();
            x.iter().zip(w).map(|(x, w)| w * x / ss).collect()
        };
        let mut kc = vec![vec![]; c.n_layers];
        let mut vc = vec![vec![]; c.n_layers];
        let mut out = vec![];
        for (pos, &tok) in tokens.iter().enumerate() {
            let mut x = m.emb[tok as usize * d..][..d].to_vec();
            for l in 0..c.n_layers {
                let xb = rms(&x, &m.rms_att[l * d..][..d]);
                let mut q = mm(&m.wq[l * d * d..], &xb, d);
                let mut k = mm(&m.wk[l * kv * d..], &xb, kv);
                let v = mm(&m.wv[l * kv * d..], &xb, kv);
                for i in (0..d).step_by(2) {
                    let f = 1.0 / 10000f32.powf((i % hs) as f32 / hs as f32);
                    let (s, co) = (pos as f32 * f).sin_cos();
                    for vec in [&mut q, &mut k] {
                        if i < vec.len() {
                            let (a, b) = (vec[i], vec[i + 1]);
                            vec[i] = a * co - b * s;
                            vec[i + 1] = a * s + b * co;
                        }
                    }
                }
                kc[l].push(k);
                vc[l].push(v);
                let mut att_out = vec![0f32; d];
                for h in 0..c.n_heads {
                    let kh = h / (c.n_heads / c.n_kv_heads) * hs;
                    let mut a: Vec<f32> = (0..=pos)
                        .map(|t| (0..hs).map(|i| q[h * hs + i] * kc[l][t][kh + i]).sum::<f32>() / (hs as f32).sqrt())
                        .collect();
                    model::softmax(&mut a);
                    for t in 0..=pos {
                        for i in 0..hs {
                            att_out[h * hs + i] += a[t] * vc[l][t][kh + i];
                        }
                    }
                }
                let o = mm(&m.wo[l * d * d..], &att_out, d);
                x.iter_mut().zip(&o).for_each(|(a, b)| *a += b);
                let xb = rms(&x, &m.rms_ffn[l * d..][..d]);
                let h1 = mm(&m.w1[l * hid * d..], &xb, hid);
                let h3 = mm(&m.w3[l * hid * d..], &xb, hid);
                let hb: Vec<f32> = h1.iter().zip(&h3).map(|(a, b)| a / (1.0 + (-a).exp()) * b).collect();
                let o = mm(&m.w2[l * d * hid..], &hb, d);
                x.iter_mut().zip(&o).for_each(|(a, b)| *a += b);
            }
            out.push(mm(&m.emb, &rms(&x, &m.rms_final), c.vocab_size));
        }
        out
    }

    #[test]
    fn int8_forward_matches_fp32_reference() {
        let (fp, file) = random_model(&CFG);
        let q8 = convert_llama2c(&file).unwrap();
        let tokens = [1u32, 17, 250, 3, 99, 42, 7, 7];
        let want = reference(&fp, &CFG, &tokens);
        for threads in [1, 2] {
            let mut t = Transformer::new(Weights::from_bytes(&q8).unwrap());
            assert_eq!(*t.config(), CFG);
            t.set_threads(threads);
            for (pos, &tok) in tokens.iter().enumerate() {
                let got = t.forward(tok, pos);
                let err = got.iter().zip(&want[pos]).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
                let scale = want[pos].iter().fold(0f32, |m, v| m.max(v.abs()));
                assert!(err < 0.05 * scale, "pos {pos} threads {threads}: err {err} vs scale {scale}");
            }
        }
    }

    #[test]
    fn rejects_bad_files() {
        let (_, file) = random_model(&CFG);
        assert!(Weights::from_bytes(&file).is_err());
        let q8 = convert_llama2c(&file).unwrap();
        assert!(Weights::from_bytes(&q8[..q8.len() - 4]).is_err());
    }

    fn tiny_tokenizer() -> Tokenizer {
        // 0 <unk>, 1 <s>, 2 </s>, 3..=258 bytes, then merges.
        let mut pieces: Vec<(Vec<u8>, f32)> = vec![(b"<unk>".to_vec(), 0.0), (b"<s>".to_vec(), 0.0), (b"</s>".to_vec(), 0.0)];
        pieces.extend((0..=255u8).map(|b| (format!("<0x{b:02X}>").into_bytes(), 0.0)));
        for (p, s) in [(" ", -1.0), ("a", -1.0), ("b", -1.0), ("ab", -0.5), (" ab", -0.1), ("ba", -0.2)] {
            pieces.push((p.as_bytes().to_vec(), s));
        }
        let mut data = 8i32.to_le_bytes().to_vec();
        for (p, s) in &pieces {
            data.extend(s.to_le_bytes());
            data.extend((p.len() as i32).to_le_bytes());
            data.extend(p);
        }
        Tokenizer::from_bytes(&data, pieces.len()).unwrap()
    }

    #[test]
    fn tokenizer_round_trip() {
        let t = tiny_tokenizer();
        let ids = t.encode("ab", true);
        // " " + "a" + "b" -> " " "ab" -> " ab"
        assert_eq!(ids, vec![BOS, 263]);
        assert_eq!(t.decode(BOS, 263), b"ab");
        assert_eq!(t.decode(5, 263), b" ab");
        // Unknown character falls back to its UTF-8 bytes.
        let ids = t.encode("é", false);
        assert_eq!(ids, vec![259, 0xc3 + 3, 0xa9 + 3]);
        assert_eq!(t.decode(259, 0xc3 + 3), &[0xc3]);
    }

    #[test]
    fn utf8_stream_holds_partial_sequences() {
        let mut s = Utf8Stream::default();
        let mut out = String::new();
        s.push(&[b'x', 0xc3], |t| out.push_str(t));
        assert_eq!(out, "x");
        s.push(&[0xa9, 0xff, b'y'], |t| out.push_str(t));
        assert_eq!(out, "xé\u{fffd}y");
    }

    #[test]
    fn greedy_sampling_picks_max() {
        let mut s = Sampler::new(0.0, 0.9, 1);
        assert_eq!(s.sample(&mut [0.1, 3.0, -1.0]), 1);
        let mut s = Sampler::new(1.0, 0.5, 7);
        // With top-p 0.5 only the dominant token is eligible.
        for _ in 0..20 {
            assert_eq!(s.sample(&mut [0.0, 5.0, 0.0, 0.0]), 1);
        }
    }
}
