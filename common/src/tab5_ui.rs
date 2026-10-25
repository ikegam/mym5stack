//! Tab5 "weather story" screen (720x1280 portrait, RGB565): an animated
//! weather scene, today's JMA forecast in Japanese, and the TinyStories text
//! as it is generated.
//!
//! Everything draws onto any `DrawTarget<Color = Rgb565>` so the layout can
//! be previewed on the host (`make preview-tab5`). Methods return the rows
//! they touched so the device only flushes those.

use std::f32::consts::TAU;
use std::ops::Range;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::geometry::Angle;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{
    Circle, Line, Polyline, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle, Sector, StrokeAlignment, Triangle,
};
use u8g2_fonts::types::{FontColor, HorizontalAlignment, VerticalPosition};
use u8g2_fonts::{fonts, FontRenderer};

use crate::weather::{Forecast, Sky};

pub const WIDTH: i32 = 720;
pub const HEIGHT: i32 = 1280;

const MARGIN: i32 = 28;
const HEADER: Range<i32> = 0..96;
pub const SCENE: Range<i32> = 96..456;
const INFO: Range<i32> = 456..676;
const STORY: Range<i32> = 700..1200;
const STATUS: Range<i32> = 1210..1280;
/// Japanese text is the 16px b16 font drawn at 2x.
const JA_SCALE: i32 = 2;
const JA_LINE_H: i32 = 16 * JA_SCALE + 8;

const BG: Rgb565 = Rgb565::new(3, 6, 10);
const INK: Rgb565 = Rgb565::new(30, 60, 28);
const ACCENT: Rgb565 = Rgb565::new(31, 45, 8);
const DIM: Rgb565 = Rgb565::new(16, 32, 18);
const RAIN_BLUE: Rgb565 = Rgb565::new(10, 40, 31);

fn story_font() -> FontRenderer {
    FontRenderer::new::<fonts::u8g2_font_helvR18_tr>()
}

fn ja_font() -> FontRenderer {
    FontRenderer::new::<fonts::u8g2_font_b16_t_japanese3>()
}

fn rows(r: &Range<i32>) -> Rectangle {
    Rectangle::new(Point::new(0, r.start), Size::new(WIDTH as u32, (r.end - r.start) as u32))
}

/// Draws through to `D` with every pixel blown up to `JA_SCALE`² pixels
/// around `origin`, so small bitmap fonts stay crisp but readable.
struct Scaled<'a, D> {
    d: &'a mut D,
    origin: Point,
}

impl<D: DrawTarget<Color = Rgb565>> OriginDimensions for Scaled<'_, D> {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32 / JA_SCALE as u32, HEIGHT as u32 / JA_SCALE as u32)
    }
}

impl<D: DrawTarget<Color = Rgb565>> DrawTarget for Scaled<'_, D> {
    type Color = Rgb565;
    type Error = D::Error;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), D::Error> {
        for Pixel(p, c) in pixels {
            let at = self.origin + p * JA_SCALE;
            self.d.fill_solid(&Rectangle::new(at, Size::new(JA_SCALE as u32, JA_SCALE as u32)), c)?;
        }
        Ok(())
    }
}

/// Japanese text at 2x, top-left at `at`. Returns its width in pixels.
pub(crate) fn ja_text<D: DrawTarget<Color = Rgb565>>(d: &mut D, s: &str, at: Point, color: Rgb565) -> i32 {
    let mut s2 = Scaled { d, origin: at };
    ja_font()
        .render(s, Point::zero(), VerticalPosition::Top, FontColor::Transparent(color), &mut s2)
        .ok()
        .and_then(|r| r.bounding_box)
        .map_or(0, |b| (b.top_left.x + b.size.width as i32) * JA_SCALE)
}

pub(crate) fn ja_width(s: &str) -> i32 {
    ja_font()
        .get_rendered_dimensions(s, Point::zero(), VerticalPosition::Top)
        .ok()
        .and_then(|r| r.bounding_box)
        .map_or(0, |b| (b.top_left.x + b.size.width as i32) * JA_SCALE)
}

