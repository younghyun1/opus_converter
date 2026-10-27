//! `opus_converter`: mirror a music tree as 128 kbit/s VBR Opus using the system ffmpeg.
//!
//! Bitrate and tags are read in-process with lofty; only sources above the target are encoded,
//! each by one single-threaded `ffmpeg` child. Throughput comes from running one encode per core,
//! scheduled largest file first, with Tokio driving the children and all file system access.
//! Album art is written once per folder instead of into every track.

// The crate writes each failure path out as a `match` instead of `?`, so a pass-through reads the
// same as an arm that handles the error.
#![allow(clippy::question_mark)]

mod art;
mod cli;
mod error;
mod pipeline;
mod plan;
mod scan;
mod staging;
mod tags;
mod transcode;

use std::process::ExitCode;

use clap::Parser;
use tracing::{error, warn};
use tracing_subscriber::EnvFilter;

use crate::cli::Args;

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn init_logging() {
    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter,
        Err(_) => EnvFilter::new("opus_converter=info,warn"),
    };
    tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(false)
        .with_ansi(false)
        .with_env_filter(filter)
        .with_writer(std::io::stdout)
        .init();
}

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();
    let args = Args::parse();

    tokio::select! {
        result = pipeline::run(args) => match result {
            Ok(totals) if totals.failed == 0 => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(e) => {
                error!(error = %e, "Aborted");
                ExitCode::FAILURE
            }
        },
        // Dropping `run` aborts every task; `kill_on_drop` stops the children and each job's
        // staged output is removed by its guard.
        signal = tokio::signal::ctrl_c() => {
            match signal {
                Ok(()) => warn!("Interrupted; finished files are kept and a rerun resumes"),
                Err(e) => error!(error = %e, "Failed to listen for Ctrl-C"),
            }
            ExitCode::from(130)
        }
    }
}
