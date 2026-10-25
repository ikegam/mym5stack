//! Map of Japan split into forecast regions by climate, for picking which
//! JMA forecast the Tab5 shows.
//!
//! The map itself is a region-id bitmap rasterized on the host from
//! prefecture outlines (`cargo run --example japan_map_gen`) and embedded
//! run-length encoded. Outlines: 地球地図日本 (GSI), via dataofjapan/land.

use std::ops::Range;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyleBuilder, Rectangle, RoundedRectangle};

use crate::tab5_ui::{self, HEIGHT, WIDTH};

/// A region of the picker and the JMA forecast office that covers it.
#[derive(Debug, PartialEq)]
pub struct Region {
    pub name: &'static str,
    /// JMA office code for `weather::url`.
    pub office: &'static str,
    /// Where the label goes, as (longitude, latitude); may be out at sea.
    label: (f32, f32),
}

const fn r(name: &'static str, office: &'static str, lon: f32, lat: f32) -> Region {
    Region { name, office, label: (lon, lat) }
}

/// Map pixels hold `index + 1`; 0 is sea and [`EDGE`] a coast or border.
pub const REGIONS: [Region; 17] = [
    r("道北", "012000", 141.9, 44.75),
    r("道東", "014100", 144.0, 43.45),
    r("道央・道南", "016000", 139.2, 42.9),
    r("東北日本海側", "050000", 137.3, 40.6),
    r("東北太平洋側", "040000", 143.2, 39.4),
    r("北陸", "170000", 136.4, 37.6),
    r("関東", "130000", 141.4, 35.6),
    r("甲信", "200000", 138.2, 36.1),
    r("東海", "230000", 137.6, 34.15),
    r("近畿", "270000", 136.25, 33.3),
    r("山陰", "320000", 132.5, 36.0),
    r("山陽", "340000", 133.4, 35.0),
    r("四国北部", "370000", 133.0, 33.95),
    r("四国南部", "390000", 133.45, 32.75),
    r("九州北部", "400000", 130.0, 34.85),
    r("九州南部", "460100", 129.3, 31.2),
    r("沖縄", "471000", 0.0, 0.0),
];
pub const EDGE: u8 = 255;

/// Region `name`, if there is one.
pub fn find(name: &str) -> Option<&'static Region> {
    REGIONS.iter().find(|r| r.name == name)
}

pub fn default_region() -> &'static Region {
    find("関東").unwrap()
}

/// The region a prefecture (JIS code) is in. Hokkaido is split by rough
/// lines along the Hidaka mountains and around 43.6°N.
pub fn region_of(pref: u32, lon: f64, lat: f64) -> Option<u8> {
    let name = match pref {
        1 if lon > 142.75 + (43.6 - lat).max(0.0) * 0.3 => "道東",
        1 if lat >= 43.6 => "道北",
        1 => "道央・道南",
        2 | 5 | 6 => "東北日本海側",
        3 | 4 | 7 => "東北太平洋側",
        15..=18 => "北陸",
        8..=14 => "関東",
        19 | 20 => "甲信",
        21..=24 => "東海",
        25..=30 => "近畿",
        31 | 32 => "山陰",
        33..=35 => "山陽",
        37 | 38 => "四国北部",
        36 | 39 => "四国南部",
        40 | 41 | 42 | 44 => "九州北部",
        43 | 45 | 46 => "九州南部",
        47 => "沖縄",
        _ => return None,
    };
    REGIONS.iter().position(|r| r.name == name).map(|i| i as u8 + 1)
}

/// Where the map sits on screen.
pub const MAP: Range<i32> = 110..900;
/// Map bounds; outlying islands beyond them (and south of the map) are
/// left out.
const WEST: f64 = 128.6;
const EAST: f64 = 145.9;
const NORTH: f64 = 45.6;
const COS_LAT: f64 = 0.7934; // cos 37.5°: keeps shapes right mid-map
const SCALE: f64 = (WIDTH as f64 - 20.0) / ((EAST - WEST) * COS_LAT);
/// Okinawa Island is far to the southwest, so it gets an enlarged inset in
/// the empty Sea of Japan corner instead.
const INSET: Rectangle = Rectangle::new(Point::new(14, 14), Size::new(150, 180));
const INSET_WEST: f64 = 127.35;
const INSET_NORTH: f64 = 26.95;
const INSET_ZOOM: f64 = 2.6;

/// Map pixel (relative to the top-left of [`MAP`]) for a point on land.
pub fn project(lon: f64, lat: f64) -> (f64, f64) {
    (10.0 + (lon - WEST) * COS_LAT * SCALE, (NORTH - lat) * SCALE)
}