/// Splits Japanese text into lines of at most `max` pixels, preferring the
/// spaces JMA puts between phrases.
fn wrap_ja(text: &str, max: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let joined = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if ja_width(&joined) <= max {
            cur = joined;
            continue;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        // A single phrase wider than a line: break it by characters.
        for ch in word.chars() {
            cur.push(ch);
            if ja_width(&cur) > max {
                cur.pop();
                lines.push(std::mem::replace(&mut cur, ch.to_string()));
            }
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Fills the whole screen with the background color.
pub fn draw_background<D: DrawTarget<Color = Rgb565>>(d: &mut D) -> Range<i32> {
    d.fill_solid(&rows(&(0..HEIGHT)), BG).ok();
    0..HEIGHT
}

pub fn draw_header<D: DrawTarget<Color = Rgb565>>(d: &mut D, f: Option<&Forecast>, region: &str) -> Range<i32> {
    draw_title(d, "きょうの おはなし");
    // The region doubles as the button that opens the region picker.
    let date = f.map_or(String::new(), |f| f.date_ja());
    let date_w = ja_width(&date) + 12;
    let w = ja_width(region);
    let button = Rectangle::new(Point::new(WIDTH - MARGIN - w - 16, 14), Size::new(w as u32 + 16, 50));
    ja_text(d, &date, Point::new(button.top_left.x - date_w, 24), DIM);
    RoundedRectangle::with_equal_corners(button, Size::new(10, 10))
        .into_styled(PrimitiveStyleBuilder::new().stroke_color(DIM).stroke_width(2).build())
        .draw(d)
        .ok();
    ja_text(d, region, button.top_left + Point::new(8, 10), INK);
    HEADER
}

/// Whether a tap at `(x, y)` hits the region button in the header.
pub fn hits_region_button(x: i32, y: i32) -> bool {
    y < HEADER.end && x > WIDTH / 2
}

/// Clears the header and writes `title` in it.
pub fn draw_title<D: DrawTarget<Color = Rgb565>>(d: &mut D, title: &str) -> Range<i32> {
    d.fill_solid(&rows(&HEADER), BG).ok();
    ja_text(d, title, Point::new(MARGIN, 24), ACCENT);
    d.fill_solid(&Rectangle::new(Point::new(MARGIN, HEADER.end - 12), Size::new((WIDTH - 2 * MARGIN) as u32, 3)), ACCENT)
        .ok();
    HEADER
}

/// The forecast panel under the scene.
pub fn draw_forecast<D: DrawTarget<Color = Rgb565>>(d: &mut D, f: Option<&Forecast>, note: &str) -> Range<i32> {
    d.fill_solid(&rows(&INFO), BG).ok();
    let mut y = INFO.start + 14;
    let Some(f) = f else {
        ja_text(d, note, Point::new(MARGIN, y), DIM);
        return INFO;
    };
    for line in wrap_ja(&f.weather, WIDTH - 2 * MARGIN).iter().take(3) {
        ja_text(d, line, Point::new(MARGIN, y), INK);
        y += JA_LINE_H;
    }
    let mut facts = Vec::new();
    if let Some(t) = f.temp_max {
        facts.push(format!("最高 {t}度"));
    }
    if let Some(t) = f.temp_min {
        facts.push(format!("最低 {t}度"));
    }
    if !f.pops.is_empty() {
        let p: Vec<String> = f.pops.iter().map(|(_, p)| p.to_string()).collect();
        facts.push(format!("降水確率 {}％", p.join("/")));
    }
    ja_text(d, &facts.join("  "), Point::new(MARGIN, INFO.end - JA_LINE_H - 8), ACCENT);
    INFO
}

pub fn draw_status<D: DrawTarget<Color = Rgb565>>(d: &mut D, text: &str) -> Range<i32> {
    d.fill_solid(&rows(&STATUS), BG).ok();
    story_font()
        .render_aligned(
            text,
            Point::new(WIDTH / 2, (STATUS.start + STATUS.end) / 2),
            VerticalPosition::Center,
            HorizontalAlignment::Center,
            FontColor::Transparent(DIM),
            d,
        )
        .ok();
    STATUS
}

/// What the scene shows, derived from the forecast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scene {
    pub sun: bool,
    pub clouds: u8,
    pub rain: bool,
    pub snow: bool,
    pub thunder: bool,
}

impl Scene {
    pub fn new(f: Option<&Forecast>) -> Self {
        let Some(f) = f else {
            return Self { sun: true, clouds: 1, rain: false, snow: false, thunder: false };
        };
        let rain = f.sky == Sky::Rain || f.rain_later();
        Self {
            sun: f.sky == Sky::Sunny || (f.sky == Sky::Cloudy && !rain),
            clouds: match f.sky {
                Sky::Sunny => 1,
                Sky::Cloudy if !rain => 3,
                _ => 5,
            },
            rain,
            snow: f.sky == Sky::Snow,
            thunder: f.thunder(),
        }
    }

    /// Draws frame at time `t` seconds.
    pub fn draw<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, t: f32) -> Range<i32> {
        self.draw_clipped(&mut d.clipped(&rows(&SCENE)), t);
        SCENE
    }

    fn draw_clipped<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, t: f32) {
        self.draw_background_clipped(d, self.is_flash(t));
        self.draw_foreground_clipped(d, t);
    }

    /// Lightning lights up the whole sky at time `t`.
    pub fn is_flash(&self, t: f32) -> bool {
        self.thunder && (t % 7.0) < 0.15
    }

    fn sky(&self, flash: bool) -> Rgb565 {
        if flash {
            Rgb565::new(28, 58, 31)
        } else if self.rain || self.snow {
            Rgb565::new(10, 22, 14)
        } else if self.clouds > 2 {
            Rgb565::new(14, 34, 22)
        } else {
            Rgb565::new(9, 40, 30)
        }
    }

    /// The parts that don't move (sky, hill, house), so a device can draw
    /// them once and only redraw `draw_foreground` per frame.
    pub fn draw_background<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, flash: bool) -> Range<i32> {
        self.draw_background_clipped(&mut d.clipped(&rows(&SCENE)), flash);
        SCENE
    }

    /// Everything that moves, over a background from `draw_background`.
    pub fn draw_foreground<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, t: f32) -> Range<i32> {
        self.draw_foreground_clipped(&mut d.clipped(&rows(&SCENE)), t);
        SCENE
    }

    fn draw_background_clipped<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, flash: bool) {
        let ground = SCENE.end - 70;
        d.fill_solid(&rows(&SCENE), self.sky(flash)).ok();
        // Hill and a house, so the weather has somewhere to happen.
        Circle::with_center(Point::new(WIDTH / 2, ground + 900), 1900)
            .into_styled(PrimitiveStyle::with_fill(if self.snow { Rgb565::new(28, 58, 30) } else { Rgb565::new(6, 38, 8) }))
            .draw(d)
            .ok();
        let house = Point::new(90, ground - 70);
        Rectangle::new(house, Size::new(110, 80)).into_styled(PrimitiveStyle::with_fill(Rgb565::new(28, 50, 18))).draw(d).ok();
        Triangle::new(house + Point::new(-15, 2), house + Point::new(55, -55), house + Point::new(125, 2))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(24, 14, 6)))
            .draw(d)
            .ok();
        Rectangle::new(house + Point::new(42, 38), Size::new(26, 42))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(14, 20, 6)))
            .draw(d)
            .ok();
    }

    fn draw_foreground_clipped<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, t: f32) {
        let top = SCENE.start;
        let h = SCENE.end - SCENE.start;
        let ground = top + h - 70;
        if self.sun {
            let c = Point::new(WIDTH - 150, top + 100);
            let ray = PrimitiveStyle::with_stroke(Rgb565::new(31, 52, 6), 6);
            for i in 0..12 {
                let a = t * 0.4 + i as f32 * TAU / 12.0;
                let p = |r: f32| c + Point::new((a.cos() * r) as i32, (a.sin() * r) as i32);
                Line::new(p(62.0), p(84.0)).into_styled(ray).draw(d).ok();
            }
            Circle::with_center(c, 100).into_styled(PrimitiveStyle::with_fill(Rgb565::new(31, 56, 4))).draw(d).ok();
        }
        self.draw_girl(d, Point::new(WIDTH / 2 + 20, ground - 10), t);

        let cloud_color = if self.rain || self.snow { Rgb565::new(18, 38, 22) } else { Rgb565::new(30, 61, 30) };
        for i in 0..self.clouds as i32 {
            let speed = 18.0 + i as f32 * 7.0;
            let span = WIDTH + 300;
            let x = ((i * 260) as f32 + t * speed) as i32 % span - 150;
            let y = top + 50 + (i * 47) % 120;
            cloud(d, Point::new(x, y), 1.0 + (i % 3) as f32 * 0.25, cloud_color);
        }

        if self.thunder && (t % 7.0) < 0.35 {
            let x = 200 + ((t / 7.0) as i32 * 157) % 300;
            let pts = [
                Point::new(x, top + 110),
                Point::new(x - 25, top + 180),
                Point::new(x + 5, top + 180),
                Point::new(x - 20, top + 260),
            ];
            Polyline::new(&pts).into_styled(PrimitiveStyle::with_stroke(Rgb565::new(31, 62, 10), 6)).draw(d).ok();
        }

        if self.rain {
            let style = PrimitiveStyle::with_stroke(RAIN_BLUE, 3);
            for i in 0..70 {
                let x = (i * 97 + 13) % WIDTH;
                let y = top + ((i * 53) as f32 + t * 420.0) as i32 % h;
                Line::new(Point::new(x, y), Point::new(x - 6, y + 22)).into_styled(style).draw(d).ok();
            }
        }
        if self.snow {
            let style = PrimitiveStyle::with_fill(Rgb565::WHITE);
            for i in 0..60 {
                let y = top + ((i * 61) as f32 + t * 60.0) as i32 % h;
                let x = (i * 113 + 29) % WIDTH + ((t * 1.3 + i as f32).sin() * 14.0) as i32;
                Circle::with_center(Point::new(x, y), 9).into_styled(style).draw(d).ok();
            }
        }
    }

    /// A girl who holds up an umbrella when it rains, and bounces otherwise.
    fn draw_girl<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, feet: Point, t: f32) {
        let bounce = if self.rain { 0 } else { ((t * 5.0).sin().abs() * 10.0) as i32 };
        let base = feet - Point::new(0, bounce);
        let skin = Rgb565::new(31, 52, 22);
        let dress = Rgb565::new(31, 20, 20);
        let line = PrimitiveStyle::with_stroke(Rgb565::new(6, 8, 6), 5);
        Line::new(base + Point::new(-10, -40), base + Point::new(-14, 0)).into_styled(line).draw(d).ok();
        Line::new(base + Point::new(10, -40), base + Point::new(14, 0)).into_styled(line).draw(d).ok();
        Triangle::new(base + Point::new(0, -100), base + Point::new(-32, -36), base + Point::new(32, -36))
            .into_styled(PrimitiveStyle::with_fill(dress))
            .draw(d)
            .ok();
        // Face and its outline share one circle so they cannot drift apart.
        let face = base + Point::new(0, -122);
        Circle::with_center(face, 44)
            .into_styled(
                PrimitiveStyleBuilder::new()
                    .fill_color(skin)
                    .stroke_color(Rgb565::new(10, 10, 4))
                    .stroke_width(5)
                    .stroke_alignment(StrokeAlignment::Inside)
                    .build(),
            )
            .draw(d)
            .ok();
        let eye = PrimitiveStyle::with_fill(Rgb565::new(6, 8, 6));
        for dx in [-8, 8] {
            Circle::with_center(face + Point::new(dx, -2), 6).into_styled(eye).draw(d).ok();
        }
        if self.rain || self.snow {
            let hand = base + Point::new(30, -80);
            Line::new(base + Point::new(10, -78), hand).into_styled(line).draw(d).ok();
            Line::new(hand, hand + Point::new(0, -90)).into_styled(line).draw(d).ok();
            let top = hand + Point::new(0, -90);
            // Canopy: the upper half of a disc centered on the shaft top.
            Sector::with_center(top + Point::new(0, 20), 150, Angle::from_degrees(180.0), Angle::from_degrees(180.0))
                .into_styled(PrimitiveStyle::with_fill(Rgb565::new(31, 12, 14)))
                .draw(d)
                .ok();
        } else {
            let wave = ((t * 4.0).sin() * 20.0) as i32;
            Line::new(base + Point::new(10, -78), base + Point::new(44, -110 + wave)).into_styled(line).draw(d).ok();
            Line::new(base + Point::new(-10, -78), base + Point::new(-40, -60)).into_styled(line).draw(d).ok();
        }
    }
}

