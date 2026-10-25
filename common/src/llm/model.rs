//! Llama-2 transformer (llama2.c architecture) with int8 weights.
//!
//! Weights are quantized symmetrically in groups of `group_size` values with
//! one f32 scale per group (llama2.c "Q8_0"). Activations are quantized the
//! same way right before each matmul, so the inner loop is i8 x i8 -> i32.
//!
//! File layout (`.q8`, little endian, every section 4-byte aligned):
//!   header      HEADER_LEN bytes: magic, version, Config, group size, shared flag
//!   f32         rms_att[L*dim], rms_ffn[L*dim], rms_final[dim]
//!   q8 tensors  tok_emb, wq, wk, wv, wo, w1, w2, w3, [wcls]
//!               each as i8 values[n] followed by f32 scales[n / group_size]

use std::ops::Range;

use super::helper::Helper;

pub const MAGIC: &[u8; 4] = b"Q8LM";
pub const VERSION: u32 = 1;
pub const HEADER_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub dim: usize,
    pub hidden_dim: usize,
    pub n_layers: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    pub vocab_size: usize,
    pub seq_len: usize,
}

impl Config {
    pub fn head_size(&self) -> usize {
        self.dim / self.n_heads
    }

    pub fn kv_dim(&self) -> usize {
        self.dim * self.n_kv_heads / self.n_heads
    }
}

/// Where one quantized tensor lives in the file.
#[derive(Debug, Clone, Copy)]
struct QSpan {
    q: usize,
    s: usize,
    n: usize,
}

/// Byte offsets of every tensor, shared by the reader and the converter.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Layout {
    rms_att: usize,
    rms_ffn: usize,
    rms_final: usize,
    tok_emb: QSpan,
    wq: QSpan,
    wk: QSpan,
    wv: QSpan,
    wo: QSpan,
    w1: QSpan,
    w2: QSpan,
    w3: QSpan,
    wcls: QSpan,
    pub total: usize,
}

impl Layout {
    pub(crate) fn new(c: &Config, gs: usize, shared: bool) -> Self {
        let mut off = HEADER_LEN;
        let mut f32s = |n: usize| {
            let o = off;
            off += n * 4;
            o
        };
        let (rms_att, rms_ffn, rms_final) =
            (f32s(c.n_layers * c.dim), f32s(c.n_layers * c.dim), f32s(c.dim));
        let mut qt = |n: usize| {
            let q = off;
            let s = q + n;
            off = s + n / gs * 4;
            QSpan { q, s, n }
        };
        let (l, dim, kv, hid) = (c.n_layers, c.dim, c.kv_dim(), c.hidden_dim);
        let tok_emb = qt(c.vocab_size * dim);
        let wq = qt(l * dim * dim);
        let wk = qt(l * kv * dim);
        let wv = qt(l * kv * dim);
        let wo = qt(l * dim * dim);
        let w1 = qt(l * hid * dim);
        let w2 = qt(l * dim * hid);
        let w3 = qt(l * hid * dim);
        let wcls = if shared { tok_emb } else { qt(c.vocab_size * dim) };
        Self { rms_att, rms_ffn, rms_final, tok_emb, wq, wk, wv, wo, w1, w2, w3, wcls, total: off }
    }
}

pub(crate) fn write_header(out: &mut Vec<u8>, c: &Config, gs: usize, shared: bool) {
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    for v in [c.dim, c.hidden_dim, c.n_layers, c.n_heads, c.n_kv_heads, c.vocab_size, c.seq_len, gs] {
        out.extend_from_slice(&(v as u32).to_le_bytes());
    }
    out.extend_from_slice(&(shared as u32).to_le_bytes());
    out.resize(HEADER_LEN, 0);
}

