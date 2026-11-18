//! H.264 through VA-API (Linux), the GPU's hardware decoder, by
//! `oxideav-vaapi` (libva loaded at run time; its H.264 front end fills the
//! picture, matrix and slice parameters from `oxideav-bitstream`'s parser).
//!
//! Each picture is read back from its surface once (`vaGetImage`, NV12)
//! and handed over as pixels. Exporting the surface itself as a `dma-buf`
//! (`vaExportSurfaceHandle`) would skip that copy, but the bridge reuses a
//! picture's surface as soon as the next one is decoded unless the picture
//! is a reference, while the player holds a few frames ahead and the
//! renderer the last ones; that needs surfaces the player can keep.

use super::VideoError;
use super::nal::{parse_avcc, split_units};
use super::player::{Coded, Decoded, FrameDecoder};
use crate::geometry::PhysicalSize;
use crate::render::external::YuvColorSpace;
use crate::render::{ExternalFormat, ExternalImage};
use oxideav_core::CodecParameters;
use oxideav_vaapi::Display;
use oxideav_vaapi::h264::{H264Decoder, H264Picture};
use oxideav_vaapi::session::Planes;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub(super) struct VaDecoder {
    decoder: H264Decoder,
    /// The parameter sets, fed again after a reset.
    sets: Vec<Vec<u8>>,
    len_size: usize,
    /// Pictures come out in decode order, one per packet: each takes the
    /// oldest packet's time.
    times: VecDeque<Duration>,
    last: Duration,
    out: Vec<Decoded>,
}

// The display and session are libva handles used by one thread at a time
// (the player's), as the bridge's own registry decoder does.
unsafe impl Send for VaDecoder {}

fn va_err(e: impl std::fmt::Display) -> VideoError {
    VideoError::Decode(format!("VA-API: {e}"))
}

/// The first render node libva initializes on.
fn open_display() -> Result<Display, VideoError> {
    let mut why = String::from("no /dev/dri render node");
    for n in 128..136 {
        let path = format!("/dev/dri/renderD{n}");
        if !Path::new(&path).exists() {
            continue;
        }
        match Display::open_drm(Path::new(&path)) {
            Ok(d) => return Ok(d),
            Err(e) => why = format!("{path}: {e}"),
        }
    }
    Err(VideoError::Unsupported(format!("no VA-API device ({why})")))
}

impl VaDecoder {
    pub(super) fn new(params: &CodecParameters) -> Result<Self, VideoError> {
        let (sets, len_size) = parse_avcc(&params.extradata).ok_or_else(|| {
            VideoError::Unsupported("H.264 without its configuration record".into())
        })?;
        let mut me = Self {
            decoder: H264Decoder::new(open_display()?),
            sets,
            len_size: len_size as usize,
            times: VecDeque::new(),
            last: Duration::ZERO,
            out: Vec::new(),
        };
        me.start()?;
        Ok(me)
    }

    /// Feed the parameter sets and make the session (a driver that can't
    /// decode the stream says so here).
    fn start(&mut self) -> Result<(), VideoError> {
        self.decoder
            .decode(self.sets.iter().map(|s| s.as_slice()))
            .map_err(va_err)?;
        self.decoder.prepare().map_err(|e| match e {
            oxideav_vaapi::session::DecodeError::Unsupported(m) => {
                VideoError::Unsupported(format!("VA-API: {m}"))
            }
            e => va_err(e),
        })
    }

    /// Read the pictures back and keep them.
    fn take(&mut self, pictures: Vec<H264Picture>) -> Result<(), VideoError> {
        for pic in pictures {
            let time = self.times.pop_front().unwrap_or(self.last);
            self.last = time;
            let (_, _, w, h) = pic.crop;
            let (w, h) = (w as usize, h as usize);
            if w == 0 || h == 0 {
                continue;
            }
            let mut luma = vec![0u8; w * h];
            let mut chroma = vec![0u8; w.div_ceil(2) * 2 * h.div_ceil(2)];
            let cs = w.div_ceil(2) * 2;
            self.decoder
                .read(&pic, &mut Planes::new(&mut luma, w, &mut chroma, cs))
                .map_err(va_err)?;
            let size = PhysicalSize::new(w as u32, h as u32);
            let color = if h > 576 {
                YuvColorSpace::Bt709Limited
            } else {
                YuvColorSpace::Bt601Limited
            };
            let Some(pixels) = crate::platform::external::to_rgba(
                ExternalFormat::Nv12,
                size,
                &[(&luma, w), (&chroma, cs)],
                color,
            ) else {
                continue;
            };
            self.out.push(Decoded {
                time,
                image: Arc::new(ExternalImage::from_pixels(Arc::new(pixels))),
            });
        }
        Ok(())
    }
}

impl FrameDecoder for VaDecoder {
    fn send(&mut self, packet: Coded<'_>) -> Result<(), VideoError> {
        self.times
            .push_back(packet.pts.unwrap_or(self.last + Duration::from_millis(33)));
        let units = split_units(packet.data, self.len_size);
        match self.decoder.decode(units) {
            Ok(pictures) => self.take(pictures),
            // A picture this bridge can't decode (field coding) is skipped.
            Err(oxideav_vaapi::session::DecodeError::Unsupported(_)) => Ok(()),
            Err(e) => Err(va_err(e)),
        }
    }

    fn receive(&mut self) -> Vec<Decoded> {
        std::mem::take(&mut self.out)
    }

    fn flush(&mut self) -> Result<(), VideoError> {
        let pictures = self.decoder.flush().map_err(va_err)?;
        self.take(pictures)
    }

    fn reset(&mut self) -> Result<(), VideoError> {
        self.decoder = H264Decoder::new(open_display()?);
        self.times.clear();
        self.out.clear();
        self.start()
    }

    fn reorder_depth(&self) -> usize {
        4
    }

    fn name(&self) -> &'static str {
        "VA-API"
    }
}
