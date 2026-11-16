//! TrueAudio (`.tta`): an optional ID3v2 tag, the 22-byte "TTA1" header,
//! the audio, then optional APEv2 and ID3v1 tags. foobar2000 writes APEv2,
//! Quod Libet and Mp3tag write ID3v2; the APEv2 tag is read first when a file
//! has both, unless it holds nothing Crescendo reads.

use super::{id3v2_at, invalid, parse_tags, tail_tags, u16_le, u32_le, TTA};
use lofty::ape::ApeTag;
use lofty::config::ParseOptions;
use lofty::error::FileParseError;
use lofty::file::FileType;
use lofty::id3::v1::Id3v1Tag;
use lofty::id3::v2::Id3v2Tag;
use lofty::properties::FileProperties;
use lofty::resolve::FileResolver;
use lofty::tag::{TagSupport, TagType};
use lofty::LoftyFile;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

#[derive(LoftyFile, Default)]
#[lofty(read_fn = "Self::read")]
#[lofty(file_type = "TTA")]
pub(crate) struct TtaFile {
    // Field order is the order of `TaggedFile::tags`: APEv2 first.
    #[lofty(tag_type = "Ape")]
    pub(crate) ape_tag: Option<ApeTag>,
    #[lofty(tag_type = "Id3v2")]
    id3v2_tag: Option<Id3v2Tag>,
    #[lofty(tag_type = "Id3v1")]
    id3v1_tag: Option<Id3v1Tag>,
    properties: FileProperties,
}

impl FileResolver for TtaFile {
    fn extension() -> Option<&'static str> {
        Some("tta")
    }

    fn primary_tag_type() -> TagType {
        TagType::Ape
    }

    fn tag_support(tag_type: TagType) -> TagSupport {
        match tag_type {
            TagType::Ape | TagType::Id3v2 | TagType::Id3v1 => TagSupport::ReadOnly,
            _ => TagSupport::Unsupported,
        }
    }

    fn guess(buf: &[u8]) -> Option<FileType> {
        buf.starts_with(b"TTA1").then_some(TTA)
    }
}

fn has_fields(ape: &ApeTag) -> bool {
    use lofty::tag::TagExt;
    let vocalist_or_event = ape.get("VOCALIST").is_some() || ape.get("EVENT").is_some();
    vocalist_or_event
        || crate::scanner::tags::ape_cover(ape).is_some()
        || !lofty::tag::Tag::from(ape.clone()).is_empty()
}

impl TtaFile {
    fn read<R: Read + Seek + ?Sized>(
        reader: &mut R,
        opts: ParseOptions,
    ) -> Result<Self, FileParseError> {
        let len = reader.seek(SeekFrom::End(0))?;
        let id3v2 = id3v2_at(reader, 0, len)?;
        let audio_start = id3v2.as_ref().map_or(0, |tag| tag.len() as u64);

        let mut header = [0u8; 22];
        reader.seek(SeekFrom::Start(audio_start))?;
        reader.read_exact(&mut header)?;
        if &header[..4] != b"TTA1" {
            return Err(invalid("no TTA1 header").into());
        }
        let channels = u16_le(&header, 6);
        let bits = u16_le(&header, 8);
        let rate = u32_le(&header, 10);
        let samples = u32_le(&header, 14);
        if rate == 0 {
            return Err(invalid("TTA sample rate is 0").into());
        }

        let tail = tail_tags(reader, len, audio_start + 22)?;
        let audio_bytes = len - audio_start - tail.len() as u64;
        let (id3v2_tag, ape_tag, id3v1_tag) = parse_tags(id3v2, tail, opts)?;
        // ffmpeg's muxer writes an APEv2 tag holding only `encoder=Lavf…`.
        // An APE tag with no field Crescendo reads would hide a real ID3v2
        // tag, so it only counts when it has one.
        let ape_tag = ape_tag.filter(has_fields);

        let duration = Duration::from_secs_f64(f64::from(samples) / f64::from(rate));
        let millis = duration.as_millis();
        let bitrate = (millis > 0).then(|| (audio_bytes as u128 * 8 / millis) as u32);
        Ok(Self {
            ape_tag,
            id3v2_tag,
            id3v1_tag,
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