/// Parses a `.q8` header: (config, group size, shared classifier).
pub fn read_header(h: &[u8]) -> Result<(Config, usize, bool), String> {
    if h.len() < HEADER_LEN || &h[..4] != MAGIC {
        return Err("not a Q8LM model file".into());
    }
    let u = |i: usize| u32::from_le_bytes(h[4 + i * 4..8 + i * 4].try_into().unwrap()) as usize;
    if u(0) != VERSION as usize {
        return Err(format!("unsupported Q8LM version {}", u(0)));
    }
    let c = Config {
        dim: u(1),
        hidden_dim: u(2),
        n_layers: u(3),
        n_heads: u(4),
        n_kv_heads: u(5),
        vocab_size: u(6),
        seq_len: u(7),
    };
    let gs = u(8);
    if gs == 0 || c.n_heads == 0 || c.n_kv_heads == 0 || c.dim % gs != 0 || c.hidden_dim % gs != 0 {
        return Err(format!("bad model header: {c:?} group_size={gs}"));
    }
    Ok((c, gs, u(9) != 0))
}

/// int8 weights of one matrix (or a stack of per-layer matrices).
#[derive(Clone, Copy)]
pub struct QView<'a> {
    pub q: &'a [i8],
    pub s: &'a [f32],
}

impl<'a> QView<'a> {
    /// Matrix `i` of a stack whose matrices hold `n` values each.
    fn layer(self, i: usize, n: usize, gs: usize) -> Self {
        Self { q: &self.q[i * n..(i + 1) * n], s: &self.s[i * n / gs..(i + 1) * n / gs] }
    }
}

/// 16 bytes, 16-aligned: backing storage for buffers the SIMD loads read.
#[derive(Clone, Copy, Default)]
#[repr(C, align(16))]
pub struct Block([u32; 4]);

/// Model file contents. Block-aligned so the f32 sections are aligned and
/// int8 rows can be fed to 128-bit vector loads.
pub struct Weights {
    buf: Vec<Block>,
    pub config: Config,
    pub group_size: usize,
    layout: Layout,
}

impl Weights {
    /// Allocates a zeroed buffer for a model file of `len` bytes; fill it
    /// through `as_bytes_mut` and then call `from_buf`.
    pub fn alloc(len: usize) -> Vec<Block> {
        vec![Block::default(); len.div_ceil(16)]
    }

    pub fn from_buf(buf: Vec<Block>, len: usize) -> Result<Self, String> {
        let bytes = blocks_as_bytes(&buf);
        let (config, group_size, shared) = read_header(bytes)?;
        let layout = Layout::new(&config, group_size, shared);
        if layout.total != len || len > bytes.len() {
            return Err(format!("model file is {len} bytes, header says {}", layout.total));
        }
        Ok(Self { buf, config, group_size, layout })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut buf = Self::alloc(bytes.len());
        as_bytes_mut(&mut buf)[..bytes.len()].copy_from_slice(bytes);
        Self::from_buf(buf, bytes.len())
    }

    fn f32s(&self, off: usize, n: usize) -> &[f32] {
        let bytes = &blocks_as_bytes(&self.buf)[off..off + n * 4];
        assert!(off % 4 == 0);
        // SAFETY: in bounds (sliced above) and 4-byte aligned (aligned buffer, off % 4 == 0).
        unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<f32>(), n) }
    }

    fn qview(&self, t: QSpan) -> QView<'_> {
        let bytes = blocks_as_bytes(&self.buf);
        let q = &bytes[t.q..t.q + t.n];
        // SAFETY: i8 and u8 have the same layout.
        let q = unsafe { std::slice::from_raw_parts(q.as_ptr().cast::<i8>(), q.len()) };
        QView { q, s: self.f32s(t.s, t.n / self.group_size) }
    }

    fn range(&self, off: usize, r: Range<usize>) -> &[f32] {
        &self.f32s(off, r.end)[r]
    }
}

pub(crate) fn blocks_as_bytes(b: &[Block]) -> &[u8] {
    // SAFETY: Block is plain data without padding; u8 has no alignment requirement.
    unsafe { std::slice::from_raw_parts(b.as_ptr().cast::<u8>(), b.len() * 16) }
}

