#!/usr/bin/env python3
"""Rebuild the tagged TrueAudio, WMA and DSF fixtures in this folder.

The audio is two seconds of silence from ffmpeg (TTA, WMA) or a few
milliseconds of DSD silence written here (DSF). The tags are written by
mutagen, the library Quod Libet uses, so the readers are tested against a
real tagger's output rather than against tags Crescendo wrote itself.

Needs ffmpeg and mutagen (`python3-mutagen`). Run from anywhere:

    python3 crates/core/tests/fixtures/formats/make_tagged.py
"""

import os
import struct
import subprocess
import tempfile
import zlib

from mutagen.asf import ASF, ASFByteArrayAttribute, ASFDWordAttribute, ASFUnicodeAttribute
from mutagen.dsf import DSF
from mutagen.id3 import APIC, ID3, TALB, TCOM, TCON, TDRC, TIT2, TPE1, TPE2, TPOS, TRCK, TSO2, TSOP, TSOT, TXXX, USLT
from mutagen.trueaudio import TrueAudio

HERE = os.path.dirname(os.path.abspath(__file__))
LYRICS = "[00:00.50]静かな夜"


def png():
    """A 2x2 red PNG."""
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    raw = b"".join(b"\x00" + b"\xc8\x1e\x1e" * 2 for _ in range(2))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def id3_frames(tags):
    tags.add(TIT2(encoding=3, text="幽雅に咲かせ、墨染の桜"))
    tags.add(TPE1(encoding=3, text="上海アリス幻樂団"))
    tags.add(TALB(encoding=3, text="東方妖々夢"))
    tags.add(TPE2(encoding=3, text="ZUN"))
    tags.add(TSOT(encoding=3, text="Yuuga ni Sakase, Sumizome no Sakura"))
    tags.add(TSOP(encoding=3, text="Shanghai Alice Gengakudan"))
    tags.add(TSO2(encoding=3, text="ZUN"))
    tags.add(TCON(encoding=3, text=["Touhou", "Game"]))
    tags.add(TCOM(encoding=3, text="ZUN"))
    tags.add(TRCK(encoding=3, text="3/12"))
    tags.add(TPOS(encoding=3, text="1/2"))
    tags.add(TDRC(encoding=3, text="2003-08-17"))
    tags.add(TXXX(encoding=3, desc="VOCALIST", text="なし"))
    tags.add(USLT(encoding=3, lang="jpn", desc="", text=LYRICS))
    tags.add(APIC(encoding=3, mime="image/png", type=3, desc="", data=png()))


def ffmpeg(args, out):
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
                    "anullsrc=r=44100:cl=stereo", "-t", "2", *args, out], check=True)


def tta():
    out = os.path.join(HERE, "tagged.tta")
    # ffmpeg still writes an APEv2 tag holding only its encoder name, as real
    # ffmpeg-made files have; the reader must look past it to the ID3v2 tag.
    ffmpeg(["-map_metadata", "-1", "-sample_fmt", "s16", "-c:a", "tta"], out)
    audio = TrueAudio(out)
    audio.add_tags()
    id3_frames(audio.tags)
    audio.save()


def wma():
    out = os.path.join(HERE, "tagged.wma")
    ffmpeg(["-map_metadata", "-1", "-c:a", "wmav2", "-b:a", "32k"], out)
    audio = ASF(out)
    audio["Title"] = "幽雅に咲かせ、墨染の桜"
    audio["Author"] = "上海アリス幻樂団"
    audio["WM/AlbumTitle"] = "東方妖々夢"
    audio["WM/AlbumArtist"] = "ZUN"
    audio["WM/TitleSortOrder"] = "Yuuga ni Sakase, Sumizome no Sakura"
    audio["WM/ArtistSortOrder"] = "Shanghai Alice Gengakudan"
    audio["WM/Genre"] = ["Touhou", "Game"]
    audio["WM/Composer"] = "ZUN"
    audio["WM/TrackNumber"] = [ASFDWordAttribute(3)]
    audio["WM/PartOfSet"] = "1/2"
    audio["WM/Year"] = "2003"
    audio["WM/Lyrics"] = LYRICS
    # Fields with no Windows Media name: foobar2000 writes them bare,
    # Quod Libet with a QL/ prefix.
    audio["VOCALIST"] = "なし"
    audio["QL/event"] = "C64"
    # A language index puts the value in the Metadata Library object.
    audio["WM/Writer"] = [ASFUnicodeAttribute("ZUN", language=1)]
    pic = (bytes([3]) + struct.pack("<I", len(png())) + "image/png\0".encode("utf-16-le")
           + "\0".encode("utf-16-le") + png())
    audio["WM/Picture"] = [ASFByteArrayAttribute(pic)]
    audio.save()


def dsf():
    out = os.path.join(HERE, "tagged.dsf")
    channels, rate, block = 2, 2822400, 4096
    blocks = 4  # per channel: 4 * 4096 bytes * 8 bits = 131072 samples
    samples = blocks * block * 8
    data = b"\x69" * (block * blocks * channels)  # DSD silence
    fmt = struct.pack("<4sQIIIIIIQII", b"fmt ", 52, 1, 0, 2, channels, rate, 1, samples, block, 0)
    body = fmt + struct.pack("<4sQ", b"data", 12 + len(data)) + data
    total = 28 + len(body)
    with open(out, "wb") as f:
        f.write(struct.pack("<4sQQQ", b"DSD ", 28, total, 0) + body)
    audio = DSF(out)
    audio.add_tags()
    id3_frames(audio.tags)
    audio.save()


if __name__ == "__main__":
    tta()
    wma()
    dsf()
    for name in ("tagged.tta", "tagged.wma", "tagged.dsf"):
        print(name, os.path.getsize(os.path.join(HERE, name)), "bytes")
