//! Windows Media Audio (`.wma`), stored in an ASF container. Everything this
//! reader needs is in the header object at the start of the file: the play
//! length (File Properties), the codec (Stream Properties) and the tags, which
//! ASF keeps as named attributes in the Content Description, Extended Content
//! Description and, inside the Header Extension, Metadata and Metadata Library
//! objects (where large covers and repeated values go).
//!
//! The attributes are renamed to their Vorbis-comment equivalents
//! (`WM/AlbumTitle` → `ALBUM`) and returned as a [`VorbisComments`] tag, so
//! lofty's Vorbis mapping turns them into the same fields as a FLAC file's.

use super::{invalid, read_at, u16_le, u32_le, u64_le, ASF};
use lofty::config::ParseOptions;
use lofty::error::FileParseError;
use lofty::file::FileType;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::OggPictureStorage;
use lofty::picture::{MimeType, Picture, PictureInformation, PictureType};
use lofty::properties::FileProperties;
use lofty::resolve::FileResolver;
use lofty::tag::{TagSupport, TagType};
use lofty::LoftyFile;
use std::io::{self, Read, Seek, SeekFrom};
use std::time::Duration;

#[derive(LoftyFile, Default)]
#[lofty(read_fn = "Self::read")]
#[lofty(file_type = "ASF")]
pub(crate) struct AsfFile {
    #[lofty(tag_type = "VorbisComments")]
    pub(crate) vorbis_comments_tag: Option<VorbisComments>,
    properties: FileProperties,
    /// The name of the audio codec, for the Format column.
    pub(crate) codec: &'static str,
}

impl FileResolver for AsfFile {
    fn extension() -> Option<&'static str> {
        Some("wma")
    }

    fn primary_tag_type() -> TagType {
        TagType::VorbisComments
    }

    fn tag_support(tag_type: TagType) -> TagSupport {
        match tag_type {
            TagType::VorbisComments => TagSupport::ReadOnly,
            _ => TagSupport::Unsupported,
        }
    }

    fn guess(buf: &[u8]) -> Option<FileType> {
        buf.starts_with(&HEADER).then_some(ASF)
    }
}

/// ASF object GUIDs as they appear in the file (the first three fields of a
/// GUID are stored little-endian).
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const HEADER: [u8; 16] = guid(
    0x75B2_2630,
    0x668E,
    0x11CF,
    [0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C],
);
const FILE_PROPERTIES: [u8; 16] = guid(
    0x8CAB_DCA1,
    0xA947,
    0x11CF,
    [0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65],
);
const STREAM_PROPERTIES: [u8; 16] = guid(
    0xB7DC_0791,
    0xA9B7,
    0x11CF,
    [0x8E, 0xE6, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65],
);
const AUDIO_MEDIA: [u8; 16] = guid(
    0xF869_9E40,
    0x5B4D,
    0x11CF,
    [0xA8, 0xFD, 0x00, 0x80, 0x5F, 0x5C, 0x44, 0x2B],
);
const CONTENT_DESCRIPTION: [u8; 16] = guid(
    0x75B2_2633,
    0x668E,
    0x11CF,
    [0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C],
);
const EXTENDED_CONTENT_DESCRIPTION: [u8; 16] = guid(
    0xD2D0_A440,
    0xE307,
    0x11D2,
    [0x97, 0xF0, 0x00, 0xA0, 0xC9, 0x5E, 0xA8, 0x50],
);
const HEADER_EXTENSION: [u8; 16] = guid(
    0x5FBF_03B5,
    0xA92E,
    0x11CF,
    [0x8E, 0xE3, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65],
);
const METADATA: [u8; 16] = guid(
    0xC5F8_CBEA,
    0x5BAF,
    0x4877,
    [0x84, 0x67, 0xAA, 0x8C, 0x44, 0xFA, 0x4C, 0xCA],
);
const METADATA_LIBRARY: [u8; 16] = guid(
    0x4423_1C94,
    0x9498,
    0x49D1,
    [0xA1, 0x41, 0x1D, 0x13, 0x4E, 0x45, 0x70, 0x54],
);

