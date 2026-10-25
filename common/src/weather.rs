//! Japan Meteorological Agency forecast JSON
//! (`https://www.jma.go.jp/bosai/forecast/data/forecast/<office>.json`).

use serde_json::Value;

pub fn url(office: &str) -> String {
    format!("https://www.jma.go.jp/bosai/forecast/data/forecast/{office}.json")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sky {
    Sunny,
    Cloudy,
    Rain,
    Snow,
}

impl Sky {
    /// From a JMA weather code: the hundreds digit is the main weather.
    pub fn from_code(code: u16) -> Self {
        match code / 100 {
            1 => Sky::Sunny,
            3 => Sky::Rain,
            4 => Sky::Snow,
            _ => Sky::Cloudy,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Forecast {
    /// e.g. "東京地方"
    pub area: String,
    /// "2026-10-25T05:00:00+09:00"
    pub reported: String,
    pub code: u16,
    pub sky: Sky,
    /// Today's forecast text with JMA's ideographic spaces as plain spaces.
    pub weather: String,
    pub wind: String,
    /// Remaining 6-hour chances of rain today: (start hour, percent).
    pub pops: Vec<(u8, u8)>,
    pub temp_max: Option<i32>,
    pub temp_min: Option<i32>,
}

impl Forecast {
    pub fn thunder(&self) -> bool {
        self.weather.contains('雷')
    }

    /// Rain later in the day although the main weather isn't rain.
    pub fn rain_later(&self) -> bool {
        self.sky != Sky::Rain && self.weather.contains('雨')
    }

    /// "10月25日" from the report time.
    pub fn date_ja(&self) -> String {
        let m: u32 = self.reported.get(5..7).and_then(|s| s.parse().ok()).unwrap_or(0);
        let d: u32 = self.reported.get(8..10).and_then(|s| s.parse().ok()).unwrap_or(0);
        format!("{m}月{d}日")
    }

    /// A Japanese opening for stories15M-ja (TinyStories translated to Japanese).
    pub fn story_prompt_ja(&self) -> String {
        let mut p = String::from(match self.sky {
            Sky::Sunny => "ある晴れた日のこと。",
            Sky::Cloudy => "ある くもりの日のこと。",
            Sky::Rain => "ある雨の日のこと。",
            Sky::Snow => "雪のふる日のこと。",
        });
        match (self.rain_later(), self.thunder()) {
            (true, true) => p.push_str("お昼すぎから雨がふってきて、空ではゴロゴロと雷が鳴っていました。"),
            (true, false) => p.push_str("お昼すぎから雨がふってきました。"),
            (false, true) => p.push_str("空ではゴロゴロと雷が鳴っていました。"),
            (false, false) => {}
        }
        match self.temp_max {
            Some(t) if t >= 30 => p.push_str("とても暑い日でした。"),
            Some(t) if t <= 8 => p.push_str("とても寒い日でした。"),
            _ => {}
        }
        p
    }

    /// An English opening for TinyStories, which only knows simple English.
    pub fn story_prompt(&self) -> String {
        let mut p = String::from(match self.sky {
            Sky::Sunny => "Once upon a time, it was a bright sunny day.",
            Sky::Cloudy => "Once upon a time, it was a gray cloudy day.",
            Sky::Rain => "Once upon a time, it was a rainy day.",
            Sky::Snow => "Once upon a time, it was a cold snowy day.",
        });
        if self.rain_later() {
            p.push_str(" Dark clouds came and it started to rain.");
        }
        if self.thunder() {
            p.push_str(" There was loud thunder in the sky.");
        }
        match self.temp_max {
            Some(t) if t >= 30 => p.push_str(" It was very hot."),
            Some(t) if t <= 8 => p.push_str(" It was very cold."),
            _ => {}
        }
        p
    }
}

pub fn parse_jma(json: &str) -> Result<Forecast, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad JSON: {e}"))?;
    let report = &v[0];
    let ts = report["timeSeries"].as_array().ok_or("no timeSeries")?;
    let str_at = |v: &Value| v.as_str().map(|s| s.replace('\u{3000}', " ")).unwrap_or_default();

    let sky_ts = ts.first().ok_or("no weather series")?;
    let area = &sky_ts["areas"][0];
    let code: u16 = area["weatherCodes"][0].as_str().and_then(|s| s.parse().ok()).ok_or("no weather code")?;
    let today = sky_ts["timeDefines"][0].as_str().and_then(|s| s.get(..10)).ok_or("no date")?.to_string();

    // Values in a series whose time falls on `today`.
    let on_today = |series: Option<&Value>, key: &str| -> Vec<(String, String)> {
        let Some(s) = series else { return Vec::new() };
        let times = s["timeDefines"].as_array().cloned().unwrap_or_default();
        let vals = s["areas"][0][key].as_array().cloned().unwrap_or_default();
        times
            .iter()
            .zip(vals)
            .filter_map(|(t, v)| Some((t.as_str()?.to_string(), v.as_str()?.to_string())))
            .filter(|(t, v)| t.starts_with(&today) && !v.is_empty())
            .collect()
    };
    let pops = on_today(ts.get(1), "pops")
        .into_iter()
        .filter_map(|(t, v)| Some((t.get(11..13)?.parse().ok()?, v.parse().ok()?)))
        .collect();
    let temps: Vec<i32> = on_today(ts.get(2), "temps").iter().filter_map(|(_, v)| v.parse().ok()).collect();
    let temp_max = temps.iter().copied().max();
    let temp_min = temps.iter().copied().min().filter(|&m| Some(m) != temp_max);

    Ok(Forecast {
        area: str_at(&area["area"]["name"]),
        reported: report["reportDatetime"].as_str().unwrap_or_default().to_string(),
        code,
        sky: Sky::from_code(code),
        weather: str_at(&area["weathers"][0]),
        wind: str_at(&area["winds"][0]),
        pops,
        temp_max,
        temp_min,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO_JSON: &str = include_str!("../testdata/jma_130000.json");

    #[test]
    fn parses_tokyo_forecast() {
        let f = parse_jma(TOKYO_JSON).unwrap();
        assert_eq!(f.area, "東京地方");
        assert_eq!(f.code, 214);
        assert_eq!(f.sky, Sky::Cloudy);
        assert!(f.weather.starts_with("くもり 昼過ぎ から 雨"));
        assert_eq!(f.pops, vec![(6, 10), (12, 60), (18, 50)]);
        assert_eq!(f.temp_max, Some(26));
        assert_eq!(f.temp_min, None);
        assert_eq!(f.date_ja(), "10月25日");
        assert!(f.rain_later() && f.thunder());
        assert_eq!(
            f.story_prompt_ja(),
            "ある くもりの日のこと。お昼すぎから雨がふってきて、空ではゴロゴロと雷が鳴っていました。"
        );
        assert_eq!(
            f.story_prompt(),
            "Once upon a time, it was a gray cloudy day. Dark clouds came and it started to rain. \
             There was loud thunder in the sky."
        );
    }

    #[test]
    fn parses_other_offices() {
        let f = parse_jma(include_str!("../testdata/jma_014100.json")).unwrap();
        assert_eq!(f.area, "釧路地方");
        assert_eq!(f.sky, Sky::Sunny);
        assert_eq!(f.pops, vec![(6, 0), (12, 0), (18, 0)]);
        assert_eq!(f.temp_max, Some(13));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_jma("[]").is_err());
        assert!(parse_jma("not json").is_err());
    }

    /// Every character JMA forecasts commonly use must exist in the font the
    /// Tab5 draws Japanese with.
    #[test]
    fn japanese_font_covers_forecast_text() {
        use u8g2_fonts::{fonts, FontRenderer};
        let f = parse_jma(TOKYO_JSON).unwrap();
        let common = "晴れ くもり 雨 雪 雷 霧 みぞれ 時々 一時 後 から 所により 朝 昼前 昼過ぎ 夕方 夜 夜のはじめ頃 \
                      明け方 未明 を伴う 強く 非常に 激しく 北 南 東 西 の風 波 メートル 降水確率 最高 最低 気温 \
                      度 ％ 月 日 地方 区 西部 東部 伊豆諸島 東京 ０１２３４５６７８９．";
        let text = format!("{}{}{}{}", f.area, f.weather, f.wind, common);
        let font = FontRenderer::new::<fonts::u8g2_font_b16_t_japanese3>();
        let missing: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .filter(|c| font.get_rendered_dimensions(*c, Default::default(), Default::default()).is_err())
            .collect();
        assert!(missing.is_empty(), "missing glyphs: {missing}");
    }
}
