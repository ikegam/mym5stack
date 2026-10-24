//! IT8951 e-paper controller over SPI (M5Paper wiring).
//!
//! Every SPI exchange is a 16-bit preamble followed by 16-bit words, with the
//! host waiting for HRDY (BUSY pin high) between them, so chip select is
//! driven manually around each exchange.
//!
//! Reads go through a second, slower device on the same bus: at the 10 MHz
//! write clock the IT8951's MISO is sampled too early and bits read back as 0.

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use esp_idf_svc::hal::gpio::{Input, Output, PinDriver};
use esp_idf_svc::hal::spi::{SpiDeviceDriver, SpiDriver};
use m5_common::fb::Framebuffer;

const PRE_CMD: u16 = 0x6000;
const PRE_WRITE: u16 = 0x0000;
const PRE_READ: u16 = 0x1000;

const CMD_SYS_RUN: u16 = 0x0001;
const CMD_REG_RD: u16 = 0x0010;
const CMD_REG_WR: u16 = 0x0011;
const CMD_LD_IMG_AREA: u16 = 0x0021;
const CMD_LD_IMG_END: u16 = 0x0022;
const CMD_DPY_AREA: u16 = 0x0034;
const CMD_VCOM: u16 = 0x0039;
const CMD_GET_DEV_INFO: u16 = 0x0302;

const REG_I80CPCR: u16 = 0x0004;
const REG_LISAR: u16 = 0x0208;
const REG_LUTAFSR: u16 = 0x1224;

const BIG_ENDIAN: u16 = 1;
const BPP_4: u16 = 2;
/// M5Paper panel VCOM in mV (-2.30 V).
const VCOM_MV: u16 = 2300;
const CHUNK: usize = 4000;

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum Mode {
    /// Full clear, slow, removes ghosting.
    Init = 0,
    /// Fast monochrome, for small UI changes.
    Du = 1,
    /// 16-level with flashing, best quality.
    Gc16 = 2,
    /// 16-level without flashing.
    Gl16 = 3,
}

type Spi<'d> = SpiDeviceDriver<'d, &'d SpiDriver<'d>>;

pub struct It8951<'d> {
    spi: Spi<'d>,
    spi_rd: Spi<'d>,
    cs: PinDriver<'d, Output>,
    busy: PinDriver<'d, Input>,
    pub width: u32,
    pub height: u32,
}

impl<'d> It8951<'d> {
    pub fn new(
        spi: Spi<'d>,
        spi_rd: Spi<'d>,
        mut cs: PinDriver<'d, Output>,
        busy: PinDriver<'d, Input>,
    ) -> Result<Self> {
        cs.set_high()?;
        let mut epd = Self { spi, spi_rd, cs, busy, width: 0, height: 0 };
        epd.command(CMD_SYS_RUN)?;

        epd.command(CMD_GET_DEV_INFO)?;
        let mut info = [0u16; 20];
        epd.read_words(&mut info)?;
        let (w, h) = (info[0] as u32, info[1] as u32);
        if w == 0 || h == 0 || w == 0xffff {
            bail!("IT8951 not responding (dev info {:04x?})", &info[..4]);
        }
        let fw = words_to_str(&info[4..12]);
        let lut = words_to_str(&info[12..20]);
        log::info!("IT8951 {w}x{h} fw={fw} lut={lut}");
        epd.width = w;
        epd.height = h;

        let buf_addr = info[2] as u32 | (info[3] as u32) << 16;
        epd.write_reg(REG_I80CPCR, 0x0001)?; // packed write
        epd.write_reg(REG_LISAR + 2, (buf_addr >> 16) as u16)?;
        epd.write_reg(REG_LISAR, buf_addr as u16)?;
        epd.command_args(CMD_VCOM, &[1, VCOM_MV])?;
        Ok(epd)
    }