fn cloud<D: DrawTarget<Color = Rgb565>>(d: &mut D, at: Point, scale: f32, color: Rgb565) {
    let style = PrimitiveStyle::with_fill(color);
    for (dx, dy, r) in [(0, 0, 60), (45, -25, 70), (95, 0, 60), (45, 15, 60)] {
        let p = at + Point::new((dx as f32 * scale) as i32, (dy as f32 * scale) as i32);
        Circle::with_center(p, (r as f32 * scale) as u32).into_styled(style).draw(d).ok();
    }
}

/// The story text (Japanese font, so it shows either language), wrapped and
/// kept in full so it can be scrolled back to the beginning.
#[derive(Default)]
pub struct StoryText {
    /// Finished lines.
    lines: Vec<String>,
    /// The line being written (always shown after `lines`).
    cur: String,
    /// First line shown.
    top: usize,
    /// Scrolled back by the reader: stop following new text.
    held: bool,
}

/// Characters that must not start a line (kinsoku); they hang off the end
/// of the previous one instead.
const NO_LINE_START: &str = "、。，．」』）！？ー…っゃゅょ";
const VISIBLE: usize = ((STORY.end - STORY.start) / JA_LINE_H) as usize;
/// Scrollback kept across stories; older lines are dropped.
const MAX_LINES: usize = 1000;
const SCROLLBAR_W: i32 = 6;

