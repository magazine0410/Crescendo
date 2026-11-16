//! Readers for the formats lofty has none for: TrueAudio, WMA (ASF) and DSF.
//!
//! Each is registered with lofty as a custom file type, so `Probe::open`,
//! `lofty::read_from_path` and everything built on them (the scanner, the
//! cover reader, embedded lyrics) handle these files like any other once
//! [`register`] has run. The readers only find the audio properties and the
//! tag bytes; lofty parses ID3v2, APEv2 and ID3v1 itself, and ASF attributes
//! are handed over as Vorbis comments, so lofty's key mapping applies to every
//! format alike.

mod asf;
mod dsf;
mod tta;

pub(crate) use asf::AsfFile;
pub(crate) use dsf::DsfFile;
pub(crate) use tta::TtaFile;

use lofty::ape::ApeTag;
use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType};
use lofty::id3::v1::Id3v1Tag;
use lofty::id3::v2::Id3v2Tag;
use lofty::mpeg::MpegFile;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::Once;

pub(crate) const TTA: FileType = FileType::Custom("TTA");
pub(crate) const ASF: FileType = FileType::Custom("ASF");
pub(crate) const DSF: FileType = FileType::Custom("DSF");

/// The most tag data read into memory from one file. Covers are capped at
/// 32 MiB by the cover reader; this leaves room for one plus the text.
const MAX_TAG_BYTES: u64 = 64 << 20;

/// Make lofty recognise `.tta`, `.wma` and `.dsf`. Cheap after the first call.
pub fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        lofty::resolve::register_custom_resolver::<TtaFile>("TTA");
        lofty::resolve::register_custom_resolver::<AsfFile>("ASF");
        lofty::resolve::register_custom_resolver::<DsfFile>("DSF");
    });
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn u16_le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u64_le(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Read `len` bytes at `offset`, refusing anything over [`MAX_TAG_BYTES`].
fn read_at<R: Read + Seek + ?Sized>(reader: &mut R, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    if len > MAX_TAG_BYTES {
        return Err(invalid("tag larger than 64 MiB"));
    }
    reader.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0; len as usize];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// The size of an ID3v2 tag starting with `header`, header and footer
/// included, or None if `header` does not start one.
fn id3v2_len(header: &[u8; 10]) -> Option<u64> {
    if &header[..3] != b"ID3" || header[6..10].iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    let size = header[6..10]
        .iter()
        .fold(0u64, |n, b| (n << 7) | u64::from(*b));
    let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + size + footer)
}

/// The APEv2 tag and ID3v1 tag at the end of a file, as one byte range: the
/// ID3v1 tag is the last 128 bytes when they start with "TAG", and an APE tag
/// ends with a 32-byte "APETAGEX" footer right before it.
fn tail_tags<R: Read + Seek + ?Sized>(reader: &mut R, len: u64, start: u64) -> io::Result<Vec<u8>> {
    let mut end = len;
    let mut tag = [0u8; 3];
    if len >= start + 128 {
        reader.seek(SeekFrom::Start(len - 128))?;
        reader.read_exact(&mut tag)?;
        if &tag == b"TAG" {
            end = len - 128;
        }
    }
    let mut first = end;
    if end >= start + 32 {
        let footer = read_at(reader, end - 32, 32)?;
        if &footer[..8] == b"APETAGEX" {
            let size = u64::from(u32_le(&footer, 12));
            let header = if u32_le(&footer, 20) & 0x8000_0000 != 0 {
                32
            } else {
                0
            };
            let total = size + header;
            if size >= 32 && total <= end - start {
                first = end - total;
            }
        }
    }
    read_at(reader, first, len - first)
}

/// Tags lofty can parse but has no public reader for outside its own
/// formats. They are handed to lofty's MPEG reader in a small buffer: the
/// ID3v2 bytes, then spaces (no frame sync, so its search for MP3 frames
/// stops at once), then the trailing APEv2/ID3v1 bytes, which it looks for
/// from the end. Only the tags are parsed; the file itself is never scanned.
fn parse_tags(
    id3v2: Option<Vec<u8>>,
    tail: Vec<u8>,
    opts: ParseOptions,
) -> io::Result<(Option<Id3v2Tag>, Option<ApeTag>, Option<Id3v1Tag>)> {
    let mut buf = id3v2.unwrap_or_default();
    buf.extend(std::iter::repeat_n(b' ', 160));
    buf.extend(tail);
    let mut opts = opts;
    let opts = opts.read_properties(false);
    let mut mpeg = MpegFile::read_from(&mut Cursor::new(buf), opts)
        .map_err(|e| invalid(&format!("tags: {e}")))?;
    Ok((mpeg.remove_id3v2(), mpeg.remove_ape(), mpeg.remove_id3v1()))
}

/// The ID3v2 tag at `offset`, if one starts there, as raw bytes.
fn id3v2_at<R: Read + Seek + ?Sized>(
    reader: &mut R,
    offset: u64,
    len: u64,
) -> io::Result<Option<Vec<u8>>> {
    if offset.saturating_add(10) > len {
        return Ok(None);
    }
    let mut header = [0u8; 10];
    reader.seek(SeekFrom::Start(offset))?;
    reader.read_exact(&mut header)?;
    match id3v2_len(&header) {
        Some(size) if offset + size <= len => Ok(Some(read_at(reader, offset, size)?)),
        _ => Ok(None),
    }
}
