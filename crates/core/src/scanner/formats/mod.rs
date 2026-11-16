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

use lofty::ape::{ApeItem, ApeTag};
use lofty::config::ParseOptions;
use lofty::error::FileParseError;
use lofty::file::{AudioFile, FileType};
use lofty::id3::v1::Id3v1Tag;
use lofty::id3::v2::Id3v2Tag;
use lofty::mpeg::MpegFile;
use lofty::tag::ItemValue;
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
    match MpegFile::read_from(&mut Cursor::new(buf.clone()), opts) {
        Ok(mut mpeg) => Ok((mpeg.remove_id3v2(), mpeg.remove_ape(), mpeg.remove_id3v1())),
        Err(error) => {
            // An APEv2 tag lofty refuses costs only its bad items: parse the
            // other tags without it, and the APE tag leniently.
            let tail = &buf[buf.len() - tail_len(&buf)..];
            let ape = lenient_ape_tag(tail);
            // Everything before the tail, then the ID3v1 tag at its very end.
            let mut rest = buf[..buf.len() - tail.len()].to_vec();
            rest.extend_from_slice(&buf[buf.len() - id3v1_len(tail)..]);
            let mut mpeg = MpegFile::read_from(&mut Cursor::new(rest), opts)
                .map_err(|_| invalid(&format!("tags: {error}")))?;
            Ok((mpeg.remove_id3v2(), ape, mpeg.remove_id3v1()))
        }
    }
}

/// Bytes at the end of `buf` holding an APEv2 tag and an ID3v1 tag, as far
/// as they are present.
fn tail_len(buf: &[u8]) -> usize {
    let id3v1 = id3v1_len(buf);
    let end = buf.len() - id3v1;
    if end >= 32 && &buf[end - 32..end - 24] == b"APETAGEX" {
        let size = u32_le(buf, end - 20) as usize;
        let header = if u32_le(buf, end - 12) & 0x8000_0000 != 0 {
            32
        } else {
            0
        };
        if size >= 32 && size + header <= end {
            return id3v1 + size + header;
        }
    }
    id3v1
}

/// 128 when `buf` ends with an ID3v1 tag, else 0.
fn id3v1_len(buf: &[u8]) -> usize {
    if buf.len() >= 128 && &buf[buf.len() - 128..buf.len() - 125] == b"TAG" {
        128
    } else {
        0
    }
}

/// The APEv2 tag at the end of `buf` (before an ID3v1 tag, if any), keeping
/// every item lofty accepts and dropping the rest. lofty refuses a whole tag,
/// and with it the whole file, when one item's name breaks the format (APE
/// names are ASCII 0x20–0x7E; some localised taggers write a translated
/// "Cover Art (Front)"), so this is the fallback for such files.
pub(crate) fn lenient_ape_tag(buf: &[u8]) -> Option<ApeTag> {
    let end = buf.len() - id3v1_len(buf);
    if end < 32 || &buf[end - 32..end - 24] != b"APETAGEX" {
        return None;
    }
    let size = u32_le(buf, end - 20) as usize;
    let count = u32_le(buf, end - 16) as usize;
    if size < 32 || size > end {
        return None;
    }
    let items = &buf[end - size..end - 32];
    let mut tag = ApeTag::new();
    let mut pos = 0;
    for _ in 0..count.min(65_536) {
        let Some(head) = items.get(pos..pos + 8) else {
            break;
        };
        let value_len = u32_le(head, 0) as usize;
        let flags = u32_le(head, 4);
        pos += 8;
        let Some(key_len) = items
            .get(pos..)
            .and_then(|rest| rest.iter().position(|b| *b == 0))
        else {
            break;
        };
        let key = String::from_utf8_lossy(&items[pos..pos + key_len]).to_string();
        pos += key_len + 1;
        let Some(value) = pos
            .checked_add(value_len)
            .and_then(|end| items.get(pos..end))
        else {
            break;
        };
        pos += value_len;
        let value = match (flags >> 1) & 3 {
            0 => ItemValue::Text(String::from_utf8_lossy(value).to_string()),
            1 => ItemValue::Binary(value.to_vec()),
            2 => ItemValue::Locator(String::from_utf8_lossy(value).to_string()),
            _ => continue,
        };
        // A name lofty refuses is the item this fallback exists to skip.
        if let Ok(item) = ApeItem::new(key, value) {
            tag.insert(item);
        }
    }
    use lofty::tag::TagExt;
    (!tag.is_empty()).then_some(tag)
}

/// Read a file whose APEv2 tag lofty may refuse (WavPack, Monkey's Audio,
/// Musepack). If the normal read fails but one without tags succeeds, the
/// failure was a tag, and the file is returned with its APE tag read
/// leniently; ID3 tags are then not read. Any other failure is returned as
/// it was.
pub(crate) fn read_tolerating_bad_ape<R, T>(
    reader: &mut R,
    opts: ParseOptions,
    read: impl Fn(&mut R, ParseOptions) -> Result<T, FileParseError>,
    set_ape: impl FnOnce(&mut T, ApeTag),
) -> Result<T, FileParseError>
where
    R: Read + Seek,
{
    reader.seek(SeekFrom::Start(0))?;
    let error = match read(reader, opts) {
        Ok(file) => return Ok(file),
        Err(error) => error,
    };
    reader.seek(SeekFrom::Start(0))?;
    let mut without_tags = opts;
    let Ok(mut file) = read(reader, without_tags.read_tags(false)) else {
        return Err(error);
    };
    let len = reader.seek(SeekFrom::End(0))?;
    if let Some(tag) = tail_tags(reader, len, 0)
        .ok()
        .and_then(|tail| lenient_ape_tag(&tail))
    {
        set_ape(&mut file, tag);
    }
    Ok(file)
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