impl StoryText {
    pub fn clear<D: DrawTarget<Color = Rgb565>>(&mut self, d: &mut D) -> Range<i32> {
        *self = Self::default();
        d.fill_solid(&rows(&STORY), BG).ok();
        STORY
    }

    /// Lines including the one being written.
    fn len(&self) -> usize {
        self.lines.len() + 1
    }

    fn bottom_top(&self) -> usize {
        self.len().saturating_sub(VISIBLE)
    }

    /// Appends text and returns the rows that changed.
    pub fn push<D: DrawTarget<Color = Rgb565>>(&mut self, d: &mut D, text: &str) -> Range<i32> {
        let before = self.lines.len();
        for ch in text.chars() {
            if ch == '\n' {
                // Paragraph breaks come as blank lines; one line break is enough.
                if !self.cur.is_empty() {
                    self.lines.push(std::mem::take(&mut self.cur));
                }
                continue;
            }
            let ch = if ja_has_glyph(ch) { ch } else { '・' };
            if self.cur.is_empty() && ch == ' ' {
                continue;
            }
            self.cur.push(ch);
            if ja_width(&self.cur) > WIDTH - 2 * MARGIN - SCROLLBAR_W && !NO_LINE_START.contains(ch) {
                // English: carry the unfinished word over; Japanese: just this character.
                let split = match self.cur.rfind(' ') {
                    Some(i) if ch.is_ascii_alphanumeric() => i + 1,
                    _ => self.cur.len() - ch.len_utf8(),
                };
                let rest = self.cur.split_off(split);
                let done = self.cur.trim_end().to_string();
                self.lines.push(done);
                self.cur = rest.trim_start().to_string();
            }
        }
        if self.lines.len() > MAX_LINES {
            let drop = self.lines.len() - MAX_LINES;
            self.lines.drain(..drop);
            self.top = if self.held { self.top.saturating_sub(drop) } else { self.bottom_top() };
            return self.draw(d);
        }
        if self.held {
            // Reader is looking back; only the scrollbar moves.
            return self.draw_scrollbar(d);
        }
        let top = self.bottom_top();
        if top != self.top {
            self.top = top;
            return self.draw(d);
        }
        // Redraw from the first line that changed down to the current one.
        let first = before.max(self.top);
        let r = self.row(first).start..self.row(self.len() - 1).end;
        for i in first..self.len() {
            self.draw_line(d, i);
        }
        union(r, self.draw_scrollbar(d))
    }

