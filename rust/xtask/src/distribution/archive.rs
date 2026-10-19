//! Purpose: deterministic release archives matching the baseline layout: the executable sits at the
//! archive root (`tar -czf <asset>.tar.gz *` / `zip -r <asset>.zip *` run inside `dist/<asset>/bin`).
//! Scope: filesystem writes into the caller-provided output directory only.
//! Uses:
//! `-- none
//! External:
//! |-- anyhow::Context
//! |-- anyhow::Result
//! |-- flate2::Compression
//! |-- flate2::read::GzDecoder
//! |-- flate2::write::GzEncoder
//! |-- tar::Archive::new
//! |-- tar::Builder::new
//! |-- tar::EntryType::Regular
//! |-- tar::Header::new_gnu
//! |-- zip::CompressionMethod::Deflated
//! |-- zip::ZipArchive::new
//! |-- zip::ZipWriter::new
//! `-- zip::write::SimpleFileOptions
//! Used by:
//! rust/xtask/
//! |-- src/distribution/build.rs
//! |-- src/distribution/mod.rs
//! |-- src/distribution/tests.rs
//! `-- src/distribution/verify.rs
//! Invariants: mtime 0 (tar) / 1980-01-01 (zip), uid/gid 0, mode 0755, entry name is the bare
//! executable name, so identical inputs give identical archive bytes.
//! Contract tests: xtask/src/distribution/tests.rs

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use zip::write::SimpleFileOptions;

/// Write `out` (`*.tar.gz` or `*.zip`) holding `binary` under `entry`.
pub fn write(binary: &Path, entry: &str, out: &Path) -> Result<()> {
    let bytes = std::fs::read(binary).with_context(|| format!("reading {}", binary.display()))?;
    let file = File::create(out).with_context(|| format!("creating {}", out.display()))?;
    if out.to_string_lossy().ends_with(".tar.gz") {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_entry_type(tar::EntryType::Regular);
        let mut builder = tar::Builder::new(GzEncoder::new(file, Compression::best()));
        builder.append_data(&mut header, entry, bytes.as_slice())?;
        builder.into_inner()?.finish()?.flush()?;
        return Ok(());
    }
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);
    zip.start_file(entry, options)?;
    zip.write_all(&bytes)?;
    zip.finish()?.flush()?;
    Ok(())
}

/// Entry names and their contents, for verification.
pub fn list(path: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    if path.to_string_lossy().ends_with(".tar.gz") {
        let mut archive = tar::Archive::new(GzDecoder::new(file));
        return archive
            .entries()?
            .map(|entry| {
                let mut entry = entry?;
                let name = entry.path()?.to_string_lossy().into_owned();
                let mut data = Vec::new();
                entry.read_to_end(&mut data)?;
                Ok((name, data))
            })
            .collect();
    }
    let mut zip = zip::ZipArchive::new(file)?;
    (0..zip.len())
        .map(|i| {
            let mut entry = zip.by_index(i)?;
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            Ok((entry.name().to_string(), data))
        })
        .collect()
}
