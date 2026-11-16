//! DSF (`.dsf`), Sony's DSD stream file: a 28-byte "DSD " chunk whose last
//! field points at an ID3v2 tag at the end of the file, a "fmt " chunk with
//! the rate and the per-channel sample count, then the 1-bit audio.

use super::{id3v2_at, invalid, parse_tags, u32_le, u64_le, DSF};
use lofty::config::ParseOptions;
use lofty::error::FileParseError;
use lofty::file::FileType;
use lofty::id3::v2::Id3v2Tag;
use lofty::properties::FileProperties;
use lofty::resolve::FileResolver;
use lofty::tag::{TagSupport, TagType};
use lofty::LoftyFile;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

#[derive(LoftyFile, Default)]
#[lofty(read_fn = "Self::read")]
#[lofty(file_type = "DSF")]
pub(crate) struct DsfFile {
    #[lofty(tag_type = "Id3v2")]
    id3v2_tag: Option<Id3v2Tag>,
    properties: FileProperties,
}

impl FileResolver for DsfFile {
    fn extension() -> Option<&'static str> {
        Some("dsf")
    }

    fn primary_tag_type() -> TagType {
        TagType::Id3v2
    }

    fn tag_support(tag_type: TagType) -> TagSupport {
        match tag_type {
            TagType::Id3v2 => TagSupport::ReadOnly,
            _ => TagSupport::Unsupported,
        }
    }

    fn guess(buf: &[u8]) -> Option<FileType> {
        buf.starts_with(b"DSD ").then_some(DSF)
    }
}

impl DsfFile {
    fn read<R: Read + Seek + ?Sized>(
        reader: &mut R,
        opts: ParseOptions,
    ) -> Result<Self, FileParseError> {
        let len = reader.seek(SeekFrom::End(0))?;
        let mut head = [0u8; 28 + 52];
        reader.seek(SeekFrom::Start(0))?;
        reader.read_exact(&mut head)?;
        if &head[..4] != b"DSD " || &head[28..32] != b"fmt " {
            return Err(invalid("not a DSF file").into());
        }
        let metadata = u64_le(&head, 20);
        let fmt = &head[28..];
        let channels = u32_le(fmt, 24);
        let rate = u32_le(fmt, 28);
        let bits = u32_le(fmt, 32);
        let samples = u64_le(fmt, 36);
        if rate == 0 {
            return Err(invalid("DSF sample rate is 0").into());
        }

        // A pointer of 0 means no tag.
        let id3v2 = if metadata > 0 {
            id3v2_at(reader, metadata, len)?
        } else {
            None
        };
        let id3v2_tag = match id3v2 {
            Some(bytes) => parse_tags(Some(bytes), Vec::new(), opts)?.0,
            None => None,
        };

        let duration = Duration::try_from_secs_f64(samples as f64 / f64::from(rate))
            .map_err(|_| invalid("DSF length out of range"))?;
        // Uncompressed: one bit per sample per channel.
        let bitrate = u64::from(rate)
            .checked_mul(u64::from(channels))
            .and_then(|n| n.checked_mul(u64::from(bits)))
            .and_then(|n| u32::try_from(n / 1000).ok());
        Ok(Self {
            id3v2_tag,
            properties: FileProperties::new(
                duration,
                bitrate,
                bitrate,
                Some(rate),
                u8::try_from(bits).ok(),
                u8::try_from(channels).ok(),
                None,
            ),
        })
    }
}
