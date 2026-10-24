//! GT911 capacitive touch controller (M5Paper: I2C SDA=21 SCL=22).

use anyhow::{bail, Result};
use esp_idf_svc::hal::delay::BLOCK;
use esp_idf_svc::hal::i2c::I2cDriver;

const ADDRS: [u8; 2] = [0x14, 0x5d];
const REG_PRODUCT_ID: u16 = 0x8140;
const REG_STATUS: u16 = 0x814e;
const REG_POINT1: u16 = 0x8150;

/// Map raw controller coordinates to screen coordinates (landscape 960x540).
/// The M5Paper GT911 reports in portrait (540x960), rotated 90° from the
/// IT8951's native landscape frame.
fn to_screen(x: u16, y: u16) -> (i32, i32) {
    (y as i32, m5_common::ui::HEIGHT as i32 - x as i32)
}

pub struct Gt911<'d> {
    i2c: I2cDriver<'d>,
    addr: u8,
    was_down: bool,
}

impl<'d> Gt911<'d> {
    pub fn new(mut i2c: I2cDriver<'d>) -> Result<Self> {
        for addr in ADDRS {
            let mut id = [0u8; 4];
            if read(&mut i2c, addr, REG_PRODUCT_ID, &mut id).is_ok() {
                log::info!("GT911 at 0x{addr:02x}, product id {:?}", String::from_utf8_lossy(&id));
                return Ok(Self { i2c, addr, was_down: false });
            }
        }
        bail!("GT911 not found")
    }

    /// Returns the screen position of a new touch (finger down edge).
    pub fn poll_tap(&mut self) -> Result<Option<(i32, i32)>> {
        let mut status = [0u8];
        read(&mut self.i2c, self.addr, REG_STATUS, &mut status)?;
        if status[0] & 0x80 == 0 {
            return Ok(None); // no new data
        }
        let points = status[0] & 0x0f;
        let mut tap = None;
        if points > 0 && !self.was_down {
            let mut p = [0u8; 4];
            read(&mut self.i2c, self.addr, REG_POINT1, &mut p)?;
            let (rx, ry) = (u16::from_le_bytes([p[0], p[1]]), u16::from_le_bytes([p[2], p[3]]));
            let pos = to_screen(rx, ry);
            log::info!("touch raw=({rx},{ry}) screen={pos:?}");
            tap = Some(pos);
        }
        self.was_down = points > 0;
        let [hi, lo] = REG_STATUS.to_be_bytes();
        self.i2c.write(self.addr, &[hi, lo, 0], BLOCK)?;
        Ok(tap)
    }
}

fn read(i2c: &mut I2cDriver, addr: u8, reg: u16, buf: &mut [u8]) -> Result<()> {
    i2c.write_read(addr, &reg.to_be_bytes(), buf, BLOCK)?;
    Ok(())
}
