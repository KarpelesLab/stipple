//! Decoding encoded audio into `f32` frames.
//!
//! Plain PCM and float WAV files are read here, always. With the `audio`
//! feature everything else (Ogg Vorbis, MP3, FLAC, Opus, and WAV in other
//! encodings) goes through oxideav: a container demuxer finds the audio
//! stream and the codec's decoder turns its packets into samples.
//!
//! Every stream comes out as one or two channels: mono stays mono, and of
//! more than two channels the first two (front left and right) are kept.

use super::AudioError;
use std::sync::Arc;

/// Frames per chunk read from a WAV file.
const WAV_CHUNK: usize = 4096;

/// A source of decoded audio, read chunk by chunk.
pub(crate) trait PcmStream: Send {
    /// Frames per second.
    fn rate(&self) -> u32;
    /// 1 or 2.
    fn channels(&self) -> u16;
    /// The next interleaved samples (a whole number of frames), or `None` at
    /// the end.
    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, AudioError>;
    /// Go back to the start.
    fn rewind(&mut self) -> Result<(), AudioError>;
}

/// Open `data` for decoding.
pub(crate) fn open(data: Arc<[u8]>) -> Result<Box<dyn PcmStream>, AudioError> {
    if let Some(wav) = Wav::parse(data.clone())? {
        return Ok(Box::new(wav));
    }
    #[cfg(feature = "audio")]
    {
        oxide::OxideStream::open(data).map(|s| Box::new(s) as Box<dyn PcmStream>)
    }
    #[cfg(not(feature = "audio"))]
    {
        Err(AudioError::Unsupported(if data.starts_with(b"RIFF") {
            "a WAV encoding other than PCM or float".into()
        } else {
            "formats other than WAV need stipple's `audio` feature".into()
        }))
    }
}

