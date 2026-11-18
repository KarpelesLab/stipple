//! Software decoding through oxideav's codec registry: MPEG-1/2 video
//! (`oxideav-mpeg12video`, pure Rust). Its frames come out in presentation
//! order as planar Y'CbCr, converted here to RGBA and handed over as
//! pixels.

use super::VideoError;
use super::player::{Coded, Decoded, FrameDecoder, context, decode_err};
use crate::geometry::PhysicalSize;
use crate::render::external::YuvColorSpace;
use crate::render::{ExternalImage, Pixmap};
use oxideav_core::{CodecParameters, Decoder, Error, Frame, Packet, TimeBase, VideoFrame};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::time::Duration;

pub(super) struct SoftDecoder {
    decoder: Box<dyn Decoder>,
    /// The presentation times of the packets fed and not matched to a
    /// frame yet, smallest first: frames come out in presentation order,
    /// so each takes the smallest (the decoder's own stamps count frames).
    times: BinaryHeap<Reverse<Duration>>,
    last: Option<Duration>,
    frame_time: Duration,
    out: Vec<Decoded>,
}

impl SoftDecoder {
    pub(super) fn new(params: &CodecParameters, frame_time: Duration) -> Result<Self, VideoError> {
        let decoder = context().codecs.first_decoder(params).map_err(decode_err)?;
        Ok(Self {
            decoder,
            times: BinaryHeap::new(),
            last: None,
            frame_time,
            out: Vec::new(),
        })
    }

    fn drain(&mut self) -> Result<(), VideoError> {
        loop {
            match self.decoder.receive_frame() {
                Ok(Frame::Video(v)) => {
                    let time = match self.times.pop() {
                        Some(Reverse(t)) => t,
                        None => self.last.map_or(Duration::ZERO, |t| t + self.frame_time),
                    };
                    self.last = Some(time);
                    if let Some(image) = to_image(&v) {
                        self.out.push(Decoded {
                            time,
                            image: Arc::new(image),
                        });
                    }
                }
                Ok(_) => {}
                Err(Error::NeedMore | Error::Eof) => return Ok(()),
                Err(e) => return Err(decode_err(e)),
            }
        }
    }
}

impl FrameDecoder for SoftDecoder {
    fn send(&mut self, packet: Coded<'_>) -> Result<(), VideoError> {
        let pts = packet
            .pts
            .unwrap_or_else(|| self.last.map_or(Duration::ZERO, |t| t + self.frame_time));
        self.times.push(Reverse(pts));
        let p = Packet::new(0, TimeBase::new(1, 1_000_000), packet.data.to_vec());
        // A packet that won't decode is skipped, as players do.
        let _ = self.decoder.send_packet(&p);
        self.drain()
    }

    fn receive(&mut self) -> Vec<Decoded> {
        std::mem::take(&mut self.out)
    }

    fn flush(&mut self) -> Result<(), VideoError> {
        self.decoder.flush().map_err(decode_err)?;
        self.drain()
    }

    fn reset(&mut self) -> Result<(), VideoError> {
        self.times.clear();
        self.last = None;
        self.out.clear();
        self.decoder.reset().map_err(decode_err)
    }

    fn reorder_depth(&self) -> usize {
        0
    }

    fn name(&self) -> &'static str {
        "software"
    }
}

/// A planar Y'CbCr frame (4:2:0, 4:2:2 or 4:4:4, each plane tightly
/// packed or with its stride) as an image of its RGBA pixels: BT.601
/// limited range at standard definition, BT.709 above, as MPEG-2 streams
/// mostly are.
pub(super) fn to_image(v: &VideoFrame) -> Option<ExternalImage> {
    let planes = v.image_planes();
    let [y, cb, cr] = [planes.first()?, planes.get(1)?, planes.get(2)?];
    let w = y.stride;
    let h = y.data.len().checked_div(y.stride)?;
    let (cw, ch) = (cb.stride, cb.data.len().checked_div(cb.stride)?);
    if w == 0 || h == 0 || cw == 0 || ch == 0 {
        return None;
    }
    let (sx, sy) = (w.div_ceil(cw).max(1), h.div_ceil(ch).max(1));
    let color = if h > 576 {
        YuvColorSpace::Bt709Limited
    } else {
        YuvColorSpace::Bt601Limited
    };
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let yr = &y.data[row * y.stride..row * y.stride + w];
        let crow = (row / sy).min(ch - 1);
        let br = cb.data.get(crow * cb.stride..crow * cb.stride + cw)?;
        let rr = cr.data.get(crow * cr.stride..crow * cr.stride + cw)?;
        for (x, px) in out[row * w * 4..(row + 1) * w * 4]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            let c = (x / sx).min(cw - 1);
            let [r, g, b] = color.to_rgb(yr[x], br[c], rr[c]);
            *px = [r, g, b, 255];
        }
    }
    let pixels = Pixmap::from_rgba8(PhysicalSize::new(w as u32, h as u32), out);
    Some(ExternalImage::from_pixels(Arc::new(pixels)))
}
