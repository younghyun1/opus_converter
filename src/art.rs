//! Folder-level album art.
//!
//! Players (foobar2000, mpd, Navidrome, Jellyfin, Rockbox and most phone players) read a
//! `cover.*` or `folder.*` image beside the tracks, so one image per album replaces a copy
//! embedded in every track. A track keeps its embedded picture only when it differs byte for
//! byte from the folder image, so art is never lost or swapped, and a folder of unrelated
//! singles keeps each track's own cover.

use std::path::Path;

use tracing::warn;

use crate::error::Error;
use crate::plan::Plan;
use crate::staging::{Staged, exists};
use crate::tags;

/// Writes the album's folder image into the output tree and returns its content hash.
///
/// A cover image already in the source directory is copied under its own name. Otherwise the
/// first track's front cover is extracted to `cover.<ext>`. Returns `None` when the album has
/// no usable art, in which case every track keeps whatever it embeds.
pub async fn prepare(
    plan: &Plan,
    dir: &Path,
    cover: Option<&Path>,
    first_track: Option<&Path>,
) -> Result<Option<u64>, Error> {
    let out_dir = match plan.mirrored(dir) {
        Ok(dir) => dir,
        Err(e) => return Err(e),
    };
    if let Err(source) = tokio::fs::create_dir_all(&out_dir).await {
        return Err(Error::io(out_dir, source));
    }

    if let Some(cover) = cover {
        let data = match tokio::fs::read(cover).await {
            Ok(data) => data,
            Err(source) => return Err(Error::io(cover, source)),
        };
        let hash = tags::picture_hash(&data);
        match cover.file_name() {
            Some(name) => match write_once(&out_dir.join(name), &data).await {
                Ok(()) => return Ok(Some(hash)),
                Err(e) => return Err(e),
            },
            None => return Ok(None),
        }
    }

    let first = match first_track {
        Some(path) => path.to_path_buf(),
        None => return Ok(None),
    };
    let picture = match tokio::task::spawn_blocking(move || tags::read_cover(&first)).await {
        Ok(Ok(Some(picture))) => picture,
        Ok(Ok(None)) => return Ok(None),
        Ok(Err(e)) => {
            warn!(path = %dir.display(), error = %e, "Cannot read embedded art");
            return Ok(None);
        }
        Err(e) => return Err(Error::Join(e)),
    };
    // Without a known extension players would not recognise the file, so leave art embedded.
    let ext = match picture.mime_type().and_then(|mime| mime.ext()) {
        Some(ext) => ext,
        None => return Ok(None),
    };
    let dst = out_dir.join(format!("cover.{ext}"));
    match write_once(&dst, picture.data()).await {
        Ok(()) => Ok(Some(tags::picture_hash(picture.data()))),
        Err(e) => Err(e),
    }
}

/// Writes `data` to `dst` atomically unless it already exists.
async fn write_once(dst: &Path, data: &[u8]) -> Result<(), Error> {
    if exists(dst).await {
        return Ok(());
    }
    let staged = Staged::beside(dst, dst.file_name());
    if let Err(source) = tokio::fs::write(staged.path(), data).await {
        return Err(Error::io(staged.path(), source));
    }
    staged.persist(dst).await
}
