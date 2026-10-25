//! 720x1280 RGB565 MIPI-DSI LCD. Panel bring-up is in C
//! (`components/tab5_display`); drawing goes straight into its frame buffer.

use core::convert::Infallible;
use core::ffi::{c_int, c_void};

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use esp_idf_svc::sys::{self, esp_err_t};

pub const WIDTH: u32 = 720;
pub const HEIGHT: u32 = 1280;

extern "C" {
    fn tab5_display_init(fb: *mut *mut c_void, variant: *mut c_int) -> esp_err_t;
    fn tab5_display_flush(y0: c_int, y1: c_int) -> esp_err_t;
    fn tab5_display_brightness(percent: c_int) -> esp_err_t;
}

pub struct Lcd {
    fb: &'static mut [u16],
}

impl Lcd {
    pub fn new() -> anyhow::Result<Self> {
        let mut fb = core::ptr::null_mut();
        let mut variant = 0;
        sys::esp!(unsafe { tab5_display_init(&mut fb, &mut variant) })?;
        log::info!("lcd: panel variant {variant}");
        // SAFETY: the DPI driver owns a WIDTH x HEIGHT RGB565 buffer for the
        // life of the program and only reads it.
        let fb = unsafe { core::slice::from_raw_parts_mut(fb.cast::<u16>(), (WIDTH * HEIGHT) as usize) };
        Ok(Self { fb })
    }

    pub fn brightness(&mut self, percent: u8) -> anyhow::Result<()> {
        sys::esp!(unsafe { tab5_display_brightness(percent as c_int) })?;
        Ok(())
    }

    /// Pushes rows `y0..y1` to the panel.
    pub fn flush_rows(&mut self, y0: i32, y1: i32) {
        let (y0, y1) = (y0.clamp(0, HEIGHT as i32), y1.clamp(0, HEIGHT as i32));
        if y0 < y1 {
            unsafe { tab5_display_flush(y0, y1) };
        }
    }

    pub fn flush(&mut self) {
        self.flush_rows(0, HEIGHT as i32);
    }
}

impl OriginDimensions for Lcd {
    fn size(&self) -> Size {
        Size::new(WIDTH, HEIGHT)
    }
}

impl DrawTarget for Lcd {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && (p.x as u32) < WIDTH && (p.y as u32) < HEIGHT {
                self.fb[(p.y as u32 * WIDTH + p.x as u32) as usize] = RawU16::from(c).into_inner();
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&self.bounding_box());
        let raw = RawU16::from(color).into_inner();
        for y in area.rows() {
            let row = (y as u32 * WIDTH) as usize;
            let x0 = area.top_left.x as usize;
            self.fb[row + x0..row + x0 + area.size.width as usize].fill(raw);
        }
        Ok(())
    }
}

/// Off-screen copy of rows `y0..y0 + h`, drawn in screen coordinates and then
/// copied to the frame buffer in one go. Drawing the animated scene straight
/// into the live frame buffer shows every clear-then-redraw as flicker.
pub struct Offscreen {
    y0: i32,
    h: i32,
    buf: Vec<u16>,
}

impl Offscreen {
    pub fn new(rows: core::ops::Range<i32>) -> Self {
        let h = rows.end - rows.start;
        Self { y0: rows.start, h, buf: vec![0; (WIDTH as i32 * h) as usize] }
    }
}

impl OriginDimensions for Offscreen {
    fn size(&self) -> Size {
        Size::new(WIDTH, HEIGHT)
    }
}

impl DrawTarget for Offscreen {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            let y = p.y - self.y0;
            if p.x >= 0 && (p.x as u32) < WIDTH && y >= 0 && y < self.h {
                self.buf[(y as u32 * WIDTH + p.x as u32) as usize] = RawU16::from(c).into_inner();
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&Rectangle::new(Point::new(0, self.y0), Size::new(WIDTH, self.h as u32)));
        let raw = RawU16::from(color).into_inner();
        for y in area.rows() {
            let row = ((y - self.y0) as u32 * WIDTH) as usize;
            let x0 = area.top_left.x as usize;
            self.buf[row + x0..row + x0 + area.size.width as usize].fill(raw);
        }
        Ok(())
    }
}

impl Offscreen {
    pub fn snapshot(&self) -> Vec<u16> {
        self.buf.clone()
    }

    pub fn restore(&mut self, saved: &[u16]) {
        self.buf.copy_from_slice(saved);
    }
}

impl Lcd {
    /// Copies an off-screen buffer into place and shows it.
    pub fn blit(&mut self, off: &Offscreen) {
        let start = (off.y0 as u32 * WIDTH) as usize;
        self.fb[start..start + off.buf.len()].copy_from_slice(&off.buf);
        self.flush_rows(off.y0, off.y0 + off.h);
    }
}