pub fn as_bytes_mut(b: &mut [Block]) -> &mut [u8] {
    // SAFETY: as above; any byte pattern is a valid Block.
    unsafe { std::slice::from_raw_parts_mut(b.as_mut_ptr().cast::<u8>(), b.len() * 16) }
}

/// A vector quantized for matmul input.
struct QBuf {
    q: Vec<Block>,
    s: Vec<f32>,
}

impl QBuf {
    fn new(n: usize, gs: usize) -> Self {
        Self { q: vec![Block::default(); n.div_ceil(16)], s: vec![0.0; n / gs] }
    }

    fn q(&self) -> &[i8] {
        let b = blocks_as_bytes(&self.q);
        // SAFETY: i8 and u8 have the same layout.
        unsafe { std::slice::from_raw_parts(b.as_ptr().cast::<i8>(), b.len()) }
    }

    fn quantize(&mut self, x: &[f32], gs: usize) {
        let b = as_bytes_mut(&mut self.q);
        // SAFETY: as in `q`.
        let q = unsafe { std::slice::from_raw_parts_mut(b.as_mut_ptr().cast::<i8>(), b.len()) };
        quantize(q, &mut self.s, x, gs);
    }
}

/// Symmetric per-group int8 quantization of `x` into `q` with scales `s`.
pub(crate) fn quantize(q: &mut [i8], s: &mut [f32], x: &[f32], gs: usize) {
    for ((xg, qg), s) in x.chunks_exact(gs).zip(q.chunks_exact_mut(gs)).zip(s) {
        let max = xg.iter().fold(0f32, |m, v| m.max(v.abs()));
        *s = max / 127.0;
        let inv = if max > 0.0 { 127.0 / max } else { 0.0 };
        for (q, v) in qg.iter_mut().zip(xg) {
            *q = (v * inv).round() as i8;
        }
    }
}

/// Activation buffers and KV cache for one generation stream.
struct State {
    x: Vec<f32>,
    xb: Vec<f32>,
    xb2: Vec<f32>,
    hb: Vec<f32>,
    hb2: Vec<f32>,
    xq: QBuf,
    q: Vec<f32>,
    att: Vec<f32>,
    logits: Vec<f32>,
    key_cache: Vec<f32>,
    value_cache: Vec<f32>,
}

pub struct Transformer {
    pub w: Weights,
    s: State,
    /// Takes half of each matmul when set (see `set_threads`).
    helper: Option<Helper>,
}

impl Transformer {
    pub fn new(w: Weights) -> Self {
        let c = w.config;
        let gs = w.group_size;
        let cache = c.n_layers * c.seq_len * c.kv_dim();
        let s = State {
            x: vec![0.0; c.dim],
            xb: vec![0.0; c.dim],
            xb2: vec![0.0; c.dim],
            hb: vec![0.0; c.hidden_dim],
            hb2: vec![0.0; c.hidden_dim],
            xq: QBuf::new(c.dim.max(c.hidden_dim), gs),
            q: vec![0.0; c.dim],
            att: vec![0.0; c.n_heads * c.seq_len],
            logits: vec![0.0; c.vocab_size],
            key_cache: vec![0.0; cache],
            value_cache: vec![0.0; cache],
        };
        Self { w, s, helper: None }
    }

    /// Splits matmuls over 1 or 2 threads.
    pub fn set_threads(&mut self, n: usize) {
        self.helper = (n > 1).then(Helper::new);
    }

    pub fn config(&self) -> &Config {
        &self.w.config
    }

