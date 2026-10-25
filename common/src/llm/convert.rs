//! llama2.c fp32 checkpoint (`stories15M.bin`, legacy "version 0" export)
//! -> `.q8` model file.

use super::model::{quantize, write_header, Config, Layout};

/// Largest group size we try; halved until it divides dim and hidden_dim.
const MAX_GROUP_SIZE: usize = 64;

pub fn convert_llama2c(src: &[u8]) -> Result<Vec<u8>, String> {
    if src.len() < 28 {
        return Err("file too short for a llama2.c header".into());
    }
    let h = |i: usize| i32::from_le_bytes(src[i * 4..i * 4 + 4].try_into().unwrap());
    // A negative vocab size marks a separate (unshared) classifier.
    let shared = h(5) > 0;
    let c = Config {
        dim: h(0) as usize,
        hidden_dim: h(1) as usize,
        n_layers: h(2) as usize,
        n_heads: h(3) as usize,
        n_kv_heads: h(4) as usize,
        vocab_size: h(5).unsigned_abs() as usize,
        seq_len: h(6) as usize,
    };
    let mut gs = MAX_GROUP_SIZE;
    while gs > 1 && (c.dim % gs != 0 || c.hidden_dim % gs != 0) {
        gs /= 2;
    }

    let mut floats = src[28..].chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap()));
    let mut take = |n: usize| -> Result<Vec<f32>, String> {
        let v: Vec<f32> = floats.by_ref().take(n).collect();
        if v.len() == n { Ok(v) } else { Err("checkpoint is truncated".into()) }
    };
    let (l, dim, kv, hid) = (c.n_layers, c.dim, c.kv_dim(), c.hidden_dim);
    let tok_emb = take(c.vocab_size * dim)?;
    let rms_att = take(l * dim)?;
    let wq = take(l * dim * dim)?;
    let wk = take(l * kv * dim)?;
    let wv = take(l * kv * dim)?;
    let wo = take(l * dim * dim)?;
    let rms_ffn = take(l * dim)?;
    let w1 = take(l * hid * dim)?;
    let w2 = take(l * dim * hid)?;
    let w3 = take(l * hid * dim)?;
    let rms_final = take(dim)?;
    // Precomputed RoPE tables (freq_cis_real / imag); we compute them on the fly.
    take(c.seq_len * c.head_size())?;
    let wcls = if shared { None } else { Some(take(c.vocab_size * dim)?) };

    let mut out = Vec::with_capacity(Layout::new(&c, gs, shared).total);
    write_header(&mut out, &c, gs, shared);
    for v in [&rms_att, &rms_ffn, &rms_final] {
        out.extend(v.iter().flat_map(|f| f.to_le_bytes()));
    }
    for t in [&tok_emb, &wq, &wk, &wv, &wo, &w1, &w2, &w3].into_iter().chain(wcls.as_ref()) {
        let mut q = vec![0i8; t.len()];
        let mut s = vec![0f32; t.len() / gs];
        quantize(&mut q, &mut s, t, gs);
        out.extend(q.iter().map(|&b| b as u8));
        out.extend(s.iter().flat_map(|f| f.to_le_bytes()));
    }
    debug_assert_eq!(out.len(), Layout::new(&c, gs, shared).total);
    Ok(out)
}
