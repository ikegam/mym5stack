//! Rasterizes prefecture outlines into the region map the Tab5 picker uses:
//!
//!   japan_map_gen japan.geojson src/japan_map.bin
//!
//! `japan.geojson` is from https://github.com/dataofjapan/land (地球地図日本).
use m5_common::japan_map::{self, EDGE};
use serde_json::Value;

type Ring = Vec<(f64, f64)>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [src, out] = &args[..] else { panic!("usage: japan_map_gen <japan.geojson> <out.bin>") };
    let geo: Value = serde_json::from_str(&std::fs::read_to_string(src).unwrap()).unwrap();
    let size = japan_map::map_size();
    let (w, h) = (size.width as usize, size.height as usize);
    let mut map = vec![0u8; w * h];
    let inset = japan_map::inset();

    for f in geo["features"].as_array().unwrap() {
        let pref = f["properties"]["id"].as_u64().unwrap() as u32;
        let g = &f["geometry"];
        let polys: Vec<&Value> = match g["type"].as_str().unwrap() {
            "Polygon" => vec![&g["coordinates"]],
            _ => g["coordinates"].as_array().unwrap().iter().collect(),
        };
        let okinawa = pref == 47;
        for poly in polys {
            let rings: Vec<Ring> = poly
                .as_array()
                .unwrap()
                .iter()
                .map(|ring| {
                    ring.as_array()
                        .unwrap()
                        .iter()
                        .map(|p| {
                            let (lon, lat) = (p[0].as_f64().unwrap(), p[1].as_f64().unwrap());
                            if okinawa { japan_map::project_inset(lon, lat) } else { japan_map::project(lon, lat) }
                        })
                        .collect()
                })
                .collect();
            fill(&rings, w, h, |x, y| {
                let id = if okinawa {
                    let inside = x as i32 > inset.top_left.x + 2
                        && (x as i32) < inset.top_left.x + inset.size.width as i32 - 2
                        && y as i32 > inset.top_left.y + 2
                        && (y as i32) < inset.top_left.y + inset.size.height as i32 - 2;
                    if !inside {
                        return;
                    }
                    japan_map::region_of(pref, 0.0, 0.0)
                } else {
                    let (lon, lat) = japan_map::unproject(x as f64 + 0.5, y as f64 + 0.5);
                    japan_map::region_of(pref, lon, lat)
                };
                if let Some(id) = id {
                    map[y * w + x] = id;
                }
            });
        }
    }

    // Coasts and region borders.
    let src = map.clone();
    for y in 0..h {
        for x in 0..w {
            let id = src[y * w + x];
            if id == 0 {
                continue;
            }
            let differs = |nx: usize, ny: usize| src[ny * w + nx] != id;
            if (x > 0 && differs(x - 1, y))
                || (x + 1 < w && differs(x + 1, y))
                || (y > 0 && differs(x, y - 1))
                || (y + 1 < h && differs(x, y + 1))
            {
                map[y * w + x] = EDGE;
            }
        }
    }

    let mut rle = Vec::new();
    for row in map.chunks(w) {
        let mut i = 0;
        while i < row.len() {
            let id = row[i];
            let n = row[i..].iter().take(255).take_while(|&&v| v == id).count();
            rle.extend([id, n as u8]);
            i += n;
        }
    }
    std::fs::write(out, &rle).unwrap();
    println!("wrote {out}: {} bytes", rle.len());
}

/// Even-odd scanline fill of `rings` (outer ring plus holes), calling `put`
/// for each pixel whose center is inside.
fn fill(rings: &[Ring], w: usize, h: usize, mut put: impl FnMut(usize, usize)) {
    let ys = rings.iter().flatten().map(|p| p.1);
    let (ymin, ymax) = ys.fold((f64::MAX, f64::MIN), |(a, b), y| (a.min(y), b.max(y)));
    let y0 = (ymin.floor().max(0.0)) as usize;
    let y1 = (ymax.ceil().min(h as f64)) as usize;
    for y in y0..y1 {
        let sy = y as f64 + 0.5;
        let mut xs = Vec::new();
        for ring in rings {
            for e in ring.windows(2) {
                let ((ax, ay), (bx, by)) = (e[0], e[1]);
                if (ay <= sy) != (by <= sy) {
                    xs.push(ax + (sy - ay) / (by - ay) * (bx - ax));
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks_exact(2) {
            let start = (pair[0] - 0.5).ceil().max(0.0) as usize;
            let end = ((pair[1] - 0.5).ceil().min(w as f64)).max(0.0) as usize;
            for x in start..end {
                put(x, y);
            }
        }
    }
}
