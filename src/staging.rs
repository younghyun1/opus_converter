//! Write-then-rename staging for output files.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::error::Error;

/// A hidden temporary file beside its destination, removed on drop unless persisted.
///
/// Outputs are written under the temporary name and renamed into place, so an existing
/// destination always means a finished file and a rerun can skip it. The drop guard keeps
/// failures and Ctrl-C (which drops in-flight jobs) from leaving temporaries behind.
pub struct Staged {
    path: PathBuf,
    armed: bool,
}

impl Staged {
    /// Stages next to `dst`, naming the temporary after `hint`.
    ///
    /// Callers pass the source file name: `a.flac` and `a.mp3` both map to `a.opus`, and must
    /// not write through the same temporary concurrently.
    pub fn beside(dst: &Path, hint: Option<&OsStr>) -> Self {
        let mut name = OsString::from(".");
        if let Some(hint) = hint {
            name.push(hint);
        }
        name.push(".opus_converter-tmp");
        Self {
            path: dst.with_file_name(name),
            armed: true,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Renames the temporary onto `dst`, disarming the cleanup.
    pub async fn persist(mut self, dst: &Path) -> Result<(), Error> {
        match tokio::fs::rename(&self.path, dst).await {
            Ok(()) => {
                self.armed = false;
                Ok(())
            }
            Err(source) => Err(Error::io(dst, source)),
        }
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if self.armed {
            // Best effort: the file may never have been created.
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Returns true when `path` exists; errors count as absent so the job retries the write.
pub async fn exists(path: &Path) -> bool {
    matches!(tokio::fs::try_exists(path).await, Ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_name_is_hidden_and_source_specific() {
        let dst = Path::new("/nonexistent/a.opus");
        let flac = Staged::beside(dst, Some(OsStr::new("a.flac")));
        let mp3 = Staged::beside(dst, Some(OsStr::new("a.mp3")));
        assert_eq!(flac.path(), Path::new("/nonexistent/.a.flac.opus_converter-tmp"));
        assert_ne!(flac.path(), mp3.path());
    }
}
