//! In-process tag, picture, and bitrate access through lofty.
//!
//! Reading headers in-process replaces one `ffprobe` spawn per file. Tags are carried over by
//! lofty rather than ffmpeg because lofty's generic tag model maps ID3v2, MP4 atoms, APE and
//! Vorbis comments onto Vorbis comment keys consistently, including ReplayGain, MusicBrainz IDs
//! and embedded pictures, which ffmpeg's Ogg Opus muxer cannot write.

use std::hash::{DefaultHasher, Hasher};
use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::error::{FileEncodingError, FileParseError};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Tag, TagExt, TagType};

/// What the encoder needs to know about a source file.
pub struct SourceInfo {
    /// Audio bitrate in kbit/s, falling back to the container's overall rate.
    pub bitrate_kbps: Option<u32>,
    /// The file's primary tag, pictures included.
    pub tag: Option<Tag>,
}

/// Reads properties and the primary tag. Blocking; call from `spawn_blocking`.
pub fn read(path: &Path) -> Result<SourceInfo, FileParseError> {
    let probe = match Probe::open(path) {
        Ok(probe) => probe,
        Err(e) => return Err(e),
    };
    let options = ParseOptions::new()
        .read_properties(true)
        .read_cover_art(true);
    let mut file = match probe.options(options).read() {
        Ok(file) => file,
        Err(e) => return Err(e),
    };
    let properties = file.properties();
    let bitrate_kbps = match properties.audio_bitrate() {
        Some(0) | None => properties.overall_bitrate().filter(|&rate| rate > 0),
        Some(rate) => Some(rate),
    };
    // Take the primary tag by value instead of cloning it; pictures can be megabytes.
    let primary = file.primary_tag_type();
    let tag = match file.remove(primary) {
        Some(tag) => Some(tag),
        None => file.first_tag().cloned(),
    };
    Ok(SourceInfo { bitrate_kbps, tag })
}

/// Reads only the front cover (or, failing that, the first picture) from a file's tags.
pub fn read_cover(path: &Path) -> Result<Option<Picture>, FileParseError> {
    let probe = match Probe::open(path) {
        Ok(probe) => probe,
        Err(e) => return Err(e),
    };
    let options = ParseOptions::new()
        .read_properties(false)
        .read_cover_art(true);
    let mut file = match probe.options(options).read() {
        Ok(file) => file,
        Err(e) => return Err(e),
    };
    let primary = file.primary_tag_type();
    let mut tag = match file.remove(primary) {
        Some(tag) => tag,
        None => return Ok(None),
    };
    let index = match tag
        .pictures()
        .iter()
        .position(|p| p.pic_type() == PictureType::CoverFront)
    {
        Some(index) => index,
        None if tag.picture_count() > 0 => 0,
        None => return Ok(None),
    };
    Ok(Some(tag.remove_picture(index)))
}

/// Content hash used to recognise a track's embedded picture as the folder cover.
///
/// `DefaultHasher::new` is SipHash with fixed keys, so the value is stable within a run, which
/// is all this needs. Length is mixed in so a truncated copy never matches.
pub fn picture_hash(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write_usize(data.len());
    hasher.write(data);
    hasher.finish()
}

/// Converts a source tag to Vorbis comments for Opus, dropping pictures identical to the folder
/// cover so each track does not carry its own copy of the album art.
pub fn to_opus(mut tag: Tag, folder_cover: Option<u64>) -> Tag {
    tag.re_map(TagType::VorbisComments);
    if let Some(cover) = folder_cover {
        // Reverse order keeps the remaining indices valid while removing.
        for index in (0..tag.pictures().len()).rev() {
            if picture_hash(tag.pictures()[index].data()) == cover {
                tag.remove_picture(index);
            }
        }
    }
    tag
}

/// Writes `tag` into the Opus file at `path`, replacing ffmpeg's comment header. Blocking.
pub fn write(tag: &Tag, path: &Path) -> Result<(), FileEncodingError> {
    tag.save_to_path(path, WriteOptions::default())
}

#[cfg(test)]
mod tests {
    use lofty::picture::MimeType;
    use lofty::tag::{Accessor, ItemKey};

    use super::*;

    fn picture(data: &[u8], pic_type: PictureType) -> Picture {
        Picture::unchecked(data.to_vec())
            .pic_type(pic_type)
            .mime_type(MimeType::Jpeg)
            .build()
    }

    #[test]
    fn hash_distinguishes_content_and_length() {
        assert_eq!(picture_hash(b"abc"), picture_hash(b"abc"));
        assert_ne!(picture_hash(b"abc"), picture_hash(b"abd"));
        assert_ne!(picture_hash(b"ab"), picture_hash(b"abc"));
    }

    #[test]
    fn strips_only_the_folder_cover() {
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title(String::from("Song"));
        tag.push_picture(picture(b"front", PictureType::CoverFront));
        tag.push_picture(picture(b"back", PictureType::CoverBack));
        let out = to_opus(tag, Some(picture_hash(b"front")));
        assert_eq!(out.tag_type(), TagType::VorbisComments);
        assert_eq!(out.get_string(ItemKey::TrackTitle), Some("Song"));
        assert_eq!(out.pictures().len(), 1);
        assert_eq!(out.pictures()[0].data(), b"back");
    }

    #[test]
    fn keeps_pictures_without_folder_cover() {
        let mut tag = Tag::new(TagType::Mp4Ilst);
        tag.push_picture(picture(b"front", PictureType::CoverFront));
        assert_eq!(to_opus(tag, None).pictures().len(), 1);
    }
}
