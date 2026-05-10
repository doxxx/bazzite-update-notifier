//! Icon assets, embedded at compile time.
//!
//! We ship a single 64×64 PNG and decode it lazily into the formats each
//! consumer needs:
//!
//! - `notify-rust` accepts a path or an [`notify_rust::Image`], built from
//!   raw RGBA bytes.
//! - `ksni` accepts ARGB32 pixel buffers (network byte order: A,R,G,B).
//!
//! Loading is done once, via `OnceCell`, so subsequent calls are free.

use once_cell::sync::OnceCell;

use crate::error::{Context, Result};

/// Raw bytes of the shipped PNG. Single source of truth.
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon-update-available.png");

/// A decoded RGBA8 buffer.
#[derive(Debug, Clone)]
pub struct Rgba8 {
    pub width: u32,
    pub height: u32,
    /// Tightly-packed RGBA bytes, length = width * height * 4.
    pub bytes: Vec<u8>,
}

static DECODED: OnceCell<Rgba8> = OnceCell::new();

/// Decode the embedded PNG into an RGBA8 buffer. Cached for the process
/// lifetime — the icon is small (~2.5 KB) but decoding it once keeps the
/// per-cycle work zero.
pub fn rgba() -> Result<&'static Rgba8> {
    if let Some(v) = DECODED.get() {
        return Ok(v);
    }
    let img = image::load_from_memory(ICON_PNG).context("decoding embedded icon PNG")?;
    let rgba = img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let bytes = rgba.into_raw();
    let _ = DECODED.set(Rgba8 {
        width,
        height,
        bytes,
    });
    Ok(DECODED.get().expect("just set"))
}

/// Convert the decoded RGBA8 to ARGB32 with bytes in network/big-endian
/// order — the layout `ksni::Icon` expects.
///
/// Network byte order means: for each pixel, the bytes appear as
/// `[A, R, G, B]` regardless of host endianness.
pub fn argb32_for_ksni() -> Result<(i32, i32, Vec<u8>)> {
    let r = rgba()?;
    let mut out = Vec::with_capacity(r.bytes.len());
    for px in r.bytes.chunks_exact(4) {
        let (red, green, blue, alpha) = (px[0], px[1], px[2], px[3]);
        out.extend_from_slice(&[alpha, red, green, blue]);
    }
    Ok((r.width as i32, r.height as i32, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_png_decodes() {
        let r = rgba().unwrap();
        assert!(r.width > 0 && r.height > 0);
        assert_eq!(r.bytes.len(), (r.width * r.height * 4) as usize);
    }

    #[test]
    fn argb_conversion_roundtrips_dimensions() {
        let (w, h, bytes) = argb32_for_ksni().unwrap();
        assert!(w > 0 && h > 0);
        assert_eq!(bytes.len(), (w * h * 4) as usize);
    }
}
