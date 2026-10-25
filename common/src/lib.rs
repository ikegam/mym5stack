//! Board-independent code shared by every firmware image.
//!
//! Keep this crate free of ESP-IDF dependencies so it can be unit-tested on
//! the host with `make test`.

pub mod active;
pub mod battery;
pub mod dns;
pub mod fb;
pub mod japan_map;
pub mod llm;
pub mod names;
pub mod ports;
pub mod sniff;
pub mod tab5_ui;
pub mod ui;
pub mod weather;

pub const FW_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Static description of the board a firmware image is built for.
#[derive(Debug, Clone, Copy)]
pub struct BoardInfo {
    pub name: &'static str,
    pub mcu: &'static str,
    pub display: Display,
}

#[derive(Debug, Clone, Copy)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    pub kind: &'static str,
}

pub fn banner(board: &BoardInfo) -> String {
    format!(
        "m5 firmware v{} on {} ({}), {} {}x{}",
        FW_VERSION, board.name, board.mcu, board.display.kind, board.display.width, board.display.height
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_mentions_board() {
        let b = BoardInfo {
            name: "Test",
            mcu: "esp32",
            display: Display { width: 1, height: 2, kind: "LCD" },
        };
        let s = banner(&b);
        assert!(s.contains("Test"));
        assert!(s.contains("1x2"));
    }
}