pub fn unproject(x: f64, y: f64) -> (f64, f64) {
    (WEST + (x - 10.0) / (COS_LAT * SCALE), NORTH - y / SCALE)
}

/// Like [`project`] for the Okinawa inset, plus the box to clip it to.
pub fn project_inset(lon: f64, lat: f64) -> (f64, f64) {
    let k = SCALE * INSET_ZOOM;
    (INSET.top_left.x as f64 + 10.0 + (lon - INSET_WEST) * COS_LAT * k, INSET.top_left.y as f64 + 10.0 + (INSET_NORTH - lat) * k)
}

pub fn inset() -> Rectangle {
    INSET
}

pub fn map_size() -> Size {
    Size::new(WIDTH as u32, (MAP.end - MAP.start) as u32)
}

static RLE: &[u8] = include_bytes!("japan_map.bin");

/// The map as one region id per pixel, row by row.
pub fn decode() -> Vec<u8> {
    let mut out = Vec::with_capacity(map_size().width as usize * map_size().height as usize);
    for run in RLE.chunks_exact(2) {
        out.extend(std::iter::repeat_n(run[0], run[1] as usize));
    }
    out
}

const SEA: Rgb565 = Rgb565::new(3, 6, 10);
const LAND: [Rgb565; 4] = [Rgb565::new(10, 34, 12), Rgb565::new(12, 40, 14), Rgb565::new(8, 30, 14), Rgb565::new(14, 38, 10)];
const PICKED: Rgb565 = Rgb565::new(31, 45, 8);
const EDGE_COLOR: Rgb565 = Rgb565::new(24, 50, 24);
const TEXT: Rgb565 = Rgb565::new(30, 60, 28);
const SHADOW: Rgb565 = Rgb565::new(0, 0, 0);

/// The picker: the whole screen, with `picked` highlighted.
pub fn draw<D: DrawTarget<Color = Rgb565>>(d: &mut D, picked: Option<&Region>) -> Range<i32> {
    tab5_ui::draw_background(d);
    tab5_ui::draw_title(d, "ちいきを えらんでね");
    let picked_id = picked.and_then(|p| REGIONS.iter().position(|r| r == p)).map(|i| i as u8 + 1);
    let w = map_size().width as i32;
    let (mut x, mut y) = (0, MAP.start);
    for run in RLE.chunks_exact(2) {
        let (id, n) = (run[0], run[1] as i32);
        let color = match id {
            0 => SEA,
            EDGE => EDGE_COLOR,
            id if Some(id) == picked_id => PICKED,
            id => LAND[id as usize % LAND.len()],
        };
        if id != 0 {
            d.fill_solid(&Rectangle::new(Point::new(x, y), Size::new(n as u32, 1)), color).ok();
        }
        x += n;
        if x >= w {
            x = 0;
            y += 1;
        }
    }
    let inset = Rectangle::new(INSET.top_left + Point::new(0, MAP.start), INSET.size);
    inset.into_styled(PrimitiveStyleBuilder::new().stroke_color(EDGE_COLOR).stroke_width(2).build()).draw(d).ok();
    for (i, _) in REGIONS.iter().enumerate() {
        let b = label_box(i);
        let at = b.top_left + Point::new(LABEL_PAD, LABEL_PAD);
        let color = if Some(i as u8 + 1) == picked_id { PICKED } else { TEXT };
        for off in [Point::new(-2, 0), Point::new(2, 0), Point::new(0, -2), Point::new(0, 2)] {
            tab5_ui::ja_text(d, REGIONS[i].name, at + off, SHADOW);
        }
        tab5_ui::ja_text(d, REGIONS[i].name, at, color);
    }
    if let Some(p) = picked {
        tab5_ui::ja_text(d, &format!("いまは {}", p.name), Point::new(28, MAP.end + 30), TEXT);
    }
    RoundedRectangle::with_equal_corners(BACK, Size::new(16, 16))
        .into_styled(PrimitiveStyleBuilder::new().stroke_color(EDGE_COLOR).stroke_width(3).build())
        .draw(d)
        .ok();
    let label = "もどる";
    let at = BACK.center() - Point::new(tab5_ui::ja_width(label) / 2, 16);
    tab5_ui::ja_text(d, label, at, TEXT);
    0..HEIGHT
}

const LABEL_PAD: i32 = 6;
const BACK: Rectangle = Rectangle::new(Point::new(220, 1120), Size::new(280, 100));

