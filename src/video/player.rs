//! The native player: a thread per video that demuxes the file (oxideav's
//! MP4 and Matroska demuxers), feeds the video stream's packets to a
//! decoder (VideoToolbox, VA-API, or oxideav's MPEG-1/2 software decoder),
//! puts the frames back in presentation order and shows each when its time
//! comes, against the clock, waking the views that draw it.

use super::{Shared, Source, VideoError, VideoFrame, VideoOptions, VideoStatus};
use crate::geometry::PhysicalSize;
use crate::render::ExternalImage;
use oxideav_core::{CodecParameters, Demuxer, Error, MediaType, RuntimeContext, TimeBase};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// A decoded frame, in whatever order the decoder gives them.
pub(super) struct Decoded {
    pub(super) time: Duration,
    pub(super) image: Arc<ExternalImage>,
}

/// A packet's payload and timing, handed to a decoder.
pub(super) struct Coded<'a> {
    pub(super) data: &'a [u8],
    /// Its presentation time, where the container says.
    pub(super) pts: Option<Duration>,
}

/// A video decoder behind the player.
pub(super) trait FrameDecoder: Send {
    /// Feed one packet (one access unit).
    fn send(&mut self, packet: Coded<'_>) -> Result<(), VideoError>;
    /// The frames decoded so far, taken.
    fn receive(&mut self) -> Vec<Decoded>;
    /// The stream ended: decode what's held.
    fn flush(&mut self) -> Result<(), VideoError>;
    /// Forget everything (a seek follows).
    fn reset(&mut self) -> Result<(), VideoError>;
    /// Frames it may give out of presentation order (what the player
    /// holds back to sort them).
    fn reorder_depth(&self) -> usize;
    /// Its name, for [`Video::decoder`](super::Video::decoder).
    fn name(&self) -> &'static str;
}

/// The registry of the containers and software codecs Stipple plays.
pub(super) fn context() -> &'static RuntimeContext {
    static CTX: OnceLock<RuntimeContext> = OnceLock::new();
    CTX.get_or_init(|| {
        let mut ctx = RuntimeContext::new();
        oxideav_mp4::register(&mut ctx);
        oxideav_mkv::register(&mut ctx);
        oxideav_mpeg12video::register(&mut ctx);
        ctx
    })
}

/// `Arc<[u8]>` as a `Cursor`'s buffer.
struct Bytes(Arc<[u8]>);

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

pub(super) fn decode_err(e: Error) -> VideoError {
    match e {
        Error::Unsupported(m) | Error::FormatNotFound(m) | Error::CodecNotFound(m) => {
            VideoError::Unsupported(m)
        }
        e => VideoError::Decode(e.to_string()),
    }
}

/// Open `source`'s container.
fn open_demuxer(source: &Source) -> Result<(Box<dyn Demuxer>, &'static str), VideoError> {
    let ctx = context();
    let mut input: Box<dyn oxideav_core::ReadSeek> = match source {
        Source::Bytes(b) => Box::new(Cursor::new(Bytes(b.clone()))),
        Source::File(p) => Box::new(std::io::BufReader::new(
            std::fs::File::open(p).map_err(|e| VideoError::Io(format!("{}: {e}", p.display())))?,
        )),
    };
    let name = ctx
        .containers
        .probe_input(&mut *input, None)
        .map_err(|_| VideoError::Unsupported("not a container Stipple can read".into()))?;
    let name: &'static str = match name.as_str() {
        "mp4" => "mp4",
        "matroska" => "matroska",
        "webm" => "webm",
        other => return Err(VideoError::Unsupported(format!("{other} files"))),
    };
    let demuxer = ctx
        .containers
        .open_demuxer(name, input, &ctx.codecs)
        .map_err(decode_err)?;
    Ok((demuxer, name))
}

/// `ticks` of `tb` as a duration (negative times as zero).
pub(super) fn to_duration(ticks: i64, tb: TimeBase) -> Duration {
    let (num, den) = (tb.0.num as i128, tb.0.den as i128);
    if den <= 0 || num <= 0 || ticks <= 0 {
        return Duration::ZERO;
    }
    let ns = ticks as i128 * num * 1_000_000_000 / den;
    Duration::from_nanos(ns.min(u64::MAX as i128) as u64)
}

fn to_ticks(t: Duration, tb: TimeBase) -> i64 {
    let (num, den) = (tb.0.num as i128, tb.0.den as i128);
    if num <= 0 || den <= 0 {
        return 0;
    }
    (t.as_nanos() as i128 * den / (num * 1_000_000_000)).min(i64::MAX as i128) as i64
}