/// Decode all of `data`: (interleaved samples, channels, rate).
pub(crate) fn decode_all(data: Arc<[u8]>) -> Result<(Vec<f32>, u16, u32), AudioError> {
    let mut s = open(data)?;
    let mut out = Vec::new();
    while let Some(chunk) = s.next_chunk()? {
        out.extend_from_slice(&chunk);
    }
    Ok((out, s.channels(), s.rate()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WavFormat {
    /// Signed integers (unsigned for 8 bits), of this many bytes.
    Int(usize),
    /// IEEE floats, of this many bytes.
    Float(usize),
}

/// A PCM or float WAV file.
struct Wav {
    data: Arc<[u8]>,
    format: WavFormat,
    /// Channels in the file.
    file_channels: usize,
    rate: u32,
    /// The sample data's byte range, and the read position in it.
    start: usize,
    end: usize,
    pos: usize,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

impl Wav {
    /// `Ok(None)` when `data` isn't a WAV file this reader handles (another
    /// format, or a WAV encoding other than PCM and float).
    fn parse(data: Arc<[u8]>) -> Result<Option<Self>, AudioError> {
        let b = &*data;
        if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
            return Ok(None);
        }
        let bad = |what: &str| AudioError::Decode(format!("WAV: {what}"));
        let mut at = 12;
        let mut fmt = None;
        let mut body = None;
        while at + 8 <= b.len() {
            let id = &b[at..at + 4];
            let size = u32_at(b, at + 4).ok_or_else(|| bad("truncated chunk"))? as usize;
            let start = at + 8;
            let end = start.saturating_add(size).min(b.len());
            match id {
                b"fmt " => {
                    let c = &b[start..end];
                    let mut tag = u16_at(c, 0).ok_or_else(|| bad("short fmt chunk"))?;
                    let channels = u16_at(c, 2).ok_or_else(|| bad("short fmt chunk"))?;
                    let rate = u32_at(c, 4).ok_or_else(|| bad("short fmt chunk"))?;
                    let bits = u16_at(c, 14).ok_or_else(|| bad("short fmt chunk"))?;
                    if tag == 0xFFFE {
                        // WAVE_FORMAT_EXTENSIBLE: the real tag opens the
                        // sub-format GUID.
                        tag = u16_at(c, 24).unwrap_or(0);
                    }
                    fmt = Some((tag, channels, rate, bits));
                }
                b"data" => {
                    body = Some((start, end));
                    if fmt.is_some() {
                        break;
                    }
                }
                _ => {}
            }
            // Chunks are padded to an even size.
            at = start.saturating_add(size).saturating_add(size & 1);
        }
        let (tag, channels, rate, bits) = fmt.ok_or_else(|| bad("no fmt chunk"))?;
        let (start, end) = body.ok_or_else(|| bad("no data chunk"))?;
        let format = match (tag, bits) {
            (1, 8 | 16 | 24 | 32) => WavFormat::Int(usize::from(bits / 8)),
            (3, 32 | 64) => WavFormat::Float(usize::from(bits / 8)),
            _ => return Ok(None),
        };
        if channels == 0 || rate == 0 {
            return Err(bad("no channels, or a rate of 0"));
        }
        Ok(Some(Self {
            data,
            format,
            file_channels: usize::from(channels),
            rate,
            start,
            end,
            pos: start,
        }))
    }

    fn sample(&self, at: usize) -> f32 {
        let b = &self.data[at..];
        match self.format {
            WavFormat::Int(1) => (f32::from(b[0]) - 128.0) / 128.0,
            WavFormat::Int(2) => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
            WavFormat::Int(3) => {
                (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0
            }
            WavFormat::Int(_) => {
                i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0
            }
            WavFormat::Float(4) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            WavFormat::Float(_) => {
                f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
            }
        }
    }
}

impl PcmStream for Wav {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> u16 {
        self.file_channels.min(2) as u16
    }

    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, AudioError> {
        let bytes = match self.format {
            WavFormat::Int(n) | WavFormat::Float(n) => n,
        };
        let frame = bytes * self.file_channels;
        let frames = ((self.end - self.pos) / frame).min(WAV_CHUNK);
        if frames == 0 {
            return Ok(None);
        }
        let out_ch = usize::from(self.channels());
        let mut out = Vec::with_capacity(frames * out_ch);
        for f in 0..frames {
            let at = self.pos + f * frame;
            for c in 0..out_ch {
                out.push(self.sample(at + c * bytes));
            }
        }
        self.pos += frames * frame;
        Ok(Some(out))
    }

    fn rewind(&mut self) -> Result<(), AudioError> {
        self.pos = self.start;
        Ok(())
    }
}

/// Write `samples` (interleaved, `channels` of them per frame) as a 16-bit
/// PCM WAV file.
pub(crate) fn encode_wav(samples: &[f32], channels: u16, rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(feature = "audio")]
mod oxide {
    //! Everything else, through oxideav's demuxers and decoders.

    use super::{AudioError, PcmStream};
    use oxideav_core::{
        CodecParameters, Decoder, Demuxer, Error, Frame, MediaType, RuntimeContext, SampleFormat,
    };
    use std::io::Cursor;
    use std::sync::{Arc, OnceLock};

    /// The registry of the formats Stipple decodes.
    fn context() -> &'static RuntimeContext {
        static CTX: OnceLock<RuntimeContext> = OnceLock::new();
        CTX.get_or_init(|| {
            let mut ctx = RuntimeContext::new();
            oxideav_basic::register(&mut ctx);
            oxideav_ogg::register(&mut ctx);
            oxideav_vorbis::register(&mut ctx);
            oxideav_mp3::register(&mut ctx);
            oxideav_flac::register(&mut ctx);
            oxideav_opus::register(&mut ctx);
            ctx
        })
    }

    fn err(e: Error) -> AudioError {
        match e {
            Error::Unsupported(m) | Error::FormatNotFound(m) | Error::CodecNotFound(m) => {
                AudioError::Unsupported(m)
            }
            e => AudioError::Decode(e.to_string()),
        }
    }

    /// `Arc<[u8]>` as a `Cursor`'s buffer.
    struct Bytes(Arc<[u8]>);

    impl AsRef<[u8]> for Bytes {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    pub(super) struct OxideStream {
        data: Arc<[u8]>,
        demuxer: Box<dyn Demuxer>,
        decoder: Box<dyn Decoder>,
        stream: u32,
        format: SampleFormat,
        file_channels: usize,
        rate: u32,
        flushed: bool,
    }

    impl OxideStream {
        pub(super) fn open(data: Arc<[u8]>) -> Result<Self, AudioError> {
            let ctx = context();
            let mut input = Cursor::new(Bytes(data.clone()));
            let name = ctx
                .containers
                .probe_input(&mut input, None)
                .map_err(|_| AudioError::Unsupported("not a format Stipple can decode".into()))?;
            let demuxer = ctx
                .containers
                .open_demuxer(&name, Box::new(input), &ctx.codecs)
                .map_err(err)?;
            let info = demuxer
                .streams()
                .iter()
                .find(|s| s.params.media_type == MediaType::Audio)
                .ok_or_else(|| AudioError::Decode(format!("no audio in this {name} file")))?;
            let stream = info.index;
            let mut params: CodecParameters = info.params.clone();
            let codec = params.codec_id.as_str().to_owned();
            if codec == "opus" {
                // Opus always decodes at 48 kHz; the header's rate is only
                // the original input's.
                params.sample_rate = Some(48_000);
            }
            let rate = params
                .sample_rate
                .filter(|&r| r > 0)
                .ok_or_else(|| AudioError::Decode("no sample rate".into()))?;
            let file_channels = usize::from(params.channels.unwrap_or(2).max(1));
            // What each decoder emits.
            let format = match codec.as_str() {
                "vorbis" => SampleFormat::F32P,
                "mp3" => SampleFormat::S16P,
                "opus" => SampleFormat::S16,
                _ => params.sample_format.unwrap_or(SampleFormat::S16),
            };
            let decoder = ctx.codecs.first_decoder(&params).map_err(err)?;
            Ok(Self {
                data,
                demuxer,
                decoder,
                stream,
                format,
                file_channels,
                rate,
                flushed: false,
            })
        }

        /// Convert one decoded frame to interleaved `f32`, one or two
        /// channels.
        fn convert(&self, planes: &[Vec<u8>], samples: usize) -> Vec<f32> {
            let ch = self.file_channels;
            let out_ch = self.channels() as usize;
            let planar = planes.len() > 1 || (self.format.is_planar() && ch == 1);
            // Trust the sizes over the declared format where they disagree.
            let per_plane = if planar { samples } else { samples * ch };
            let width = planes
                .first()
                .and_then(|p| p.len().checked_div(per_plane))
                .unwrap_or(0);
            let float = matches!(
                self.format,
                SampleFormat::F32 | SampleFormat::F32P | SampleFormat::F64 | SampleFormat::F64P
            );
            let read = |b: &[u8]| -> f32 {
                match (width, float) {
                    (1, _) => match self.format {
                        SampleFormat::S8 => f32::from(b[0] as i8) / 128.0,
                        _ => (f32::from(b[0]) - 128.0) / 128.0,
                    },
                    (2, _) => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
                    (3, _) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
                    (4, true) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    (4, false) => {
                        i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0
                    }
                    (8, _) => {
                        f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
                    }
                    _ => 0.0,
                }
            };
            if width == 0 {
                return Vec::new();
            }
            let mut out = Vec::with_capacity(samples * out_ch);
            for i in 0..samples {
                for c in 0..out_ch {
                    let (plane, at) = if planar {
                        (planes.get(c).or(planes.first()), i * width)
                    } else {
                        (planes.first(), (i * ch + c.min(ch - 1)) * width)
                    };
                    let v = plane.and_then(|p| p.get(at..at + width)).map_or(0.0, read);
                    out.push(v);
                }
            }
            out
        }
    }

    impl PcmStream for OxideStream {
        fn rate(&self) -> u32 {
            self.rate
        }

        fn channels(&self) -> u16 {
            self.file_channels.min(2) as u16
        }

        fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, AudioError> {
            loop {
                match self.decoder.receive_frame() {
                    Ok(Frame::Audio(a)) => {
                        let out = self.convert(&a.data, a.samples as usize);
                        if out.is_empty() {
                            continue;
                        }
                        return Ok(Some(out));
                    }
                    Ok(_) => continue,
                    Err(Error::NeedMore) => {}
                    Err(Error::Eof) => return Ok(None),
                    Err(e) => return Err(err(e)),
                }
                if self.flushed {
                    return Ok(None);
                }
                match self.demuxer.next_packet() {
                    Ok(p) if p.stream_index == self.stream => {
                        // A packet that won't decode is skipped, as players do.
                        let _ = self.decoder.send_packet(&p);
                    }
                    Ok(_) => {}
                    Err(Error::Eof) => {
                        self.flushed = true;
                        self.decoder.flush().map_err(err)?;
                    }
                    Err(e) => return Err(err(e)),
                }
            }
        }

        fn rewind(&mut self) -> Result<(), AudioError> {
            *self = Self::open(self.data.clone())?;
            Ok(())
        }
    }
}