    /// Runs `token` at position `pos` and returns the next-token logits.
    pub fn forward(&mut self, token: u32, pos: usize) -> &mut [f32] {
        let w = &self.w;
        let c = w.config;
        let gs = w.group_size;
        let lay = &w.layout;
        let s = &mut self.s;
        let helper = self.helper.as_ref();
        let (dim, hid, kv_dim, hs) = (c.dim, c.hidden_dim, c.kv_dim(), c.head_size());
        let kv_mul = c.n_heads / c.n_kv_heads;

        let emb = w.qview(lay.tok_emb).layer(token as usize, dim, gs);
        for ((xg, qg), sc) in s.x.chunks_exact_mut(gs).zip(emb.q.chunks_exact(gs)).zip(emb.s) {
            for (x, &q) in xg.iter_mut().zip(qg) {
                *x = q as f32 * sc;
            }
        }

        for l in 0..c.n_layers {
            rmsnorm(&mut s.xb, &s.x, w.range(lay.rms_att, l * dim..(l + 1) * dim));
            s.xq.quantize(&s.xb, gs);
            let loff = l * c.seq_len * kv_dim;
            let k = &mut s.key_cache[loff + pos * kv_dim..][..kv_dim];
            let v = &mut s.value_cache[loff + pos * kv_dim..][..kv_dim];
            matmul(&mut s.q, &s.xq, w.qview(lay.wq).layer(l, dim * dim, gs), dim, gs, helper);
            matmul(k, &s.xq, w.qview(lay.wk).layer(l, kv_dim * dim, gs), dim, gs, helper);
            matmul(v, &s.xq, w.qview(lay.wv).layer(l, kv_dim * dim, gs), dim, gs, helper);

            // RoPE
            for i in (0..dim).step_by(2) {
                let freq = 1.0 / 10000f32.powf((i % hs) as f32 / hs as f32);
                let (fci, fcr) = (pos as f32 * freq).sin_cos();
                let rot = |vec: &mut [f32]| {
                    let (v0, v1) = (vec[i], vec[i + 1]);
                    vec[i] = v0 * fcr - v1 * fci;
                    vec[i + 1] = v0 * fci + v1 * fcr;
                };
                rot(&mut s.q);
                if i < kv_dim {
                    rot(k);
                }
            }

            // Multi-head attention over positions 0..=pos
            let scale = 1.0 / (hs as f32).sqrt();
            for h in 0..c.n_heads {
                let q = &s.q[h * hs..(h + 1) * hs];
                let att = &mut s.att[h * c.seq_len..h * c.seq_len + pos + 1];
                let kvo = loff + (h / kv_mul) * hs;
                for (t, a) in att.iter_mut().enumerate() {
                    let k = &s.key_cache[kvo + t * kv_dim..][..hs];
                    *a = q.iter().zip(k).map(|(a, b)| a * b).sum::<f32>() * scale;
                }
                softmax(att);
                let xb = &mut s.xb[h * hs..(h + 1) * hs];
                xb.fill(0.0);
                for (t, &a) in att.iter().enumerate() {
                    let v = &s.value_cache[kvo + t * kv_dim..][..hs];
                    for (o, v) in xb.iter_mut().zip(v) {
                        *o += a * v;
                    }
                }
            }

            s.xq.quantize(&s.xb, gs);
            matmul(&mut s.xb2, &s.xq, w.qview(lay.wo).layer(l, dim * dim, gs), dim, gs, helper);
            add(&mut s.x, &s.xb2);

            // SwiGLU feed-forward
            rmsnorm(&mut s.xb, &s.x, w.range(lay.rms_ffn, l * dim..(l + 1) * dim));
            s.xq.quantize(&s.xb, gs);
            matmul(&mut s.hb, &s.xq, w.qview(lay.w1).layer(l, hid * dim, gs), dim, gs, helper);
            matmul(&mut s.hb2, &s.xq, w.qview(lay.w3).layer(l, hid * dim, gs), dim, gs, helper);
            for (a, b) in s.hb.iter_mut().zip(&s.hb2) {
                *a = *a / (1.0 + (-*a).exp()) * b;
            }
            s.xq.quantize(&s.hb, gs);
            matmul(&mut s.xb, &s.xq, w.qview(lay.w2).layer(l, dim * hid, gs), hid, gs, helper);
            add(&mut s.x, &s.xb);
        }

        let x = s.x.clone();
        rmsnorm(&mut s.x, &x, w.f32s(lay.rms_final, dim));
        s.xq.quantize(&s.x, gs);
        matmul(&mut s.logits, &s.xq, w.qview(lay.wcls), dim, gs, helper);
        &mut s.logits
    }
}

