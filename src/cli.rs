//! Command-line arguments.

use std::path::PathBuf;

use clap::Parser;

/// Transcode a music tree to 128 kbit/s VBR Opus, mirroring it into a new directory.
///
/// Sources already at or below the target bitrate are copied unchanged.
/// Existing outputs are left alone, so an interrupted run resumes where it stopped.
#[derive(Debug, Parser)]
#[command(version)]
pub struct Args {
    /// Directory to scan recursively for music files.
    pub input: PathBuf,

    /// Output directory; defaults to `<input>-opus` beside the input.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Concurrent encodes; defaults to the number of logical CPUs.
    #[arg(short, long)]
    pub jobs: Option<usize>,

    /// Target Opus bitrate in kbit/s; sources at or below it are copied instead.
    #[arg(short, long, default_value_t = 128)]
    pub bitrate: u32,
}
