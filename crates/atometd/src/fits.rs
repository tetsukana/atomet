//! FITS file primitives — fixed-form 80-char cards, 2880-byte block padding,
//! and image-HDU header builders. Supports BITPIX=8 (uint8) 2-D images.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

const CARD: usize = 80;
const BLOCK: usize = 2880;

/// Format a free-form 80-char FITS card from a pre-formatted string.
/// Caller is responsible for FITS column conventions when used directly;
/// prefer the typed helpers (`card_int`, `card_str`, etc.) where possible.
pub fn make_card(s: &str) -> Vec<u8> {
    let mut buf = vec![b' '; CARD];
    let bytes = s.as_bytes();
    let len = bytes.len().min(CARD);
    buf[..len].copy_from_slice(&bytes[..len]);
    buf
}

/// `KEYWORD = VALUE` (value right-justified in cols 11–30).
pub fn card_int(key: &str, value: i64) -> Vec<u8> {
    make_card(&format!("{:<8}= {:>20}", key, value))
}

pub fn card_bool(key: &str, value: bool) -> Vec<u8> {
    let v = if value { "T" } else { "F" };
    make_card(&format!("{:<8}= {:>20}", key, v))
}

pub fn card_float(key: &str, value: f64) -> Vec<u8> {
    make_card(&format!("{:<8}= {:>20.6}", key, value))
}

/// `KEYWORD = 'VALUE   '` — string value, body padded to ≥8 chars.
pub fn card_str(key: &str, value: &str) -> Vec<u8> {
    let body = if value.len() < 8 {
        format!("{:<8}", value)
    } else {
        value.to_string()
    };
    make_card(&format!("{:<8}= '{}'", key, body))
}

pub fn end_card() -> Vec<u8> {
    make_card("END")
}

/// Pad to a 2880-byte block boundary with ASCII spaces (header padding).
pub fn pad_header(mut v: Vec<u8>) -> Vec<u8> {
    let pad = (BLOCK - (v.len() % BLOCK)) % BLOCK;
    v.extend(vec![b' '; pad]);
    v
}

/// Pad to a 2880-byte block boundary with zero bytes (data padding).
pub fn pad_data(mut v: Vec<u8>) -> Vec<u8> {
    let pad = (BLOCK - (v.len() % BLOCK)) % BLOCK;
    v.extend(vec![0u8; pad]);
    v
}

/// Build the primary HDU header for a 2-D BITPIX=8 image.
/// `has_extensions = true` adds `EXTEND = T`.
pub fn primary_image_header(
    width: u32,
    height: u32,
    has_extensions: bool,
    extra_cards: &[Vec<u8>],
) -> Vec<u8> {
    let mut hdr = Vec::with_capacity(BLOCK);
    hdr.extend(card_bool("SIMPLE", true));
    hdr.extend(card_int("BITPIX", 8));
    hdr.extend(card_int("NAXIS", 2));
    hdr.extend(card_int("NAXIS1", width as i64));
    hdr.extend(card_int("NAXIS2", height as i64));
    if has_extensions {
        hdr.extend(card_bool("EXTEND", true));
    }
    for c in extra_cards {
        hdr.extend(c);
    }
    hdr.extend(end_card());
    pad_header(hdr)
}

/// Build an `IMAGE` extension HDU header for a 2-D BITPIX=8 image.
pub fn image_extension_header(
    width: u32,
    height: u32,
    extname: &str,
    extra_cards: &[Vec<u8>],
) -> Vec<u8> {
    let mut hdr = Vec::with_capacity(BLOCK);
    hdr.extend(card_str("XTENSION", "IMAGE"));
    hdr.extend(card_int("BITPIX", 8));
    hdr.extend(card_int("NAXIS", 2));
    hdr.extend(card_int("NAXIS1", width as i64));
    hdr.extend(card_int("NAXIS2", height as i64));
    hdr.extend(card_int("PCOUNT", 0));
    hdr.extend(card_int("GCOUNT", 1));
    hdr.extend(card_str("EXTNAME", extname));
    for c in extra_cards {
        hdr.extend(c);
    }
    hdr.extend(end_card());
    pad_header(hdr)
}

/// Atomically write `contents` to `path`: create parent dirs, write to a
/// sibling `.tmp` file, fsync, then rename into place.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("fits.tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}