/// The decoder for `params`: the platform's hardware one where it takes
/// the codec (and `hardware` allows), else a software one.
fn make_decoder(
    params: &CodecParameters,
    hardware: bool,
    frame_time: Duration,
) -> Result<Box<dyn FrameDecoder>, VideoError> {
    let codec = params.codec_id.as_str();
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "ios", target_os = "linux")),
        allow(unused_mut)
    )]
    let mut why = None;
    if hardware {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        if matches!(codec, "h264" | "hevc" | "h265") {
            match super::vt::VtDecoder::new(params) {
                Ok(d) => return Ok(Box::new(d)),
                Err(e) => why = Some(e),
            }
        }
        #[cfg(target_os = "linux")]
        if codec == "h264" {
            match super::vaapi::VaDecoder::new(params) {
                Ok(d) => return Ok(Box::new(d)),
                Err(e) => why = Some(e),
            }
        }
    }
    if matches!(codec, "mpeg1video" | "mpeg2video") {
        return super::soft::SoftDecoder::new(params, frame_time).map(|d| Box::new(d) as _);
    }
    Err(why.unwrap_or_else(|| {
        VideoError::Unsupported(match codec {
            "h264" | "hevc" | "h265" if !hardware => {
                format!("{codec} has no software decoder; it needs hardware decoding")
            }
            "h264" | "hevc" | "h265" => format!("{codec} on this platform"),
            other => format!("the {other} codec"),
        })
    }))
}

/// A video's file, decoded frame by frame in presentation order.
pub(super) struct Stream {
    source: Source,
    demuxer: Box<dyn Demuxer>,
    stream: u32,
    time_base: TimeBase,
    decoder: Box<dyn FrameDecoder>,
    /// Decoded frames waiting for their turn, by time (and arrival).
    held: BTreeMap<(Duration, u64), Arc<ExternalImage>>,
    arrivals: u64,
    /// The demuxer said the end; the decoder was flushed.
    drained: bool,
    pub(super) duration: Option<Duration>,
    pub(super) size: PhysicalSize,
    /// One frame's time (from the frame rate; 1/30 s if unknown).
    pub(super) frame_time: Duration,
}

impl Stream {
    pub(super) fn open(source: Source, hardware: bool) -> Result<Self, VideoError> {
        let (demuxer, container) = open_demuxer(&source)?;
        let info = demuxer
            .streams()
            .iter()
            .find(|s| s.params.media_type == MediaType::Video)
            .ok_or_else(|| VideoError::Decode(format!("no video in this {container} file")))?
            .clone();
        let fps = info
            .params
            .frame_rate
            .filter(|r| r.num > 0 && r.den > 0)
            .map(|r| r.num as f64 / r.den as f64)
            .filter(|f| (1.0..=480.0).contains(f))
            .unwrap_or(30.0);
        let frame_time = Duration::from_secs_f64(1.0 / fps);
        let decoder = make_decoder(&info.params, hardware, frame_time)?;
        let duration = demuxer
            .duration_micros()
            .filter(|&d| d > 0)
            .map(|d| Duration::from_micros(d as u64))
            .or_else(|| info.duration.map(|d| to_duration(d, info.time_base)));
        let size = PhysicalSize::new(
            info.params.width.unwrap_or(0),
            info.params.height.unwrap_or(0),
        );
        let mut me = Self {
            source,
            demuxer,
            stream: info.index,
            time_base: info.time_base,
            decoder,
            held: BTreeMap::new(),
            arrivals: 0,
            drained: false,
            duration,
            size,
            frame_time,
        };
        me.demuxer.set_active_streams(&[me.stream]);
        Ok(me)
    }

    pub(super) fn decoder_name(&self) -> &'static str {
        self.decoder.name()
    }

    /// The next frame in presentation order; `None` at the end.
    pub(super) fn next(&mut self) -> Result<Option<Decoded>, VideoError> {
        loop {
            for d in self.decoder.receive() {
                self.arrivals += 1;
                self.held.insert((d.time, self.arrivals), d.image);
            }
            if self.held.len() > self.decoder.reorder_depth()
                || (self.drained && !self.held.is_empty())
            {
                let ((time, _), image) = self.held.pop_first().expect("a held frame");
                if self.size.width == 0 {
                    self.size = image.size();
                }
                return Ok(Some(Decoded { time, image }));
            }
            if self.drained {
                return Ok(None);
            }
            match self.demuxer.next_packet() {
                Ok(p) if p.stream_index == self.stream => {
                    let pts = p.pts.map(|t| to_duration(t, p.time_base));
                    self.decoder.send(Coded { data: &p.data, pts })?;
                }
                Ok(_) => {}
                Err(Error::Eof) => {
                    self.drained = true;
                    self.decoder.flush()?;
                }
                Err(e) => return Err(decode_err(e)),
            }
        }
    }

    /// Go to `at`: the frame to show there (the first at or after it, or
    /// the last one if it's past the end).
    pub(super) fn seek(&mut self, at: Duration) -> Result<Option<Decoded>, VideoError> {
        self.held.clear();
        self.drained = false;
        self.decoder.reset()?;
        if self
            .demuxer
            .seek_to(self.stream, to_ticks(at, self.time_base))
            .is_err()
        {
            // No index: read it again from the start.
            let (demuxer, _) = open_demuxer(&self.source)?;
            self.demuxer = demuxer;
            self.demuxer.set_active_streams(&[self.stream]);
        }
        // Within half a frame counts as there.
        let target = at.saturating_sub(self.frame_time / 2);
        let mut last = None;
        while let Some(d) = self.next()? {
            if d.time >= target {
                return Ok(Some(d));
            }
            last = Some(d);
        }
        Ok(last)
    }
}

