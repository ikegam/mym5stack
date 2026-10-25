//! Next-token sampling: greedy, temperature, and top-p (nucleus).

use super::model::softmax;

pub struct Sampler {
    /// 0.0 = always pick the most likely token.
    pub temperature: f32,
    /// Sample only from the smallest set of tokens whose probability reaches this.
    pub topp: f32,
    rng: u64,
    cand: Vec<(f32, u32)>,
}

impl Sampler {
    pub fn new(temperature: f32, topp: f32, seed: u64) -> Self {
        Self { temperature, topp, rng: seed.max(1), cand: Vec::new() }
    }

    pub fn sample(&mut self, logits: &mut [f32]) -> u32 {
        if self.temperature == 0.0 {
            return argmax(logits);
        }
        for l in logits.iter_mut() {
            *l /= self.temperature;
        }
        softmax(logits);
        let coin = self.random_f32();
        if self.topp <= 0.0 || self.topp >= 1.0 {
            return sample_mult(logits, coin);
        }
        self.sample_topp(logits, coin)
    }

    fn sample_topp(&mut self, probs: &[f32], coin: f32) -> u32 {
        // Tokens below this can't be in the nucleus; skipping them avoids
        // sorting the whole vocabulary.
        let cutoff = (1.0 - self.topp) / (probs.len() - 1) as f32;
        self.cand.clear();
        self.cand.extend(probs.iter().enumerate().filter(|(_, &p)| p >= cutoff).map(|(i, &p)| (p, i as u32)));
        self.cand.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        let mut cum = 0.0;
        let mut last = self.cand.len() - 1;
        for (i, (p, _)) in self.cand.iter().enumerate() {
            cum += p;
            if cum > self.topp {
                last = i;
                break;
            }
        }
        let r = coin * cum;
        let mut acc = 0.0;
        for &(p, id) in &self.cand[..=last] {
            acc += p;
            if r < acc {
                return id;
            }
        }
        self.cand[last].1
    }

    /// xorshift64*
    fn random_f32(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let r = (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32;
        (r >> 8) as f32 / 16_777_216.0
    }
}

fn argmax(v: &[f32]) -> u32 {
    let mut best = 0;
    for (i, &x) in v.iter().enumerate() {
        if x > v[best] {
            best = i;
        }
    }
    best as u32
}

fn sample_mult(probs: &[f32], coin: f32) -> u32 {
    let mut acc = 0.0;
    for (i, &p) in probs.iter().enumerate() {
        acc += p;
        if coin < acc {
            return i as u32;
        }
    }
    probs.len() as u32 - 1
}
