//! Lists characters on stdin that the Tab5's Japanese font can't draw.
use std::collections::BTreeMap;
use std::io::Read;

use u8g2_fonts::{fonts, FontRenderer};

fn main() {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).unwrap();
    let font = FontRenderer::new::<fonts::u8g2_font_b16_t_japanese3>();
    let mut missing = BTreeMap::new();
    let mut total = 0;
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        total += 1;
        if font.get_rendered_dimensions(c, Default::default(), Default::default()).is_err() {
            *missing.entry(c).or_insert(0) += 1;
        }
    }
    let n: usize = missing.values().sum();
    println!("{n} of {total} characters missing ({} distinct)", missing.len());
    let mut v: Vec<_> = missing.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    println!("{}", v.iter().map(|(c, n)| format!("{c}{n}")).collect::<Vec<_>>().join(" "));
}
