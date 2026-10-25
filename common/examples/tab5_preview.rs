//! Render the Tab5 weather-story screen to a PPM: `make preview-tab5`
//!
//!   tab5_preview out.ppm [forecast.json] [t_seconds] [weather_code]
//!   tab5_preview out.ppm --map [region]
use std::convert::Infallible;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use m5_common::japan_map;
use m5_common::tab5_ui::{self, Scene, StoryText, HEIGHT, WIDTH};
use m5_common::weather::{self, Sky};

struct Fb(Vec<Rgb565>);

impl OriginDimensions for Fb {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for Fb {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), Infallible> {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && p.x < WIDTH && p.y < HEIGHT {
                self.0[(p.y * WIDTH + p.x) as usize] = c;
            }
        }
        Ok(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().map_or("tab5_preview.ppm", String::as_str);
    let mut fb = Fb(vec![Rgb565::BLACK; (WIDTH * HEIGHT) as usize]);
    if args.get(1).is_some_and(|a| a == "--map") {
        japan_map::draw(&mut fb, japan_map::find(args.get(2).map_or("関東", String::as_str)));
        return write(out, &fb);
    }
    let json = std::fs::read_to_string(args.get(1).map_or("testdata/jma_130000.json", String::as_str)).unwrap();
    let t: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.1);
    let mut f = weather::parse_jma(&json).unwrap();
    if let Some(code) = args.get(3).and_then(|s| s.parse().ok()) {
        f.code = code;
        f.sky = Sky::from_code(code);
        f.weather = match f.sky {
            Sky::Sunny => "晴れ".into(),
            Sky::Cloudy => "くもり".into(),
            Sky::Rain => "雨".into(),
            Sky::Snow => "雪 所により 明け方 まで 雨".into(),
        };
    }

    tab5_ui::draw_header(&mut fb, Some(&f), "関東");
    Scene::new(Some(&f)).draw(&mut fb, t);
    tab5_ui::draw_forecast(&mut fb, Some(&f), "");
    let mut story = StoryText::default();
    story.clear(&mut fb);
    story.push(
        &mut fb,
        &format!(
            "{}子供たちは雨の中で遊びたがりましたが、お母さんは危なすぎるからダメだと言いました。\n\n\
             子供たちはため息をつきました。どうしても外に出たかったのですが、お母さんは「だめよ」と言いました。",
            f.story_prompt_ja()
        ),
    );
    tab5_ui::draw_status(&mut fb, "178 tokens in 35.1s (5.07 tok/s)");
    write(out, &fb);
}

fn write(out: &str, fb: &Fb) {

    let mut ppm = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    for c in &fb.0 {
        ppm.extend([c.r() << 3 | c.r() >> 2, c.g() << 2 | c.g() >> 4, c.b() << 3 | c.b() >> 2]);
    }
    std::fs::write(out, ppm).unwrap();
    println!("wrote {out}");
}
