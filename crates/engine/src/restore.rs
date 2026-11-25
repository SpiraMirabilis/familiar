//! A character's restore log, as in BeipMU: the end of what the server sent (with its colors),
//! kept between runs and shown again when the character's tab opens.
//!
//! The file holds raw lines as they arrived. A line starting with [`MARK`] records when a
//! session started.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDateTime};

/// How much output is kept and replayed.
pub const KEEP_BYTES: u64 = 256 * 1024;
const MARK: char = '\u{1}';

pub struct RestoreLog {
    path: PathBuf,
    file: BufWriter<File>,
    size: u64,
}

/// One entry of a restore log.
#[derive(Debug, PartialEq, Eq)]
pub enum Entry {
    /// A session started at this time.
    Start(NaiveDateTime),
    Line(String),
}

impl RestoreLog {
    /// Opens the log for appending and records the start of a session.
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        let mut log = Self { path: path.into(), file: BufWriter::new(file), size };
        log.write(&format!("{MARK}{}", Local::now().naive_local().format("%Y-%m-%d %H:%M:%S")))?;
        Ok(log)
    }

    /// Records a line as the server sent it.
    pub fn line(&mut self, raw: &str) -> io::Result<()> {
        let raw = raw.replace(['\r', '\n'], "");
        // A line that happens to start with the marker would be read as one.
        let raw = raw.strip_prefix(MARK).unwrap_or(&raw);
        self.write(raw)
    }

    fn write(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.file, "{line}")?;
        self.file.flush()?;
        self.size += line.len() as u64 + 1;
        if self.size > KEEP_BYTES * 2 {
            self.trim()?;
        }
        Ok(())
    }

    /// Cuts the file down to its last [`KEEP_BYTES`].
    fn trim(&mut self) -> io::Result<()> {
        let bytes = std::fs::read(&self.path)?;
        familiar_config::paths::write_atomic(&self.path, tail(&bytes))?;
        let file = OpenOptions::new().append(true).open(&self.path)?;
        self.size = file.metadata()?.len();
        self.file = BufWriter::new(file);
        Ok(())
    }

    /// The entries at the end of the log at `path`, oldest first. A missing file has none.
    pub fn read(path: &Path) -> io::Result<Vec<Entry>> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let text = String::from_utf8_lossy(tail(&bytes));
        let mut entries: Vec<Entry> = text
            .lines()
            .map(|l| match l.strip_prefix(MARK) {
                Some(time) => NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M:%S").map(Entry::Start).unwrap_or(Entry::Line(String::new())),
                None => Entry::Line(l.to_string()),
            })
            .collect();
        // A session that received nothing isn't worth a heading.
        entries.dedup_by(|b, a| matches!((a, b), (Entry::Start(_), Entry::Start(_))));
        if matches!(entries.last(), Some(Entry::Start(_))) {
            entries.pop();
        }
        Ok(entries)
    }
}

/// The last [`KEEP_BYTES`] of `bytes`, starting at a line.
fn tail(bytes: &[u8]) -> &[u8] {
    if bytes.len() as u64 <= KEEP_BYTES {
        return bytes;
    }
    let cut = &bytes[bytes.len() - KEEP_BYTES as usize..];
    match cut.iter().position(|&b| b == b'\n') {
        Some(i) => &cut[i + 1..],
        None => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r/w-c.txt");
        assert!(RestoreLog::read(&path).unwrap().is_empty());
        let mut log = RestoreLog::open(&path).unwrap();
        log.line("\x1b[31mred\x1b[0m\r").unwrap();
        log.line("\u{1}not a mark").unwrap();
        drop(log);
        // Sessions with nothing in them are dropped.
        drop(RestoreLog::open(&path).unwrap());
        drop(RestoreLog::open(&path).unwrap());
        let entries = RestoreLog::read(&path).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[0], Entry::Start(_)));
        assert_eq!(entries[1], Entry::Line("\x1b[31mred\x1b[0m".into()));
        assert_eq!(entries[2], Entry::Line("not a mark".into()));
    }

    #[test]
    fn keeps_only_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let mut log = RestoreLog::open(&path).unwrap();
        let line = "x".repeat(999);
        for i in 0..1000 {
            log.line(&format!("{i:04}{line}")).unwrap();
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= KEEP_BYTES * 2);
        let entries = RestoreLog::read(&path).unwrap();
        assert!(entries.len() < 300);
        assert_eq!(entries.last(), Some(&Entry::Line(format!("0999{line}"))));
        // Every entry is a whole line.
        assert!(entries.iter().all(|e| matches!(e, Entry::Line(l) if l.len() == 1003)));
    }
}
