//! Crate error type.

use std::path::PathBuf;
use std::process::ExitStatus;

/// Every failure source in the crate, one variant each.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        program: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("ffmpeg failed on {path} ({status}): {stderr}")]
    Encode {
        path: PathBuf,
        status: ExitStatus,
        stderr: String,
    },
    #[error("failed to write tags to {path}: {source}")]
    TagWrite {
        path: PathBuf,
        #[source]
        source: lofty::error::FileEncodingError,
    },
    #[error("blocking task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("{path} is not a directory")]
    NotADirectory { path: PathBuf },
    #[error("output directory {path} is the input directory")]
    SameDirectory { path: PathBuf },
    #[error("{path} lies outside the input root")]
    OutsideRoot { path: PathBuf },
}

impl Error {
    /// Wraps an I/O error with the path it occurred on.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