/// One ASF attribute value.
enum Value {
    Text(String),
    Bytes(Vec<u8>),
    Number(u64),
}

impl Value {
    /// Data type codes shared by every attribute list.
    fn parse(kind: u16, data: &[u8]) -> Option<Self> {
        Some(match kind {
            0 => Value::Text(utf16(data)),
            1 => Value::Bytes(data.to_vec()),
            2 | 3 if data.len() >= 4 => Value::Number(u64::from(u32_le(data, 0))),
            // BOOL is 2 bytes in the metadata objects.
            2 if data.len() >= 2 => Value::Number(u64::from(u16_le(data, 0))),
            4 if data.len() >= 8 => Value::Number(u64_le(data, 0)),
            5 if data.len() >= 2 => Value::Number(u64::from(u16_le(data, 0))),
            _ => return None,
        })
    }

    fn text(&self) -> Option<String> {
        match self {
            Value::Text(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bytes(_) => None,
        }
    }
}

/// UTF-16LE text, up to the first NUL.
fn utf16(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// A bounds-checked cursor over the header bytes.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if n > self.0.len() {
            return Err(invalid("ASF object runs past its end"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16_le(self.take(2)?, 0))
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32_le(self.take(4)?, 0))
    }

    /// The child objects of a list of objects: (GUID, body).
    fn objects(mut self) -> io::Result<Vec<([u8; 16], &'a [u8])>> {
        let mut out = Vec::new();
        while self.0.len() >= 24 {
            let id: [u8; 16] = self.0[..16].try_into().unwrap();
            let size = u64_le(self.0, 16);
            if size < 24 || size > self.0.len() as u64 {
                return Err(invalid("ASF object size out of range"));
            }
            let object = self.take(size as usize)?;
            out.push((id, &object[24..]));
        }
        Ok(out)
    }
}

/// Where each ASF attribute goes in Vorbis-comment terms. Names follow
/// Windows Media Player, foobar2000 and Quod Libet (via mutagen).
fn vorbis_key(name: &str) -> Option<String> {
    let key = match name {
        "Title" => "TITLE",
        "Author" => "ARTIST",
        "WM/AlbumTitle" => "ALBUM",
        "WM/AlbumArtist" => "ALBUMARTIST",
        "WM/TitleSortOrder" => "TITLESORT",
        "WM/ArtistSortOrder" => "ARTISTSORT",
        "WM/AlbumArtistSortOrder" => "ALBUMARTISTSORT",
        "WM/AlbumSortOrder" => "ALBUMSORT",
        "WM/Genre" => "GENRE",
        "WM/ContentGroupDescription" => "GROUPING",
        "WM/Composer" => "COMPOSER",
        "WM/Writer" => "LYRICIST",
        "WM/Conductor" => "CONDUCTOR",
        "WM/Producer" => "PRODUCER",
        "WM/ModifiedBy" => "REMIXER",
        "WM/Publisher" => "LABEL",
        "WM/CatalogNo" => "CATALOGNUMBER",
        "WM/ISRC" => "ISRC",
        "WM/TrackNumber" => "TRACKNUMBER",
        "WM/PartOfSet" => "DISCNUMBER",
        "WM/SetSubTitle" => "DISCSUBTITLE",
        "WM/Year" => "DATE",
        "WM/OriginalReleaseYear" => "ORIGINALDATE",
        "WM/BeatsPerMinute" => "BPM",
        "WM/Lyrics" => "LYRICS",
        "WM/Mood" => "MOOD",
        "WM/EncodedBy" => "ENCODEDBY",
        "Copyright" => "COPYRIGHT",
        "Description" => "COMMENT",
        _ => {
            // Fields without a Windows Media name are stored under their own:
            // foobar2000 writes them bare ("VOCALIST"), Quod Libet as
            // "QL/vocalist". Other WM/ attributes (codec and DRM data, play
            // counts kept by WMP) are not tags.
            let bare = name.strip_prefix("QL/").unwrap_or(name);
            if name.starts_with("WM/")
                || bare.is_empty()
                || !bare
                    .bytes()
                    .all(|b| (0x20..=0x7D).contains(&b) && b != b'=')
            {
                return None;
            }
            return Some(bare.to_ascii_uppercase());
        }
    };
    Some(key.to_string())
}

