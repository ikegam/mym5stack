//! 4-bit grayscale framebuffer (0 = black, 15 = white).
//!
//! Two pixels per byte, even x in the high nibble: the layout the IT8951
//! expects for 4bpp loads in big-endian mode, so rows can be sent as-is.

use core::convert::Infallible;
use embedded_graphics::pixelcolor::{Gray4, GrayColor};
use embedded_graphics::prelude::*;

pub struct Framebuffer {
    width: u32,
    height: u32,
    buf: Vec<u8>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        assert!(width % 4 == 0, "width must be a multiple of 4");
        Self { width, height, buf: vec![0xff; (width * height / 2) as usize] }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn clear_white(&mut self) {
        self.buf.fill(0xff);
    }

    /// Packed bytes of row `y` for pixels `x0..x1` (both even).
    pub fn row(&self, y: u32, x0: u32, x1: u32) -> &[u8] {
        let base = (y * self.width / 2) as usize;
        &self.buf[base + (x0 / 2) as usize..base + (x1 / 2) as usize]
    }

    pub fn get(&self, x: u32, y: u32) -> u8 {
        let b = self.buf[((y * self.width + x) / 2) as usize];
        if x % 2 == 0 { b >> 4 } else { b & 0x0f }
    }

    fn set(&mut self, x: u32, y: u32, luma: u8) {
        let i = ((y * self.width + x) / 2) as usize;
        let b = &mut self.buf[i];
        *b = if x % 2 == 0 { (*b & 0x0f) | (luma << 4) } else { (*b & 0xf0) | luma };
    }

    /// Binary PGM, for previewing layouts on the host.
    pub fn to_pgm(&self) -> Vec<u8> {
        let mut out = format!("P5\n{} {}\n15\n", self.width, self.height).into_bytes();
        for y in 0..self.height {
            for x in 0..self.width {
                out.push(self.get(x, y));
            }
        }
        out
    }
}

impl OriginDimensions for Framebuffer {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for Framebuffer {
    type Color = Gray4;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && (p.x as u32) < self.width && (p.y as u32) < self.height {
                self.set(p.x as u32, p.y as u32, c.luma());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nibble_layout() {
        let mut fb = Framebuffer::new(4, 1);
        Pixel(Point::new(0, 0), Gray4::new(0x1)).draw(&mut fb).unwrap();
        Pixel(Point::new(1, 0), Gray4::new(0x2)).draw(&mut fb).unwrap();
        Pixel(Point::new(3, 0), Gray4::BLACK).draw(&mut fb).unwrap();
        assert_eq!(fb.row(0, 0, 4), &[0x12, 0xf0]);
    }
}
