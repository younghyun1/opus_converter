//! Recursive discovery of music files, grouped by the directory (album) that holds them.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use tracing::warn;

use crate::error::Error;

/// Extensions treated as audio, compared case-insensitively.
const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aiff", "alac", "ape", "dff", "dsf", "flac", "m4a", "mka", "mp2", "mp3", "mpc",
    "oga", "ogg", "opus", "tta", "wav", "webm", "wma", "wv",
];

/// Folder image names players look for, best first; compared case-insensitively.
const COVER_STEMS: &[&str] = &["cover", "folder", "front", "album"];
const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// A discovered source file and its size, used to schedule the largest jobs first.
#[derive(Debug)]
pub struct Source {
    pub path: PathBuf,
    pub size: u64,
}

/// One directory's audio files plus its folder cover image, if it has one.
#[derive(Debug)]
pub struct Album {
    pub dir: PathBuf,
    pub cover: Option<PathBuf>,
    /// Sorted by path, so "the first track" is stable between runs.
    pub tracks: Vec<Source>,
}

/// Returns true when the file name carries a known audio extension.
pub fn is_audio(path: &Path) -> bool {
    match path.extension().and_then(OsStr::to_str) {
        Some(ext) => AUDIO_EXTENSIONS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(ext)),
        None => false,
    }
}

/// Ranks a file as a folder cover image; lower is better, `None` means not a cover.
pub fn cover_rank(path: &Path) -> Option<usize> {
    let (Some(ext), Some(stem)) = (
        path.extension().and_then(OsStr::to_str),
        path.file_stem().and_then(OsStr::to_str),
    ) else {
        return None;
    };
    if !COVER_EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
        return None;
    }
    COVER_STEMS
        .iter()
        .position(|s| s.eq_ignore_ascii_case(stem))
}

/// Walks `root` and returns every directory that holds audio.
///
/// Hidden entries are skipped (macOS `._` resource forks would otherwise fail to decode), as is
/// `exclude`, so an output directory nested inside the input is never rescanned. Symlinks are not
/// followed, which rules out cycles. Unreadable subdirectories are logged and skipped rather than
/// aborting the whole run.
pub async fn collect(root: &Path, exclude: &Path) -> Result<Vec<Album>, Error> {
    let mut albums = Vec::new();
    let mut pending = vec![root.to_path_buf()];

    while let Some(dir) = pending.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(source) if dir == root => return Err(Error::io(dir, source)),
            Err(source) => {
                warn!(path = %dir.display(), error = %source, "Skipping unreadable directory");
                continue;
            }
        };
        let mut tracks = Vec::new();
        let mut cover: Option<(usize, PathBuf)> = None;
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(source) => {
                    warn!(path = %dir.display(), error = %source, "Directory listing interrupted");
                    break;
                }
            };
            if entry.file_name().as_encoded_bytes().first() == Some(&b'.') {
                continue;
            }
            let path = entry.path();
            // `DirEntry::file_type` comes from d_type on Linux, so no extra stat for directories.
            let file_type = match entry.file_type().await {
                Ok(file_type) => file_type,
                Err(source) => {
                    warn!(path = %path.display(), error = %source, "Skipping unreadable entry");
                    continue;
                }
            };
            if file_type.is_dir() {
                if path != exclude {
                    pending.push(path);
                }
            } else if !file_type.is_file() {
                continue;
            } else if is_audio(&path) {
                match entry.metadata().await {
                    Ok(meta) => tracks.push(Source {
                        path,
                        size: meta.len(),
                    }),
                    Err(source) => {
                        warn!(path = %path.display(), error = %source, "Skipping unreadable file");
                    }
                }
            } else if let Some(rank) = cover_rank(&path) {
                let better = match &cover {
                    Some((best, _)) => rank < *best,
                    None => true,
                };
                if better {
                    cover = Some((rank, path));
                }
            }
        }
        if !tracks.is_empty() {
            tracks.sort_unstable_by(|a, b| a.path.cmp(&b.path));
            albums.push(Album {
                dir,
                cover: cover.map(|(_, path)| path),
                tracks,
            });
        }
    }
    Ok(albums)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_audio_case_insensitively() {
        assert!(is_audio(Path::new("a/b/track.flac")));
        assert!(is_audio(Path::new("TRACK.MP3")));
        assert!(is_audio(Path::new("x.Opus")));
    }

    #[test]
    fn rejects_non_audio() {
        assert!(!is_audio(Path::new("cover.jpg")));
        assert!(!is_audio(Path::new("album.cue")));
        assert!(!is_audio(Path::new("flac")));
        assert!(!is_audio(Path::new("noext")));
    }

    #[test]
    fn ranks_cover_names() {
        assert_eq!(cover_rank(Path::new("/a/Cover.JPG")), Some(0));
        assert_eq!(cover_rank(Path::new("/a/folder.png")), Some(1));
        assert!(cover_rank(Path::new("/a/cover.jpg")) < cover_rank(Path::new("/a/front.jpg")));
    }

    #[test]
    fn rejects_non_cover_images() {
        assert_eq!(cover_rank(Path::new("/a/back.jpg")), None);
        assert_eq!(cover_rank(Path::new("/a/cover.txt")), None);
        assert_eq!(cover_rank(Path::new("/a/cover")), None);
    }
}