/// A `WM/Picture` value: type byte, data length, MIME type and description
/// (both UTF-16, NUL-terminated), then the image.
fn picture(data: &[u8]) -> Option<Picture> {
    let mut b = Bytes(data);
    let kind = b.take(1).ok()?[0];
    let len = b.u32().ok()? as usize;
    let mime = nul_utf16(&mut b)?;
    nul_utf16(&mut b)?;
    let image = b.take(len).ok()?;
    if image.is_empty() {
        return None;
    }
    Some(
        Picture::unchecked(image.to_vec())
            .pic_type(PictureType::from_u8(kind))
            .mime_type(MimeType::from_str(&mime))
            .build(),
    )
}

/// A NUL-terminated UTF-16LE string.
fn nul_utf16(b: &mut Bytes) -> Option<String> {
    let mut units = Vec::new();
    loop {
        let unit = b.u16().ok()?;
        if unit == 0 {
            return Some(String::from_utf16_lossy(&units));
        }
        units.push(unit);
    }
}

#[derive(Default)]
struct Attributes {
    /// (name, value) in file order.
    items: Vec<(String, Value)>,
}

impl Attributes {
    fn content_description(&mut self, body: &[u8]) -> io::Result<()> {
        let mut b = Bytes(body);
        let lens = [b.u16()?, b.u16()?, b.u16()?, b.u16()?, b.u16()?];
        for (name, len) in ["Title", "Author", "Copyright", "Description", "Rating"]
            .into_iter()
            .zip(lens)
        {
            let text = utf16(b.take(usize::from(len))?);
            if !text.is_empty() {
                self.items.push((name.to_string(), Value::Text(text)));
            }
        }
        Ok(())
    }

    fn extended_content_description(&mut self, body: &[u8]) -> io::Result<()> {
        let mut b = Bytes(body);
        for _ in 0..b.u16()? {
            let name_len = usize::from(b.u16()?);
            let name = utf16(b.take(name_len)?);
            let kind = b.u16()?;
            let value_len = usize::from(b.u16()?);
            let data = b.take(value_len)?;
            if let Some(value) = Value::parse(kind, data) {
                self.items.push((name, value));
            }
        }
        Ok(())
    }

    /// The Metadata and Metadata Library objects share one record layout.
    fn metadata(&mut self, body: &[u8]) -> io::Result<()> {
        let mut b = Bytes(body);
        for _ in 0..b.u16()? {
            let _language_or_reserved = b.u16()?;
            let _stream = b.u16()?;
            let name_len = usize::from(b.u16()?);
            let kind = b.u16()?;
            let data_len = b.u32()? as usize;
            let name = utf16(b.take(name_len)?);
            let data = b.take(data_len)?;
            if let Some(value) = Value::parse(kind, data) {
                self.items.push((name, value));
            }
        }
        Ok(())
    }

    fn into_vorbis(self) -> VorbisComments {
        let mut vc = VorbisComments::default();
        let has_track_number = self.items.iter().any(|(n, _)| n == "WM/TrackNumber");
        for (name, value) in self.items {
            if name == "WM/Picture" {
                if let Value::Bytes(data) = &value {
                    if let Some(picture) = picture(data) {
                        // The dimensions in PictureInformation are not used,
                        // so the image is not decoded here.
                        let _ = vc.insert_picture(picture, Some(PictureInformation::default()));
                    }
                }
                continue;
            }
            // WM/Track is the old zero-based track number.
            if name == "WM/Track" {
                if let (false, Value::Number(n)) = (has_track_number, &value) {
                    vc.push("TRACKNUMBER".into(), (n + 1).to_string());
                }
                continue;
            }
            let (Some(key), Some(text)) = (vorbis_key(&name), value.text()) else {
                continue;
            };
            if !text.trim().is_empty() {
                vc.push(key, text);
            }
        }
        vc
    }
}

