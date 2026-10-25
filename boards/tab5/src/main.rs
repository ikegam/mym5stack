mod display;
mod net;
mod sdcard;
mod touch;

use std::io::{Read, Write};
use std::ops::Range;
use std::time::{Duration, Instant};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys;
use m5_common::llm::{self, Sampler, Tokenizer, Transformer, Weights};
use m5_common::japan_map::{self, Pick, Region};
use m5_common::tab5_ui::{self, Scene, StoryText, SCENE};
use m5_common::weather::{self, Forecast};
use m5_common::{BoardInfo, Display};

use display::{Lcd, Offscreen};
use touch::Touch;

const BOARD: BoardInfo = BoardInfo {
    name: "M5Stack Tab5",
    mcu: "ESP32-P4",
    display: Display { width: 720, height: 1280, kind: "MIPI-DSI LCD" },
};

const WIFI_SSID: Option<&str> = option_env!("WIFI_SSID");
const WIFI_PASS: &str = match option_env!("WIFI_PASS") {
    Some(p) => p,
    None => "",
};

/// A model and tokenizer pair in the root of the microSD card (see README),
/// with how to open a story in its language.
struct Model {
    path: &'static str,
    tokenizer: &'static str,
    prompt: fn(&Forecast) -> String,
    fallback_prompt: &'static str,
}

/// Japanese is used when its files are on the card.
const MODELS: [Model; 2] = [
    Model {
        path: "/sd/stories15M-ja.q8",
        tokenizer: "/sd/tokenizer-ja.bin",
        prompt: Forecast::story_prompt_ja,
        fallback_prompt: "むかしむかし、",
    },
    Model {
        path: "/sd/stories15M.q8",
        tokenizer: "/sd/tokenizer.bin",
        prompt: Forecast::story_prompt,
        fallback_prompt: "Once upon a time",
    },
];
const STEPS: usize = 256;
const TEMPERATURE: f32 = 0.8;
const TOPP: f32 = 0.9;
const FORECAST_EVERY: Duration = Duration::from_secs(60 * 60);
/// Animation time between stories. Earlier stories stay in the scrollback,
/// so reading never holds up the next one.
const PAUSE: Duration = Duration::from_secs(20);
/// The forecast region picked on the map, by name.
const REGION_FILE: &str = "/sd/region.txt";
/// The map closes by itself if left alone this long.
const PICK_TIMEOUT: Duration = Duration::from_secs(60);

