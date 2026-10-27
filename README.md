# opus_converter

Mirrors a music library as 128 kbit/s VBR Opus using the system `ffmpeg` with libopus. Sources already at or below the target bitrate are copied unchanged, so lossy files are never transcoded a second time.

## Usage

```
opus_converter <input> [-o <output>] [-j <jobs>] [-b <kbps>]
```

The input directory is scanned recursively; the output defaults to `<input>-opus` beside it and mirrors the input layout. Logs are JSON lines on stdout. An interrupted run resumes: outputs are written to a hidden temporary file and renamed into place, so any existing output is complete and is skipped.

## Behaviour

- **Encoding**: libopus, unconstrained VBR, `compression_level 10`, `application audio`, 20 ms frames, soxr resampling to 48 kHz. One single-threaded `ffmpeg` per core, largest files first.
- **Bitrate check**: read in-process from file headers with [lofty](https://crates.io/crates/lofty); files at or below the target (with 1% tolerance) are copied. Unknown bitrates are encoded.
- **Tags**: lofty converts ID3v2, MP4, APE and Vorbis comments to Opus Vorbis comments. Formats lofty cannot read (WMA, DSD, TTA) fall back to ffmpeg's tag mapping.
- **Album art**: written once per folder. An existing `cover`, `folder`, `front` or `album` image is copied; otherwise the first track's front cover is extracted to `cover.<ext>`. A track keeps an embedded picture only when it differs byte for byte from the folder image.

## Requirements

Linux or macOS, `ffmpeg` built with `--enable-libopus` and `--enable-libsoxr` on `PATH`. Building uses nightly with `-Zbuild-std`; `.cargo/config.toml` targets `x86_64-unknown-linux-gnu` at x86-64-v3, and Apple Silicon builds pass `--target aarch64-apple-darwin`.