fn add(x: &mut [f32], y: &[f32]) {
    for (a, b) in x.iter_mut().zip(y) {
        *a += b;
    }
}

fn rmsnorm(o: &mut [f32], x: &[f32], weight: &[f32]) {
    let ss = x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32;
    let inv = 1.0 / (ss + 1e-5).sqrt();
    for ((o, x), w) in o.iter_mut().zip(x).zip(weight) {
        *o = w * (inv * x);
    }
}

pub(crate) fn softmax(x: &mut [f32]) {
    let max = x.iter().fold(f32::NEG_INFINITY, |m, &v| m.max(v));
    let mut sum = 0.0;
    for v in x.iter_mut() {
        *v = (*v - max).exp();
        sum += *v;
    }
    for v in x.iter_mut() {
        *v /= sum;
    }
}

/// `out[i] = Σ_j W[i][j] · x[j]` with `n` inputs; the helper, if any, takes
/// the second half of the rows.
fn matmul(out: &mut [f32], x: &QBuf, w: QView, n: usize, gs: usize, helper: Option<&Helper>) {
    let Some(helper) = helper.filter(|_| out.len() >= 64) else {
        return matmul_rows(out, 0, x, w, n, gs);
    };
    let half = out.len() / 2;
    let (a, b) = out.split_at_mut(half);
    helper.run(&mut || matmul_rows(b, half, x, w, n, gs), || matmul_rows(a, 0, x, w, n, gs));
}

fn matmul_rows(out: &mut [f32], row0: usize, x: &QBuf, w: QView, n: usize, gs: usize) {
    let (xq, xs) = (&x.q()[..n], &x.s[..n / gs]);
    for (i, o) in out.iter_mut().enumerate() {
        let r = row0 + i;
        let wq = &w.q[r * n..(r + 1) * n];
        let ws = &w.s[r * n / gs..(r + 1) * n / gs];
        let simd = simd::usable(wq, xq, gs);
        let mut acc = 0f32;
        for ((wg, xg), (sw, sx)) in wq.chunks_exact(gs).zip(xq.chunks_exact(gs)).zip(ws.iter().zip(xs)) {
            let dot = if simd { simd::dot(wg, xg) } else { dot_scalar(wg, xg) };
            acc += dot as f32 * sw * sx;
        }
        *o = acc;
    }
}

fn dot_scalar(a: &[i8], b: &[i8]) -> i32 {
    a.iter().zip(b).map(|(&a, &b)| a as i32 * b as i32).sum()
}

/// int8 dot products on the ESP32-P4's PIE vector unit (16 MACs per instruction).
#[cfg(all(target_arch = "riscv32", target_os = "espidf"))]
mod simd {
    use core::arch::asm;

    /// Both rows 16-byte aligned and whole vectors per group.
    pub fn usable(w: &[i8], x: &[i8], gs: usize) -> bool {
        (w.as_ptr() as usize | x.as_ptr() as usize) & 15 == 0 && gs % 16 == 0
    }

