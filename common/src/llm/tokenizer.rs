//! SentencePiece-style BPE tokenizer in llama2.c's `tokenizer.bin` format:
//! `max_token_length: i32`, then per token `score: f32, len: i32, bytes`.

pub const BOS: u32 = 1;
pub const EOS: u32 = 2;

pub struct Tokenizer {
    /// All pieces back to back; `spans[id]` is piece `id`'s range in it.
    /// Kept in a few large buffers so they land in PSRAM on the device.
    pieces: Vec<u8>,
    spans: Vec<(u32, u32)>,
    scores: Vec<f32>,
    /// Token ids ordered by piece bytes, for binary search.
    sorted: Vec<u32>,
    /// Single-byte pieces for `<0xXX>` tokens.
    bytes: [u8; 256],
}

impl Tokenizer {
    pub fn from_bytes(data: &[u8], vocab_size: usize) -> Result<Self, String> {
        let mut p = 4;
        let mut next = |n: usize| -> Result<&[u8], String> {
            let b = data.get(p..p + n).ok_or("tokenizer file is truncated")?;
            p += n;
            Ok(b)
        };
        let mut pieces = Vec::with_capacity(data.len());
        let mut spans = Vec::with_capacity(vocab_size);
        let mut scores = Vec::with_capacity(vocab_size);
        for _ in 0..vocab_size {
            scores.push(f32::from_le_bytes(next(4)?.try_into().unwrap()));
            let len = i32::from_le_bytes(next(4)?.try_into().unwrap());
            let start = pieces.len() as u32;
            pieces.extend_from_slice(next(len.max(0) as usize)?);
            spans.push((start, pieces.len() as u32));
        }
        let mut t = Self { pieces, spans, scores, sorted: Vec::new(), bytes: std::array::from_fn(|i| i as u8) };
        let mut sorted: Vec<u32> = (0..vocab_size as u32).collect();
        sorted.sort_by(|&a, &b| t.piece(a).cmp(t.piece(b)));
        t.sorted = sorted;
        Ok(t)
    }

    fn piece(&self, id: u32) -> &[u8] {
        let (a, b) = self.spans[id as usize];
        &self.pieces[a as usize..b as usize]
    }

    fn lookup(&self, piece: &[u8]) -> Option<u32> {
        let i = self.sorted.binary_search_by(|&id| self.piece(id).cmp(piece)).ok()?;
        Some(self.sorted[i])
    }

    /// Bytes of `token` when it follows `prev`.
    pub fn decode(&self, prev: u32, token: u32) -> &[u8] {
        let mut piece = self.piece(token);
        // The tokenizer adds a leading space to text; drop it after BOS.
        if prev == BOS && piece.first() == Some(&b' ') {
            piece = &piece[1..];
        }
        if let Some(b) = raw_byte(piece) {
            return &self.bytes[b as usize..b as usize + 1];
        }
        piece
    }

    pub fn encode(&self, text: &str, bos: bool) -> Vec<u32> {
        let mut tokens = Vec::new();
        if bos {
            tokens.push(BOS);
        }
        if !text.is_empty() {
            if let Some(sp) = self.lookup(b" ") {
                tokens.push(sp);
            }
        }
        let mut buf = [0u8; 4];
        for ch in text.chars() {
            let s = ch.encode_utf8(&mut buf).as_bytes();
            match self.lookup(s) {
                Some(id) => tokens.push(id),
                // Byte fallback: ids 3..=258 are <0x00>..<0xFF>.
                None => tokens.extend(s.iter().map(|&b| b as u32 + 3)),
            }
        }
        // Repeatedly merge the adjacent pair whose merged piece scores best.
        let mut pair = Vec::new();
        loop {
            let mut best: Option<(f32, usize, u32)> = None;
            for i in 0..tokens.len().saturating_sub(1) {
                pair.clear();
                pair.extend_from_slice(self.piece(tokens[i]));
                pair.extend_from_slice(self.piece(tokens[i + 1]));
                if let Some(id) = self.lookup(&pair) {
                    let score = self.scores[id as usize];
                    if best.is_none_or(|(b, _, _)| score > b) {
                        best = Some((score, i, id));
                    }
                }
            }
            let Some((_, i, id)) = best else { break };
            tokens[i] = id;
            tokens.remove(i + 1);
        }
        tokens
    }
}

/// `<0xAB>` -> 0xAB
fn raw_byte(piece: &[u8]) -> Option<u8> {
    let hex = piece.strip_prefix(b"<0x")?.strip_suffix(b">")?;
    if hex.len() != 2 {
        return None;
    }
    u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()
}

/// Turns a byte stream into text, holding back incomplete UTF-8 sequences.
#[derive(Default)]
pub struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    pub fn push(&mut self, bytes: &[u8], mut emit: impl FnMut(&str)) {
        self.pending.extend_from_slice(bytes);
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    if !s.is_empty() {
                        emit(s);
                    }
                    self.pending.clear();
                    return;
                }
                Err(e) => {
                    let ok = e.valid_up_to();
                    if ok > 0 {
                        emit(std::str::from_utf8(&self.pending[..ok]).unwrap());
                    }
                    match e.error_len() {
                        // Invalid bytes: replace and keep going.
                        Some(bad) => {
                            emit("\u{fffd}");
                            self.pending.drain(..ok + bad);
                        }
                        // Incomplete sequence at the end: wait for more.
                        None => {
                            self.pending.drain(..ok);
                            return;
                        }
                    }
                }
            }
        }
    }
}