    fn wait_ready(&mut self) -> Result<()> {
        let start = Instant::now();
        while self.busy.is_low() {
            if start.elapsed() > Duration::from_secs(3) {
                bail!("IT8951 busy timeout");
            }
            std::hint::spin_loop();
        }
        Ok(())
    }

    fn word(&mut self, w: u16) -> Result<()> {
        self.spi.write(&w.to_be_bytes())?;
        Ok(())
    }

    /// One chip-select framed exchange: preamble, then `body`.
    fn framed(&mut self, preamble: u16, body: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        self.wait_ready()?;
        self.cs.set_low()?;
        let r = self.word(preamble).and_then(|_| self.wait_ready()).and_then(|_| body(self));
        self.cs.set_high()?;
        r
    }

    fn command(&mut self, cmd: u16) -> Result<()> {
        self.framed(PRE_CMD, |s| s.word(cmd))
    }

    fn write_word(&mut self, w: u16) -> Result<()> {
        self.framed(PRE_WRITE, |s| s.word(w))
    }

    fn command_args(&mut self, cmd: u16, args: &[u16]) -> Result<()> {
        self.command(cmd)?;
        for &a in args {
            self.write_word(a)?;
        }
        Ok(())
    }

    fn read_words(&mut self, out: &mut [u16]) -> Result<()> {
        self.framed(PRE_READ, |s| {
            let mut dummy = [0u8; 2];
            s.spi_rd.read(&mut dummy)?;
            s.wait_ready()?;
            let mut buf = vec![0u8; out.len() * 2];
            s.spi_rd.read(&mut buf)?;
            for (o, b) in out.iter_mut().zip(buf.chunks_exact(2)) {
                *o = u16::from_be_bytes([b[0], b[1]]);
            }
            Ok(())
        })
    }

    fn write_reg(&mut self, reg: u16, val: u16) -> Result<()> {
        self.command_args(CMD_REG_WR, &[reg, val])
    }

    fn read_reg(&mut self, reg: u16) -> Result<u16> {
        self.command_args(CMD_REG_RD, &[reg])?;
        let mut v = [0u16];
        self.read_words(&mut v)?;
        Ok(v[0])
    }

    /// Wait until the previous display refresh has finished.
    fn wait_display(&mut self) -> Result<()> {
        let start = Instant::now();
        while self.read_reg(REG_LUTAFSR)? != 0 {
            if start.elapsed() > Duration::from_secs(5) {
                bail!("IT8951 display timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Upload `fb` region (x, w multiples of 4) and refresh it on the panel.
    pub fn update(&mut self, fb: &Framebuffer, x: u32, y: u32, w: u32, h: u32, mode: Mode) -> Result<()> {
        let (x, w) = (x & !3, (w + 3) & !3);
        self.wait_display()?;
        self.command_args(
            CMD_LD_IMG_AREA,
            &[BIG_ENDIAN << 8 | BPP_4 << 4, x as u16, y as u16, w as u16, h as u16],
        )?;
        self.framed(PRE_WRITE, |s| {
            let mut chunk = Vec::with_capacity(CHUNK);
            for row in y..y + h {
                for part in fb.row(row, x, x + w).chunks(CHUNK) {
                    if chunk.len() + part.len() > CHUNK {
                        s.spi.write(&chunk)?;
                        chunk.clear();
                    }
                    chunk.extend_from_slice(part);
                }
            }
            if !chunk.is_empty() {
                s.spi.write(&chunk)?;
            }
            Ok(())
        })?;
        self.command(CMD_LD_IMG_END)?;
        self.command_args(CMD_DPY_AREA, &[x as u16, y as u16, w as u16, h as u16, mode as u16])
    }

    pub fn update_full(&mut self, fb: &Framebuffer, mode: Mode) -> Result<()> {
        self.update(fb, 0, 0, fb.width(), fb.height(), mode)
    }
}

fn words_to_str(w: &[u16]) -> String {
    w.iter()
        .flat_map(|w| w.to_le_bytes())
        .take_while(|&b| b != 0)
        .map(|b| b as char)
        .collect()
}
