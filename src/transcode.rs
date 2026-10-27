//! Per-file work: read properties and tags, then encode to Opus or copy through.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;
use tracing::warn;

use crate::error::Error;
use crate::plan::Plan;
use crate::staging::{Staged, exists};
use crate::tags::{self, SourceInfo};

/// What happened to one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Encoded,
    Copied,
    AlreadyDone,
}

/// Prefixes a path with ffmpeg's `file:` protocol so names containing `:` or starting with `-`
/// are never read as a protocol or an option.
fn ffmpeg_url(path: &Path) -> OsString {
    let mut url = OsString::from("file:");
    url.push(path.as_os_str());
    url
}

/// Processes one source file end to end, returning the outcome and the source bitrate.
///
/// `folder_cover` is the hash of the album's folder image; embedded pictures matching it are
/// not written into the Opus file.
pub async fn process(
    plan: &Plan,
    src: &Path,
    folder_cover: Option<u64>,
) -> Result<(Outcome, Option<u32>), Error> {
    let mirrored = match plan.mirrored(src) {
        Ok(path) => path,
        Err(e) => return Err(e),
    };
    let encoded = mirrored.with_extension("opus");
    if exists(&encoded).await || exists(&mirrored).await {
        return Ok((Outcome::AlreadyDone, None));
    }
    if let Some(parent) = mirrored.parent()
        && let Err(source) = tokio::fs::create_dir_all(parent).await
    {
        return Err(Error::io(parent, source));
    }

    let path = src.to_path_buf();
    let info = match tokio::task::spawn_blocking(move || tags::read(&path)).await {
        Ok(Ok(info)) => Some(info),
        // lofty does not parse WMA, DSD or TTA; ffmpeg still can, and maps their tags itself.
        Ok(Err(e)) => {
            warn!(path = %src.display(), error = %e, "Cannot read tags; ffmpeg will map them");
            None
        }
        Err(e) => return Err(Error::Join(e)),
    };
    let bitrate = info.as_ref().and_then(|info| info.bitrate_kbps);
    // An unknown bitrate is treated as above target: encoding a file that did not need it costs
    // time, while copying a lossless file through would defeat the point of the run.
    let copy = match bitrate {
        Some(kbps) => kbps <= plan.copy_threshold(),
        None => false,
    };

    if copy {
        let staged = Staged::beside(&mirrored, src.file_name());
        if let Err(source) = tokio::fs::copy(src, staged.path()).await {
            return Err(Error::io(staged.path(), source));
        }
        return match staged.persist(&mirrored).await {
            Ok(()) => Ok((Outcome::Copied, bitrate)),
            Err(e) => Err(e),
        };
    }

    let staged = Staged::beside(&encoded, src.file_name());
    if let Err(e) = encode(src, staged.path(), plan.target_kbps, info.is_none()).await {
        return Err(e);
    }
    if let Some(SourceInfo { tag: Some(tag), .. }) = info {
        let tag = tags::to_opus(tag, folder_cover);
        let path = staged.path().to_path_buf();
        match tokio::task::spawn_blocking(move || tags::write(&tag, &path)).await {
            Ok(Ok(())) => {}
            Ok(Err(source)) => {
                return Err(Error::TagWrite {
                    path: src.to_path_buf(),
                    source,
                });
            }
            Err(e) => return Err(Error::Join(e)),
        }
    }
    match staged.persist(&encoded).await {
        Ok(()) => Ok((Outcome::Encoded, bitrate)),
        Err(e) => Err(e),
    }
}

/// Encodes `src` to Ogg Opus at `dst`.
///
/// - `-threads 1`: libopus is single-threaded and the caller already runs one job per core, so
///   decoder threads would only oversubscribe the CPU.
/// - `-map 0:a:0`: first audio stream only. The Ogg Opus muxer rejects attached pictures; art
///   is handled at folder level and by lofty.
/// - `ffmpeg_tags`: map tags through ffmpeg only when lofty could not read the source;
///   otherwise lofty rewrites the comment header afterwards and ffmpeg's would be discarded.
/// - soxr at precision 28 is the highest-quality resampler ffmpeg offers; Opus always runs at
///   48 kHz internally, so 44.1 kHz sources must be resampled and it is a no-op for 48 kHz input.
/// - `-vbr on` with `-compression_level 10` is unconstrained VBR at the slowest, best encoder
///   search; `-application audio` tunes for music rather than speech. 20 ms frames (the default)
///   are the best quality-per-bit for music; longer frames smear transients.
async fn encode(src: &Path, dst: &Path, target_kbps: u32, ffmpeg_tags: bool) -> Result<(), Error> {
    let output = match Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(["-threads", "1", "-i"])
        .arg(ffmpeg_url(src))
        .args(["-map", "0:a:0"])
        .args(["-map_metadata", if ffmpeg_tags { "0" } else { "-1" }])
        .args(["-af", "aresample=48000:resampler=soxr:precision=28"])
        .args(["-c:a", "libopus", "-b:a"])
        .arg(format!("{target_kbps}k"))
        .args([
            "-vbr",
            "on",
            "-compression_level",
            "10",
            "-application",
            "audio",
        ])
        .args(["-f", "opus"])
        .arg(ffmpeg_url(dst))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
    {
        Ok(output) => output,
        Err(source) => {
            return Err(Error::Spawn {
                program: "ffmpeg",
                source,
            });
        }
    };
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Encode {
            path: src.to_path_buf(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_prefixes_file_protocol() {
        assert_eq!(
            ffmpeg_url(Path::new("/a:b.flac")),
            OsString::from("file:/a:b.flac")
        );
    }
}
