//! The monitor's refresh rate, from RandR, to pace the frame clock.
//!
//! X11 has no frame callback without the Present extension, so animation frames
//! come from a timer at the refresh interval (see [`crate::platform::frame`]).
//! This asks RandR for the modes of the active CRTCs and takes the fastest.

use std::io::{self, Read};
use std::time::Duration;

use super::{Conn, finish, query_extension, rd_u16, rd_u32};

const RR_QUERY_VERSION: u8 = 0;
const RR_GET_SCREEN_RESOURCES: u8 = 8;
const RR_GET_CRTC_INFO: u8 = 20;
const RR_GET_SCREEN_RESOURCES_CURRENT: u8 = 25;

const MODE_INTERLACE: u32 = 0x10;
const MODE_DOUBLE_SCAN: u32 = 0x20;

/// The refresh interval of the fastest active monitor, or `None` if RandR is
/// missing or reports nothing usable. Call before any window is mapped, so the
/// replies don't interleave with events.
pub(super) fn interval(conn: &mut Conn) -> Option<Duration> {
    let major = query_extension(conn, b"RANDR").ok()??;
    let hz = fastest_refresh(conn, major).ok()??;
    (hz.is_finite() && (10.0..=500.0).contains(&hz)).then(|| Duration::from_secs_f64(1.0 / hz))
}

/// Send a request and read its reply: the 32-byte header plus any extra words.
/// `None` for an error packet.
fn round_trip(conn: &mut Conn, req: Vec<u8>) -> io::Result<Option<Vec<u8>>> {
    conn.send(&finish(req))?;
    let mut head = [0u8; 32];
    conn.stream.read_exact(&mut head)?;
    if head[0] != 1 {
        return Ok(None); // an error: 32 bytes, nothing follows
    }
    let extra = rd_u32(&head, 4).unwrap_or(0) as usize * 4;
    let mut reply = head.to_vec();
    reply.resize(32 + extra, 0);
    conn.stream.read_exact(&mut reply[32..])?;
    Ok(Some(reply))
}

fn fastest_refresh(conn: &mut Conn, major: u8) -> io::Result<Option<f64>> {
    // Clients must state the version they speak before other requests.
    let mut req = vec![major, RR_QUERY_VERSION, 0, 0];
    req.extend_from_slice(&1u32.to_le_bytes());
    req.extend_from_slice(&5u32.to_le_bytes());
    let Some(version) = round_trip(conn, req)? else {
        return Ok(None);
    };
    let minor = rd_u32(&version, 12).unwrap_or(0);
    let resources_op = if minor >= 3 {
        RR_GET_SCREEN_RESOURCES_CURRENT // doesn't poll the hardware
    } else {
        RR_GET_SCREEN_RESOURCES
    };
    let mut req = vec![major, resources_op, 0, 0];
    req.extend_from_slice(&conn.setup.root.to_le_bytes());
    let Some(res) = round_trip(conn, req)? else {
        return Ok(None);
    };
    let (Some(config_time), Some(crtcs), Some(outputs), Some(modes)) = (
        rd_u32(&res, 12),
        rd_u16(&res, 16),
        rd_u16(&res, 18),
        rd_u16(&res, 20),
    ) else {
        return Ok(None);
    };
    let crtc_ids: Vec<u32> = (0..crtcs as usize)
        .filter_map(|i| rd_u32(&res, 32 + i * 4))
        .collect();
    let modes_at = 32 + (crtcs as usize + outputs as usize) * 4;
    let mode_info = |id: u32| {
        (0..modes as usize)
            .map(|i| modes_at + i * 32)
            .find(|&at| rd_u32(&res, at) == Some(id))
            .and_then(|at| mode_refresh(res.get(at..at + 32)?))
    };

    let mut fastest: Option<f64> = None;
    for crtc in crtc_ids {
        let mut req = vec![major, RR_GET_CRTC_INFO, 0, 0];
        req.extend_from_slice(&crtc.to_le_bytes());
        req.extend_from_slice(&config_time.to_le_bytes());
        let Some(info) = round_trip(conn, req)? else {
            continue;
        };
        let mode = rd_u32(&info, 20).unwrap_or(0);
        if mode != 0
            && let Some(hz) = mode_info(mode)
        {
            fastest = Some(fastest.map_or(hz, |f: f64| f.max(hz)));
        }
    }
    Ok(fastest)
}

/// The refresh rate of a 32-byte RandR `MODEINFO`, in Hz.
fn mode_refresh(mode: &[u8]) -> Option<f64> {
    let dot_clock = rd_u32(mode, 8)? as f64;
    let htotal = rd_u16(mode, 16)? as f64;
    let mut vtotal = rd_u16(mode, 24)? as f64;
    let flags = rd_u32(mode, 28)?;
    if flags & MODE_DOUBLE_SCAN != 0 {
        vtotal *= 2.0;
    }
    if flags & MODE_INTERLACE != 0 {
        vtotal /= 2.0;
    }
    (dot_clock > 0.0 && htotal > 0.0 && vtotal > 0.0).then(|| dot_clock / (htotal * vtotal))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modeinfo(dot_clock: u32, htotal: u16, vtotal: u16, flags: u32) -> Vec<u8> {
        let mut m = vec![0u8; 32];
        m[8..12].copy_from_slice(&dot_clock.to_le_bytes());
        m[16..18].copy_from_slice(&htotal.to_le_bytes());
        m[24..26].copy_from_slice(&vtotal.to_le_bytes());
        m[28..32].copy_from_slice(&flags.to_le_bytes());
        m
    }

    #[test]
    fn refresh_from_mode_timings() {
        // 1920x1080@60: 148.5 MHz over 2200 x 1125.
        let hz = mode_refresh(&modeinfo(148_500_000, 2200, 1125, 0)).unwrap();
        assert!((hz - 60.0).abs() < 0.01, "{hz}");
        let hz = mode_refresh(&modeinfo(148_500_000, 2200, 1125, MODE_INTERLACE)).unwrap();
        assert!((hz - 120.0).abs() < 0.02, "interlaced fields: {hz}");
        assert_eq!(mode_refresh(&modeinfo(0, 2200, 1125, 0)), None, "no clock");
    }
}