/// Screen box of region `i`'s label, which is also a touch target.
fn label_box(i: usize) -> Rectangle {
    let r = &REGIONS[i];
    let w = tab5_ui::ja_width(r.name) + 2 * LABEL_PAD;
    let h = 32 + 2 * LABEL_PAD;
    let (x, y) = if r.name == "沖縄" {
        (INSET.center().x as f64, (INSET.top_left.y + INSET.size.height as i32 - 22) as f64)
    } else {
        project(r.label.0 as f64, r.label.1 as f64)
    };
    let c = Point::new(x as i32, y as i32 + MAP.start);
    let left = (c.x - w / 2).clamp(0, WIDTH - w);
    Rectangle::new(Point::new(left, c.y - h / 2), Size::new(w as u32, h as u32))
}

/// What a tap on the picker means.
#[derive(Debug, PartialEq)]
pub enum Pick {
    Region(&'static Region),
    Back,
    Nothing,
}

/// How far from a coast a tap at sea still picks the region.
const REACH: i32 = 30;

/// Resolves a tap at screen `(x, y)`; `map` is from [`decode`].
pub fn hit(map: &[u8], x: i32, y: i32) -> Pick {
    let p = Point::new(x, y);
    if BACK.offset(10).contains(p) {
        return Pick::Back;
    }
    let size = map_size();
    let (w, h) = (size.width as i32, size.height as i32);
    let (mx, my) = (x, y - MAP.start);
    let id_at = |x: i32, y: i32| (x >= 0 && y >= 0 && x < w && y < h).then(|| map[(y * w + x) as usize]);
    // Land wins over a label drawn on top of it.
    if let Some(id) = id_at(mx, my).filter(|&id| id != 0 && id != EDGE) {
        return Pick::Region(&REGIONS[id as usize - 1]);
    }
    if let Some(i) = (0..REGIONS.len()).find(|&i| label_box(i).offset(6).contains(p)) {
        return Pick::Region(&REGIONS[i]);
    }
    let mut best: Option<(i32, u8)> = None;
    for dy in -REACH..=REACH {
        for dx in -REACH..=REACH {
            let d2 = dx * dx + dy * dy;
            if d2 > REACH * REACH || best.is_some_and(|(b, _)| b <= d2) {
                continue;
            }
            if let Some(id) = id_at(mx + dx, my + dy).filter(|&id| id != 0 && id != EDGE) {
                best = Some((d2, id));
            }
        }
    }
    best.map_or(Pick::Nothing, |(_, id)| Pick::Region(&REGIONS[id as usize - 1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(lon: f64, lat: f64) -> Pick {
        let (x, y) = project(lon, lat);
        hit(&decode(), x as i32, y as i32 + MAP.start)
    }

    fn named(name: &str) -> Pick {
        Pick::Region(find(name).unwrap())
    }

    #[test]
    fn map_is_whole() {
        let size = map_size();
        assert_eq!(decode().len(), (size.width * size.height) as usize);
    }

    #[test]
    fn cities_land_in_their_regions() {
        assert_eq!(at(139.69, 35.69), named("関東"));
        assert_eq!(at(141.35, 43.06), named("道央・道南"));
        assert_eq!(at(142.36, 43.77), named("道北"));
        assert_eq!(at(144.38, 42.98), named("道東"));
        assert_eq!(at(143.2, 42.92), named("道東")); // Obihiro
        assert_eq!(at(140.10, 39.72), named("東北日本海側"));
        assert_eq!(at(140.87, 38.27), named("東北太平洋側"));
        assert_eq!(at(135.50, 34.69), named("近畿"));
        assert_eq!(at(133.05, 35.47), named("山陰"));
        assert_eq!(at(132.46, 34.39), named("山陽"));
        assert_eq!(at(133.53, 33.56), named("四国南部"));
        assert_eq!(at(130.40, 33.59), named("九州北部"));
        assert_eq!(at(130.56, 31.6), named("九州南部"));
    }

    #[test]
    fn labels_and_inset_pick_their_region() {
        let map = decode();
        for (i, r) in REGIONS.iter().enumerate() {
            // A label at sea, or on its own region's land.
            let c = label_box(i).center();
            assert_eq!(hit(&map, c.x, c.y), Pick::Region(r), "{}", r.name);
        }
        let (x, y) = project_inset(127.68, 26.21); // Naha
        assert_eq!(hit(&map, x as i32, y as i32 + MAP.start), named("沖縄"));
        assert_eq!(hit(&map, BACK.center().x, BACK.center().y), Pick::Back);
    }

    #[test]
    fn labels_do_not_overlap() {
        for i in 0..REGIONS.len() {
            for j in i + 1..REGIONS.len() {
                assert!(label_box(i).intersection(&label_box(j)).is_zero_sized(), "{} / {}", REGIONS[i].name, REGIONS[j].name);
            }
        }
    }

    #[test]
    fn every_region_has_land() {
        let map = decode();
        for i in 1..=REGIONS.len() as u8 {
            assert!(map.contains(&i), "{}", REGIONS[i as usize - 1].name);
        }
    }
}