    /// Starts a new story below the previous ones, after a divider line.
    pub fn next_story<D: DrawTarget<Color = Rgb565>>(&mut self, d: &mut D) -> Range<i32> {
        if self.lines.is_empty() && self.cur.is_empty() {
            return 0..0;
        }
        self.push(d, "\n＊　＊　＊\n")
    }

    /// Scrolls by `lines` (positive: later text) and returns the rows to show.
    pub fn scroll<D: DrawTarget<Color = Rgb565>>(&mut self, d: &mut D, lines: i32) -> Range<i32> {
        let top = (self.top as i64 + lines as i64).clamp(0, self.bottom_top() as i64) as usize;
        if top == self.top {
            return 0..0;
        }
        self.top = top;
        // Back at the end: follow new text again.
        self.held = top < self.bottom_top();
        self.draw(d)
    }

    /// Draws the visible lines again, e.g. after something covered them.
    pub fn redraw<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) -> Range<i32> {
        self.draw(d)
    }

    fn row(&self, line: usize) -> Range<i32> {
        let y = STORY.start + (line as i32 - self.top as i32) * JA_LINE_H;
        y..y + JA_LINE_H
    }

    fn line(&self, i: usize) -> &str {
        self.lines.get(i).map_or(self.cur.as_str(), String::as_str)
    }

    fn draw_line<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, i: usize) {
        let r = self.row(i);
        d.fill_solid(&Rectangle::new(Point::new(0, r.start), Size::new((WIDTH - SCROLLBAR_W - 4) as u32, JA_LINE_H as u32)), BG)
            .ok();
        ja_text(d, self.line(i), Point::new(MARGIN, r.start + 4), INK);
    }

    fn draw<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) -> Range<i32> {
        d.fill_solid(&rows(&STORY), BG).ok();
        for i in self.top..self.len().min(self.top + VISIBLE) {
            self.draw_line(d, i);
        }
        self.draw_scrollbar(d);
        STORY
    }

    fn draw_scrollbar<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) -> Range<i32> {
        let x = WIDTH - SCROLLBAR_W - 2;
        let h = STORY.end - STORY.start;
        d.fill_solid(&Rectangle::new(Point::new(x, STORY.start), Size::new(SCROLLBAR_W as u32, h as u32)), BG).ok();
        if self.len() > VISIBLE {
            let n = self.len() as i32;
            let y0 = STORY.start + h * self.top as i32 / n;
            let y1 = STORY.start + h * (self.top + VISIBLE).min(self.len()) as i32 / n;
            d.fill_solid(&Rectangle::new(Point::new(x, y0), Size::new(SCROLLBAR_W as u32, (y1 - y0).max(8) as u32)), DIM)
                .ok();
        }
        STORY
    }
}

