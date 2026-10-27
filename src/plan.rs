//! Run-wide settings and source-to-output path mapping.

use std::path::{Path, PathBuf};

use crate::error::Error;

/// Shared settings for every job in a run.
#[derive(Debug)]
pub struct Plan {
    pub input: PathBuf,
    pub output: PathBuf,
    /// Target bitrate in kbit/s.
    pub target_kbps: u32,
}

impl Plan {
    /// Sources at or below this many kbit/s are copied rather than re-encoded.
    ///
    /// Encoders report nominal rates loosely (a 128k AAC can read as 129 after rounding), so
    /// allow 1% before deciding a file is above the target; re-encoding lossy audio at the same
    /// rate only adds generation loss.
    pub fn copy_threshold(&self) -> u32 {
        self.target_kbps + self.target_kbps / 100
    }

    /// Maps a source path to its mirrored path in the output tree, keeping the original name.
    pub fn mirrored(&self, src: &Path) -> Result<PathBuf, Error> {
        match src.strip_prefix(&self.input) {
            Ok(rel) => Ok(self.output.join(rel)),
            Err(_) => Err(Error::OutsideRoot {
                path: src.to_path_buf(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan {
            input: PathBuf::from("/music"),
            output: PathBuf::from("/music-opus"),
            target_kbps: 128,
        }
    }

    #[test]
    fn threshold_allows_one_percent() {
        assert_eq!(plan().copy_threshold(), 129);
        assert_eq!(
            Plan {
                target_kbps: 96,
                ..plan()
            }
            .copy_threshold(),
            96
        );
    }

    #[test]
    fn mirrors_relative_path() {
        let got = plan()
            .mirrored(Path::new("/music/Artist/Album/01.flac"))
            .ok();
        assert_eq!(got, Some(PathBuf::from("/music-opus/Artist/Album/01.flac")));
        assert_eq!(
            plan().mirrored(Path::new("/music")).ok(),
            Some(PathBuf::from("/music-opus"))
        );
    }

    #[test]
    fn rejects_path_outside_root() {
        assert!(plan().mirrored(Path::new("/elsewhere/a.flac")).is_err());
    }
}
