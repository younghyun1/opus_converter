//! Run orchestration: resolve roots, scan, prepare folder art, then transcode concurrently.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;
use tokio::task::{JoinError, JoinSet};
use tracing::{error, info, warn};

use crate::art;
use crate::cli::Args;
use crate::error::Error;
use crate::plan::Plan;
use crate::scan::{self, Source};
use crate::transcode::{self, Outcome};

/// Per-run counters for the final summary.
#[derive(Debug, Default)]
pub struct Totals {
    pub encoded: u64,
    pub copied: u64,
    pub already_done: u64,
    pub failed: u64,
}

type JobResult = (PathBuf, Result<(Outcome, Option<u32>), Error>, Duration);

/// Resolves the input and output roots, both canonical so prefix stripping and the nested-output
/// exclusion compare like with like.
async fn resolve_roots(args: &Args) -> Result<(PathBuf, PathBuf), Error> {
    let input = match tokio::fs::canonicalize(&args.input).await {
        Ok(path) => path,
        Err(source) => return Err(Error::io(&args.input, source)),
    };
    match tokio::fs::metadata(&input).await {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(Error::NotADirectory { path: input }),
        Err(source) => return Err(Error::io(input, source)),
    }
    let output = match &args.output {
        Some(path) => path.clone(),
        None => match input.file_name() {
            Some(name) => {
                let mut name = name.to_os_string();
                name.push("-opus");
                input.with_file_name(name)
            }
            None => return Err(Error::NotADirectory { path: input }),
        },
    };
    if let Err(source) = tokio::fs::create_dir_all(&output).await {
        return Err(Error::io(output, source));
    }
    let output = match tokio::fs::canonicalize(&output).await {
        Ok(path) => path,
        Err(source) => return Err(Error::io(output, source)),
    };
    if output == input {
        return Err(Error::SameDirectory { path: output });
    }
    Ok((input, output))
}

/// Runs the whole conversion and returns the counters.
pub async fn run(args: Args) -> Result<Totals, Error> {
    let (input, output) = match resolve_roots(&args).await {
        Ok(roots) => roots,
        Err(e) => return Err(e),
    };
    let jobs = match args.jobs {
        Some(jobs) => jobs.max(1),
        None => match std::thread::available_parallelism() {
            Ok(n) => n.get(),
            Err(_) => 1,
        },
    };

    let started = Instant::now();
    let albums = match scan::collect(&input, &output).await {
        Ok(albums) => albums,
        Err(e) => return Err(e),
    };
    info!(
        input = %input.display(),
        output = %output.display(),
        albums = albums.len(),
        files = albums.iter().map(|a| a.tracks.len()).sum::<usize>(),
        jobs,
        "Scan complete"
    );

    let plan = Arc::new(Plan {
        input,
        output,
        target_kbps: args.bitrate,
    });
    // Each permit is taken before spawning, so at most `jobs` tasks and child processes exist at
    // once regardless of library size.
    let permits = Arc::new(Semaphore::new(jobs));

    // Folder art first: every track needs its album's cover hash before it is tagged. This is
    // header reads and small writes, so it finishes in a fraction of the encode time.
    let mut covers: Vec<Option<u64>> = vec![None; albums.len()];
    let mut tasks = JoinSet::new();
    for (index, album) in albums.iter().enumerate() {
        let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
            break;
        };
        let plan = Arc::clone(&plan);
        let dir = album.dir.clone();
        let cover = album.cover.clone();
        let first = album.tracks.first().map(|track| track.path.clone());
        tasks.spawn(async move {
            let result = art::prepare(&plan, &dir, cover.as_deref(), first.as_deref()).await;
            drop(permit);
            (index, result)
        });
    }
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((index, Ok(cover))) => covers[index] = cover,
            Ok((index, Err(e))) => {
                warn!(path = %albums[index].dir.display(), error = %e, "Folder art failed; tracks keep embedded art");
            }
            Err(e) => error!(error = %e, "Folder art task failed"),
        }
    }

    let mut work: Vec<(Source, Option<u64>)> = albums
        .into_iter()
        .zip(covers)
        .flat_map(|(album, cover)| album.tracks.into_iter().map(move |t| (t, cover)))
        .collect();
    // Longest-job-first keeps a large file from starting last and stretching the tail.
    work.sort_unstable_by_key(|(source, _)| std::cmp::Reverse(source.size));

    let mut tasks = JoinSet::new();
    let mut totals = Totals::default();
    for (source, cover) in work {
        let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
            break;
        };
        let plan = Arc::clone(&plan);
        tasks.spawn(async move {
            let job_started = Instant::now();
            let result = transcode::process(&plan, &source.path, cover).await;
            drop(permit);
            (source.path, result, job_started.elapsed())
        });
        // Reap finished tasks as we go so results are logged promptly and the set stays small.
        while let Some(joined) = tasks.try_join_next() {
            record(joined, &mut totals);
        }
    }
    while let Some(joined) = tasks.join_next().await {
        record(joined, &mut totals);
    }

    info!(
        encoded = totals.encoded,
        copied = totals.copied,
        already_done = totals.already_done,
        failed = totals.failed,
        elapsed_s = started.elapsed().as_secs_f64(),
        "Done"
    );
    Ok(totals)
}

fn record(joined: Result<JobResult, JoinError>, totals: &mut Totals) {
    let (path, result, elapsed) = match joined {
        Ok(job) => job,
        Err(e) => {
            totals.failed += 1;
            error!(error = %e, "Job panicked or was cancelled");
            return;
        }
    };
    let path = path.display().to_string();
    match result {
        Ok((Outcome::Encoded, kbps)) => {
            totals.encoded += 1;
            info!(
                path,
                source_kbps = kbps,
                elapsed_s = elapsed.as_secs_f64(),
                "Encoded"
            );
        }
        Ok((Outcome::Copied, kbps)) => {
            totals.copied += 1;
            info!(
                path,
                source_kbps = kbps,
                "Copied; already at or below target"
            );
        }
        Ok((Outcome::AlreadyDone, _)) => totals.already_done += 1,
        Err(e) => {
            totals.failed += 1;
            error!(path, error = %e, "Failed");
        }
    }
}
