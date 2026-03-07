use anyhow::{Context, Result};
use chrono::Local;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

const MAX_LOG_SIZE_BYTES: u64 = 1_048_576;

struct LoggerInner {
    file: File,
    log_file: PathBuf,
    verbose: bool,
}

static LOGGER: OnceLock<Mutex<LoggerInner>> = OnceLock::new();

pub fn init(log_file: PathBuf, verbose: bool) -> Result<()> {
    if LOGGER.get().is_some() {
        return Ok(());
    }

    let _ = rotate_if_needed(&log_file)?;
    let file = open_append(&log_file)?;

    let inner = LoggerInner {
        file,
        log_file,
        verbose,
    };

    let _ = LOGGER.set(Mutex::new(inner));
    Ok(())
}

pub fn info(message: impl AsRef<str>) {
    write_line("INFO", message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    write_line("WARN", message.as_ref());
}

pub fn error(message: impl AsRef<str>) {
    write_line("ERROR", message.as_ref());
}

fn write_line(level: &str, message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("[{timestamp}] [{level}] {message}\n");

    if let Some(lock) = LOGGER.get() {
        if let Ok(mut inner) = lock.lock() {
            if rotate_if_needed(&inner.log_file).unwrap_or(false) {
                if let Ok(new_file) = open_append(&inner.log_file) {
                    inner.file = new_file;
                }
            }

            let _ = inner.file.write_all(line.as_bytes());
            let _ = inner.file.flush();
            if inner.verbose {
                print!("{line}");
            }
            return;
        }
    }

    eprintln!("{line}");
}

fn open_append(path: &PathBuf) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Falha ao abrir log {}", path.display()))
}

fn rotate_if_needed(log_file: &PathBuf) -> Result<bool> {
    if !log_file.exists() {
        return Ok(false);
    }

    let metadata = fs::metadata(log_file)
        .with_context(|| format!("Falha ao ler metadados de {}", log_file.display()))?;
    if metadata.len() < MAX_LOG_SIZE_BYTES {
        return Ok(false);
    }

    let rotated = log_file.with_extension("log.1");
    if rotated.exists() {
        fs::remove_file(&rotated)
            .with_context(|| format!("Falha ao remover {}", rotated.display()))?;
    }

    fs::rename(log_file, &rotated).with_context(|| {
        format!(
            "Falha ao rotacionar log de {} para {}",
            log_file.display(),
            rotated.display()
        )
    })?;

    Ok(true)
}