/// Turns a finger dragging over the screen into whole lines of scrolling.
#[derive(Default)]
pub struct Swipe {
    last_y: Option<i32>,
    acc: i32,
}

impl Swipe {
    /// Feeds the current touch point (or none). Returns lines to scroll:
    /// dragging up moves the text up, i.e. towards later lines.
    pub fn update(&mut self, touch: Option<(i32, i32)>) -> i32 {
        let Some((_, y)) = touch else {
            *self = Self::default();
            return 0;
        };
        if let Some(last) = self.last_y.replace(y) {
            self.acc += last - y;
        }
        let lines = self.acc / JA_LINE_H;
        self.acc -= lines * JA_LINE_H;
        lines
    }
}

/// Turns touches into taps: a press released without moving far.
#[derive(Default)]
pub struct Tap {
    start: Option<(i32, i32)>,
    moved: bool,
}

impl Tap {
    const SLOP: i32 = 24;

    /// Feeds the current touch point (or none); returns a finished tap.
    pub fn update(&mut self, touch: Option<(i32, i32)>) -> Option<(i32, i32)> {
        match touch {
            Some(p) => {
                let s = *self.start.get_or_insert(p);
                self.moved |= (p.0 - s.0).abs() > Self::SLOP || (p.1 - s.1).abs() > Self::SLOP;
                None
            }
            None => {
                let tap = self.start.filter(|_| !self.moved);
                *self = Self::default();
                tap
            }
        }
    }
}

fn ja_has_glyph(c: char) -> bool {
    ja_font().get_rendered_dimensions(c, Point::zero(), VerticalPosition::Top).is_ok()
}

