//! Host-side tools for the TinyStories LLM:
//!
//!   llm quantize stories15M.bin stories15M.q8
//!   llm run stories15M.q8 tokenizer.bin ["Once upon a time"] [threads] [temperature] [seed]
use std::io::Write;
use std::time::Instant;

use m5_common::llm::{self, Sampler, Tokenizer, Transformer, Weights};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let read = |p: &str| std::fs::read(p).map_err(|e| format!("{p}: {e}"));
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["quantize", src, dst] => {
            let out = llm::convert_llama2c(&read(src)?)?;
            std::fs::write(dst, &out).map_err(|e| format!("{dst}: {e}"))?;
            println!("wrote {dst} ({} bytes)", out.len());
        }
        ["run", model, tokenizer, rest @ ..] => {
            let mut t = Transformer::new(Weights::from_bytes(&read(model)?)?);
            t.set_threads(rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(1));
            let tok = Tokenizer::from_bytes(&read(tokenizer)?, t.config().vocab_size)?;
            let temperature = rest.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let seed = rest.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
            let mut sampler = Sampler::new(temperature, 0.9, seed);
            let start = Instant::now();
            let n = llm::generate(&mut t, &tok, &mut sampler, rest.first().copied().unwrap_or(""), 256, |s| {
                print!("{s}");
                std::io::stdout().flush().ok();
                true
            });
            let secs = start.elapsed().as_secs_f32();
            println!("\n{n} tokens in {secs:.2}s ({:.1} tok/s)", n as f32 / secs);
        }
        _ => return Err("usage: llm quantize <in.bin> <out.q8> | llm run <model.q8> <tokenizer.bin> [prompt] [threads] [temperature] [seed]".into()),
    }
    Ok(())
}