/// Open the file (on the caller's thread, so what can't be played says so
/// at once) and start the player's thread.
pub(super) fn start(
    source: Source,
    shared: Arc<Shared>,
    options: &VideoOptions,
) -> Result<(), VideoError> {
    let stream = Stream::open(source, options.hardware)?;
    {
        let mut st = shared.lock();
        st.duration = stream.duration;
        st.size = stream.size;
        st.decoder = stream.decoder_name();
    }
    std::thread::Builder::new()
        .name("stipple-video".into())
        .spawn(move || run(stream, shared))
        .map_err(|e| VideoError::Decode(format!("no thread for the player: {e}")))?;
    Ok(())
}

/// Frames this late are dropped (decoded, not shown), unless nothing has
/// been shown for a while.
const LATE: Duration = Duration::from_millis(60);

fn run(mut stream: Stream, shared: Arc<Shared>) {
    // The next frame, decoded, not shown yet.
    let mut pending: Option<Decoded> = None;
    // The wall-clock instant a media time is due at, while playing.
    let mut anchor: Option<(Instant, Duration)> = None;
    // Show the next frame at once, playing or not (the first, a seek's).
    let mut show_now = true;
    let mut last_shown = Instant::now();
    let mut ended = false;
    loop {
        let (playing, looping, seek, generation) = {
            let mut st = shared.lock();
            if st.quit {
                return;
            }
            (st.playing, st.looping, st.seek.take(), st.generation)
        };
        if let Some(at) = seek {
            ended = false;
            anchor = None;
            match stream.seek(at) {
                Ok(frame) => {
                    pending = frame;
                    show_now = true;
                }
                Err(e) => {
                    shared.set_status(VideoStatus::Failed(e));
                    return;
                }
            }
        }
        let wait = |timeout: Option<Duration>| {
            let st = shared.lock();
            let still = |st: &mut super::State| st.generation == generation && !st.quit;
            match timeout {
                Some(t) => drop(shared.changed.wait_timeout_while(st, t, still)),
                None => drop(shared.changed.wait_while(st, still)),
            }
        };
        if ended && pending.is_none() {
            if looping && playing {
                ended = false;
                match stream.seek(Duration::ZERO) {
                    Ok(frame) => pending = frame,
                    Err(e) => {
                        shared.set_status(VideoStatus::Failed(e));
                        return;
                    }
                }
                // The first frame comes a frame's time after the last.
                anchor = pending
                    .as_ref()
                    .map(|f| (last_shown + stream.frame_time, f.time));
                continue;
            }
            {
                let mut st = shared.lock();
                if st.generation == generation {
                    st.playing = false;
                }
            }
            if shared.lock().status != VideoStatus::Ended {
                shared.set_status(VideoStatus::Ended);
            }
            wait(None);
            continue;
        }
        if !playing && !show_now {
            anchor = None;
            if shared.lock().status == VideoStatus::Playing {
                shared.set_status(VideoStatus::Paused);
            }
            wait(None);
            continue;
        }
        if pending.is_none() {
            match stream.next() {
                Ok(Some(f)) => pending = Some(f),
                Ok(None) => {
                    ended = true;
                    show_now = false;
                    continue;
                }
                Err(e) => {
                    shared.set_status(VideoStatus::Failed(e));
                    return;
                }
            }
        }
        let frame = pending.as_ref().expect("a pending frame");
        let now = Instant::now();
        if !show_now {
            let (at, media) = *anchor.get_or_insert((now, frame.time));
            let due = at + frame.time.saturating_sub(media);
            if now < due {
                wait(Some(due - now));
                continue;
            }
            if now > due + LATE && now - last_shown < Duration::from_millis(250) {
                // Behind: skip this one.
                pending = None;
                continue;
            }
        } else if playing {
            anchor = Some((now, frame.time));
        }
        show_now = false;
        let frame = pending.take().expect("a pending frame");
        {
            let mut st = shared.lock();
            if st.size.width == 0 {
                st.size = frame.image.size();
            }
        }
        shared.show(VideoFrame {
            timestamp: frame.time,
            image: frame.image,
        });
        last_shown = now;
        let status = if playing {
            VideoStatus::Playing
        } else {
            VideoStatus::Paused
        };
        if shared.lock().status != status {
            shared.set_status(status);
        }
    }
}