fn union(a: Range<i32>, b: Range<i32>) -> Range<i32> {
    a.start.min(b.start)..a.end.max(b.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    /// Records nothing; enough to drive layout code.
    struct Null;
    impl OriginDimensions for Null {
        fn size(&self) -> Size {
            Size::new(WIDTH as u32, HEIGHT as u32)
        }
    }
    impl DrawTarget for Null {
        type Color = Rgb565;
        type Error = Infallible;
        fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, _: I) -> Result<(), Infallible> {
            Ok(())
        }
    }

    #[test]
    fn story_wraps_japanese_with_kinsoku() {
        let mut s = StoryText::default();
        let mut d = Null;
        // 20 kanji fill a line exactly; the following 。 must hang, not wrap.
        s.push(&mut d, &format!("{}。次", "雨".repeat(20)));
        assert_eq!(s.lines.len(), 1);
        assert_eq!(s.cur, "次");
        // Blank lines between paragraphs collapse into one line break.
        s.push(&mut d, "\n\nあ");
        assert_eq!((s.lines.len(), s.cur.as_str()), (2, "あ"));
    }

    #[test]
    fn scrolls_back_and_resumes_following() {
        let mut s = StoryText::default();
        let mut d = Null;
        for i in 0..30 {
            s.push(&mut d, &format!("{i}行目\n"));
        }
        assert_eq!(s.top, s.bottom_top());
        assert!(s.top > 0);
        s.scroll(&mut d, -100);
        assert_eq!((s.top, s.held), (0, true));
        // New text doesn't yank the reader away from the first page.
        s.push(&mut d, "続き\n");
        assert_eq!(s.top, 0);
        s.scroll(&mut d, 100);
        assert_eq!(s.top, s.bottom_top());
        assert!(!s.held);
        s.push(&mut d, "もっと\n");
        assert_eq!(s.top, s.bottom_top());
    }

    #[test]
    fn keeps_earlier_stories_up_to_a_limit() {
        let mut s = StoryText::default();
        let mut d = Null;
        s.push(&mut d, "一つ目");
        s.next_story(&mut d);
        s.push(&mut d, "二つ目");
        assert_eq!(s.lines, vec!["一つ目", "＊　＊　＊"]);
        for _ in 0..MAX_LINES {
            s.push(&mut d, "行\n");
        }
        assert_eq!(s.lines.len(), MAX_LINES);
        assert_eq!(s.top, s.bottom_top());
    }

    #[test]
    fn swipe_counts_whole_lines() {
        let mut w = Swipe::default();
        assert_eq!(w.update(Some((0, 500))), 0);
        assert_eq!(w.update(Some((0, 500 - JA_LINE_H - 5))), 1);
        // 5px left over plus 15px more: still under a line.
        assert_eq!(w.update(Some((0, 440))), 0);
        assert_eq!(w.update(Some((0, 540))), -2);
        assert_eq!(w.update(None), 0);
        assert_eq!(w.update(Some((0, 100))), 0);
    }

    #[test]
    fn tap_is_press_and_release_in_place() {
        let mut t = Tap::default();
        assert_eq!(t.update(Some((600, 40))), None);
        assert_eq!(t.update(Some((605, 45))), None);
        assert_eq!(t.update(None), Some((600, 40)));
        assert_eq!(t.update(None), None);
        // A drag is not a tap.
        t.update(Some((300, 900)));
        t.update(Some((300, 800)));
        assert_eq!(t.update(None), None);
    }

    /// Records which lines were drawn, by y.
    struct Lines(Vec<i32>);
    impl OriginDimensions for Lines {
        fn size(&self) -> Size {
            Size::new(WIDTH as u32, HEIGHT as u32)
        }
    }
    impl DrawTarget for Lines {
        type Color = Rgb565;
        type Error = Infallible;
        fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, px: I) -> Result<(), Infallible> {
            for Pixel(p, c) in px {
                if c == INK && !self.0.contains(&(p.y / JA_LINE_H)) {
                    self.0.push(p.y / JA_LINE_H);
                }
            }
            Ok(())
        }
    }

    #[test]
    fn text_before_a_newline_is_drawn() {
        let mut s = StoryText::default();
        let mut d = Lines(Vec::new());
        // Fill a line, wrap one character, then end the paragraph.
        s.push(&mut d, &format!("{}た。\n\n次", "雨".repeat(20)));
        let rows: Vec<i32> = (0..3).map(|l| (STORY.start + l * JA_LINE_H + 8) / JA_LINE_H).collect();
        assert_eq!(s.lines.len(), 2);
        for r in rows {
            assert!(d.0.contains(&r), "line at row {r} never drawn: {:?}", d.0);
        }
    }

    #[test]
    fn wraps_japanese_at_phrase_boundaries() {
        let lines = wrap_ja("くもり 昼過ぎ から 雨 所により 昼過ぎ から 夜のはじめ頃 雷 を伴う", WIDTH - 2 * MARGIN);
        assert!(lines.len() >= 2);
        assert!(lines.iter().all(|l| ja_width(l) <= WIDTH - 2 * MARGIN));
        assert!(lines[0].starts_with("くもり 昼過ぎ"));
    }
}