impl AsfFile {
    fn read<R: Read + Seek + ?Sized>(
        reader: &mut R,
        _opts: ParseOptions,
    ) -> Result<Self, FileParseError> {
        let len = reader.seek(SeekFrom::End(0))?;
        let mut top = [0u8; 30];
        reader.seek(SeekFrom::Start(0))?;
        reader.read_exact(&mut top)?;
        if top[..16] != HEADER {
            return Err(invalid("not an ASF file").into());
        }
        let header_size = u64_le(&top, 16);
        if header_size < 30 || header_size > len {
            return Err(invalid("ASF header size out of range").into());
        }
        let header = read_at(reader, 30, header_size - 30)?;

        // Attribute lists are read in a fixed order, not file order: the
        // Extended Content Description holds one value per name, so taggers
        // put a field's further values in the Metadata Library, which often
        // comes first in the file.
        let mut content = None;
        let mut extended = None;
        let mut metadata = Vec::new();
        let mut library = Vec::new();
        let mut duration = Duration::ZERO;
        let mut bitrate = None;
        let mut sample_rate = None;
        let mut bit_depth = None;
        let mut channels = None;
        let mut codec = "WMA";
        for (id, body) in Bytes(&header).objects()? {
            match id {
                FILE_PROPERTIES if body.len() >= 80 => {
                    let play_100ns = u64_le(body, 40);
                    let preroll_ms = u64_le(body, 56);
                    let broadcast = u32_le(body, 64) & 1 != 0;
                    if !broadcast {
                        let ms = (play_100ns / 10_000).saturating_sub(preroll_ms);
                        duration = Duration::from_millis(ms);
                    }
                    bitrate = Some(u32_le(body, 76) / 1000).filter(|b| *b > 0);
                }
                STREAM_PROPERTIES if body.len() >= 54 + 16 && body[..16] == AUDIO_MEDIA => {
                    // WAVEFORMATEX follows the 54-byte fixed part.
                    let wave = &body[54..];
                    codec = match u16_le(wave, 0) {
                        0x0162 => "WMA Pro",
                        0x0163 => "WMA Lossless",
                        0x000A => "WMA Voice",
                        _ => "WMA",
                    };
                    channels = u8::try_from(u16_le(wave, 2)).ok();
                    sample_rate = Some(u32_le(wave, 4));
                    bit_depth = u8::try_from(u16_le(wave, 14)).ok().filter(|b| *b > 0);
                }
                CONTENT_DESCRIPTION => content = Some(body),
                EXTENDED_CONTENT_DESCRIPTION => extended = Some(body),
                HEADER_EXTENSION if body.len() >= 22 => {
                    for (id, inner) in Bytes(&body[22..]).objects()? {
                        match id {
                            METADATA => metadata.push(inner),
                            METADATA_LIBRARY => library.push(inner),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        let mut attributes = Attributes::default();
        if let Some(body) = content {
            attributes.content_description(body)?;
        }
        if let Some(body) = extended {
            attributes.extended_content_description(body)?;
        }
        for body in metadata.into_iter().chain(library) {
            attributes.metadata(body)?;
        }
        let vc = attributes.into_vorbis();
        Ok(Self {
            vorbis_comments_tag: Some(vc),
            properties: FileProperties::new(
                duration,
                bitrate,
                bitrate,
                sample_rate,
                bit_depth,
                channels,
                None,
            ),
            codec,
        })
    }
}