fn main() -> anyhow::Result<()> {
    sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("{}", m5_common::banner(&BOARD));

    let mut lcd = Lcd::new()?;
    if WIFI_SSID.is_some_and(|s| !s.is_empty()) {
        net::power_on()?;
    }
    let mut scene = Scene::new(None);
    let mut scene_buf = Offscreen::new(SCENE);
    tab5_ui::draw_background(&mut lcd);
    let mut region = japan_map::default_region();
    tab5_ui::draw_header(&mut lcd, None, region.name);
    scene.draw(&mut scene_buf, 0.0);
    lcd.blit(&scene_buf);
    tab5_ui::draw_forecast(&mut lcd, None, "じゅんびちゅう…");
    lcd.flush();
    lcd.brightness(80)?;
    let touch = Touch::start().map_err(|e| log::error!("touch: {e:?}")).ok();

    let p = Peripherals::take()?;
    // WiFi first: esp_hosted wants to bring up the SDMMC host itself before
    // the microSD slot uses it. WiFi is optional; without it stories just
    // aren't about today's weather.
    let wifi = match WIFI_SSID.filter(|s| !s.is_empty()) {
        Some(ssid) => {
            show_status(&mut lcd, &format!("connecting to {ssid}..."));
            let sysloop = EspSystemEventLoop::take()?;
            let nvs = EspDefaultNvsPartition::take()?;
            net::connect(p.modem, sysloop, nvs, ssid, WIFI_PASS).map_err(|e| log::error!("wifi: {e:?}")).ok()
        }
        None => {
            log::warn!("WIFI_SSID not set; no forecast");
            None
        }
    };

    show_status(&mut lcd, "loading model from microSD...");
    let _sd = sdcard::mount(p.sdmmc0, p.pins)?;
    if let Some(r) = std::fs::read_to_string(REGION_FILE).ok().and_then(|s| japan_map::find(s.trim())) {
        region = r;
    }
    log::info!("region: {} ({})", region.name, region.office);
    let model = MODELS
        .iter()
        .find(|m| std::fs::metadata(m.path).is_ok())
        .ok_or_else(|| anyhow::anyhow!("no model on the microSD card"))?;
    let weights = load_weights(model.path)?;
    log::info!("bench: {}", llm::bench(&weights));
    let mut t = Transformer::new(weights);
    let tok = Tokenizer::from_bytes(&std::fs::read(model.tokenizer)?, t.config().vocab_size)
        .map_err(anyhow::Error::msg)?;
    log::info!("model: {:?}", t.config());
    set_threads(&mut t, 2)?;
    log_heap("after model load");

    let mut sampler = Sampler::new(TEMPERATURE, TOPP, unsafe { sys::esp_random() } as u64);
    let mut forecast: Option<Forecast> = None;
    let mut fetched: Option<Instant> = None;
    let mut story = StoryText::default();
    let clock = Instant::now();
    let mut backdrop = Backdrop::default();
    loop {
        if wifi.is_some() && fetched.is_none_or(|t| t.elapsed() >= FORECAST_EVERY) {
            show_status(&mut lcd, "fetching the weather forecast...");
            match fetch_forecast(region) {
                Ok(f) => {
                    log::info!("forecast: {f:?}");
                    forecast = Some(f);
                    fetched = Some(Instant::now());
                }
                Err(e) => log::error!("forecast: {e:?}"),
            }
        }
        let note = if wifi.is_none() { "Wi-Fi に つながっていません" } else { "てんきよほうを とれませんでした" };
        scene = Scene::new(forecast.as_ref());
        let header = tab5_ui::draw_header(&mut lcd, forecast.as_ref(), region.name);
        flush(&mut lcd, header);
        let info = tab5_ui::draw_forecast(&mut lcd, forecast.as_ref(), note);
        flush(&mut lcd, info);
        let r = story.next_story(&mut lcd);
        flush(&mut lcd, r);
        show_status(&mut lcd, "writing a story...");

        let prompt = forecast.as_ref().map_or(model.fallback_prompt.to_string(), model.prompt);
        let start = Instant::now();
        let (mut ui_text, mut ui_scene, mut ui_blit, mut ui_wait) = (0f32, 0f32, 0f32, 0f32);
        // The region button was tapped: stop writing and open the map.
        let mut pick = false;
        let n = llm::generate(&mut t, &tok, &mut sampler, &prompt, STEPS, |s| {
            let t0 = Instant::now();
            print!("{s}");
            std::io::stdout().flush().ok();
            let r = story.push(&mut lcd, s);
            flush(&mut lcd, r);
            scroll(&mut lcd, &mut story, touch.as_ref());
            let t1 = Instant::now();
            backdrop.draw(&scene, &mut scene_buf, clock.elapsed().as_secs_f32());
            let t2 = Instant::now();
            lcd.blit(&scene_buf);
            let t3 = Instant::now();
            // Let the idle tasks run so the task watchdog stays quiet.
            FreeRtos::delay_ms(1);
            ui_text += (t1 - t0).as_secs_f32();
            ui_scene += (t2 - t1).as_secs_f32();
            ui_blit += (t3 - t2).as_secs_f32();
            ui_wait += t3.elapsed().as_secs_f32();
            pick = region_tapped(touch.as_ref());
            !pick
        });
        let secs = start.elapsed().as_secs_f32();
        println!();
        let per = |x: f32| x * 1000.0 / n.max(1) as f32;
        log::info!(
            "per token: total {:.0} ms, text {:.1}, scene {:.1}, blit {:.1}, wait {:.1}",
            per(secs),
            per(ui_text),
            per(ui_scene),
            per(ui_blit),
            per(ui_wait)
        );
        let stats = format!("{n} tokens in {secs:.1}s ({:.2} tok/s)", n as f32 / secs);
        log::info!("{stats}");
        show_status(&mut lcd, &stats);
        log_heap("after story");

        let pause = Instant::now();
        while !pick && pause.elapsed() < PAUSE {
            scroll(&mut lcd, &mut story, touch.as_ref());
            backdrop.draw(&scene, &mut scene_buf, clock.elapsed().as_secs_f32());
            lcd.blit(&scene_buf);
            FreeRtos::delay_ms(50);
            pick = region_tapped(touch.as_ref());
        }
        if let Some(touch) = touch.as_ref().filter(|_| pick) {
            if let Some(r) = pick_region(&mut lcd, touch, region).filter(|&r| r != region) {
                log::info!("region: {} ({})", r.name, r.office);
                region = r;
                std::fs::write(REGION_FILE, r.name).map_err(|e| log::error!("{REGION_FILE}: {e}")).ok();
                forecast = None;
                fetched = None;
            }
            tab5_ui::draw_background(&mut lcd);
            story.redraw(&mut lcd);
            lcd.flush();
        }
    }
}

fn region_tapped(touch: Option<&Touch>) -> bool {
    touch.and_then(Touch::take_tap).is_some_and(|(x, y)| tab5_ui::hits_region_button(x, y))
}