    pub fn dot(w: &[i8], x: &[i8]) -> i32 {
        debug_assert!(w.len() == x.len() && w.len() % 16 == 0);
        let (mut pw, mut px) = (w.as_ptr(), x.as_ptr());
        let out: i32;
        // Raw encodings from IDF's GCC (`-march=..._xesppie`, the ISA IDF
        // builds for every P4 revision); rustc's LLVM names these extensions
        // differently per ESPV version. Registers are fixed to match:
        //   0000005b  esp.zero.xacc
        //   0201223b  esp.vld.128.ip q0, a0, 16
        //   0201a63b  esp.vld.128.ip q1, a1, 16
        //   06c1005f  esp.vmulas.s8.xacc q0, q1
        //   84b0025f  esp.movx.r.xacc.l a2
        // SAFETY: reads exactly w.len() bytes from each 16-aligned slice;
        // XACC and q0/q1 are not used by compiled code.
        unsafe {
            asm!(".word 0x0000005b", options(nostack));
            for _ in 0..w.len() / 16 {
                asm!(
                    ".word 0x0201223b",
                    ".word 0x0201a63b",
                    ".word 0x06c1005f",
                    inout("a0") pw,
                    inout("a1") px,
                    options(nostack, readonly),
                );
            }
            asm!(".word 0x84b0025f", out("a2") out, options(nostack));
        }
        out
    }
}

#[cfg(not(all(target_arch = "riscv32", target_os = "espidf")))]
mod simd {
    pub fn usable(_: &[i8], _: &[i8], _: usize) -> bool {
        false
    }

    pub fn dot(a: &[i8], b: &[i8]) -> i32 {
        super::dot_scalar(a, b)
    }
}

/// Rough throughput numbers for tuning on the device.
pub fn bench(w: &Weights) -> String {
    use std::time::Instant;
    let gs = w.group_size;
    let dim = w.config.dim;

    let bytes = blocks_as_bytes(&w.buf);
    let t = Instant::now();
    let sum = w.buf.iter().fold(0u32, |a, b| a.wrapping_add(b.0[0] ^ b.0[1] ^ b.0[2] ^ b.0[3]));
    let read = bytes.len() as f32 / t.elapsed().as_secs_f32() / 1e6;

    let mut x = QBuf::new(dim, gs);
    let input: Vec<f32> = (0..dim).map(|i| (i as f32 * 0.37).sin()).collect();
    x.quantize(&input, gs);
    let cls = w.qview(w.layout.wcls);
    let rows = w.config.vocab_size;
    let mut out = vec![0f32; rows];
    let t = Instant::now();
    matmul_rows(&mut out, 0, &x, cls, dim, gs);
    let psram = (rows * dim) as f32 / t.elapsed().as_secs_f32() / 1e6;

    // Same kernel on a copy small enough to sit in internal RAM.
    let rows_int = 16 * 1024 / dim;
    let mut qb = vec![Block::default(); rows_int * dim / 16];
    let qbytes = as_bytes_mut(&mut qb);
    for (d, &v) in qbytes.iter_mut().zip(&cls.q[..rows_int * dim]) {
        *d = v as u8;
    }
    // SAFETY: i8 and u8 have the same layout.
    let q = unsafe { std::slice::from_raw_parts(qbytes.as_ptr().cast::<i8>(), qbytes.len()) };
    let s: Vec<f32> = cls.s[..rows_int * dim / gs].to_vec();
    let mut out_int = vec![0f32; rows_int];
    let reps = 200;
    let t = Instant::now();
    for _ in 0..reps {
        matmul_rows(&mut out_int, 0, &x, QView { q, s: &s }, dim, gs);
    }
    let internal = (reps * rows_int * dim) as f32 / t.elapsed().as_secs_f32() / 1e6;

    let xq = &x.q()[..dim];
    let simd = simd::usable(&q[..dim], xq, gs);
    let simd_ok = q.chunks_exact(dim).all(|row| simd::dot(row, xq) == dot_scalar(row, xq));
    format!(
        "psram read {read:.0} MB/s, int8 matmul {psram:.0} M MAC/s from psram, {internal:.0} M MAC/s internal, \
         simd={simd} matches scalar={simd_ok} (checksum {sum:x} {:.3})",
        out[0] + out_int[0]
    )
}