/// Shows the map until a region (or "back") is tapped.
fn pick_region(lcd: &mut Lcd, touch: &Touch, current: &'static Region) -> Option<&'static Region> {
    japan_map::draw(lcd, Some(current));
    lcd.flush();
    let map = japan_map::decode();
    let shown = Instant::now();
    while shown.elapsed() < PICK_TIMEOUT {
        touch.take_lines();
        match touch.take_tap().map(|(x, y)| japan_map::hit(&map, x, y)) {
            Some(Pick::Region(r)) => {
                // Show the choice for a moment before leaving.
                japan_map::draw(lcd, Some(r));
                lcd.flush();
                FreeRtos::delay_ms(400);
                return Some(r);
            }
            Some(Pick::Back) => return None,
            _ => FreeRtos::delay_ms(20),
        }
    }
    None
}

/// The scene's static background, drawn once per scene and restored each
/// frame instead of redrawn (it is most of the drawing time).
#[derive(Default)]
struct Backdrop {
    saved: Option<(Scene, Vec<u16>)>,
}

impl Backdrop {
    fn draw(&mut self, scene: &Scene, buf: &mut Offscreen, t: f32) {
        if scene.is_flash(t) {
            scene.draw(buf, t);
            return;
        }
        match &self.saved {
            Some((s, bg)) if s == scene => buf.restore(bg),
            _ => {
                scene.draw_background(buf, false);
                self.saved = Some((*scene, buf.snapshot()));
            }
        }
        scene.draw_foreground(buf, t);
    }
}

/// Applies any swipe since the last call to the story text.
fn scroll(lcd: &mut Lcd, story: &mut StoryText, touch: Option<&Touch>) {
    let lines = touch.map_or(0, Touch::take_lines);
    if lines != 0 {
        let r = story.scroll(lcd, lines);
        flush(lcd, r);
    }
}

fn flush(lcd: &mut Lcd, rows: Range<i32>) {
    lcd.flush_rows(rows.start, rows.end);
}

fn show_status(lcd: &mut Lcd, text: &str) {
    let r = tab5_ui::draw_status(lcd, text);
    flush(lcd, r);
}

fn fetch_forecast(region: &Region) -> anyhow::Result<Forecast> {
    let body = net::get(&weather::url(region.office))?;
    weather::parse_jma(std::str::from_utf8(&body)?).map_err(anyhow::Error::msg)
}

/// Starts the matmul helper pinned to core 1; unpinned, it tends to preempt
/// the main task on core 0 instead of running alongside it.
fn set_threads(t: &mut Transformer, n: usize) -> anyhow::Result<()> {
    unsafe {
        let mut cfg = sys::esp_pthread_get_default_config();
        cfg.pin_to_core = 1;
        sys::esp!(sys::esp_pthread_set_cfg(&cfg))?;
        t.set_threads(n);
        sys::esp!(sys::esp_pthread_set_cfg(&sys::esp_pthread_get_default_config()))?;
    }
    Ok(())
}

/// Reads the whole model file into one PSRAM buffer.
fn load_weights(path: &str) -> anyhow::Result<Weights> {
    let start = Instant::now();
    let mut f = std::fs::File::open(path).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    let len = f.metadata()?.len() as usize;
    let mut header = [0u8; llm::HEADER_LEN];
    f.read_exact(&mut header)?;
    llm::read_header(&header).map_err(anyhow::Error::msg)?;

    let mut buf = Weights::alloc(len);
    let bytes = &mut llm::as_bytes_mut(&mut buf)[..len];
    bytes[..header.len()].copy_from_slice(&header);
    let mut done = header.len();
    let mut next_log = 0;
    while done < len {
        let n = f.read(&mut bytes[done..(done + 256 * 1024).min(len)])?;
        anyhow::ensure!(n > 0, "{path}: unexpected end of file");
        done += n;
        if done * 10 / len >= next_log {
            log::info!("loading {path}: {}%", done * 100 / len);
            next_log = done * 10 / len + 1;
        }
    }
    let secs = start.elapsed().as_secs_f32();
    log::info!("loaded {len} bytes in {secs:.1}s ({:.1} MB/s)", len as f32 / secs / 1e6);
    Weights::from_buf(buf, len).map_err(anyhow::Error::msg)
}

fn log_heap(when: &str) {
    let (free, largest) = unsafe {
        (
            sys::heap_caps_get_free_size(sys::MALLOC_CAP_SPIRAM),
            sys::heap_caps_get_largest_free_block(sys::MALLOC_CAP_SPIRAM),
        )
    };
    let internal = unsafe { sys::heap_caps_get_free_size(sys::MALLOC_CAP_INTERNAL) };
    log::info!("heap {when}: psram free={free} largest={largest}, internal free={internal}");
}
