//! Plain-text session logs. By default one file per world per day, named
//! `<world>-<YYYY-MM-DD>.log`; a character's autolog can name its own file, with or without the
//! date. A dated log starts a new file at midnight.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};

pub struct Logger {
    /// The file name without the date.
    base: PathBuf,
    dated: bool,
    timestamps: bool,
    date: NaiveDate,
    path: PathBuf,
    file: BufWriter<File>,
}

impl Logger {
    /// `base` with the date put before its extension: `Bob.log` becomes `Bob-2024-12-09.log`.
    fn dated_path(base: &Path, date: NaiveDate) -> PathBuf {
        let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = match base.extension() {
            Some(ext) => format!("{stem}-{}.{}", date.format("%Y-%m-%d"), ext.to_string_lossy()),
            None => format!("{stem}-{}", date.format("%Y-%m-%d")),
        };
        base.with_file_name(name)
    }

    /// Opens (appending) today's log for `name` in `dir`, creating the directory if needed.
    pub fn open(dir: &Path, name: &str, timestamps: bool) -> io::Result<Self> {
        let base = dir.join(format!("{}.log", familiar_config::paths::file_name(name, "log")));
        Self::open_path(&base, true, timestamps)
    }

    /// Opens (appending) the log at `path`, or with `dated`, at `path` with today's date in its
    /// name.
    pub fn open_path(path: &Path, dated: bool, timestamps: bool) -> io::Result<Self> {
        if path.file_name().is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} isn't a file name", path.display())));
        }
        let date = Local::now().date_naive();
        let base = path.to_path_buf();
        let path = if dated { Self::dated_path(&base, date) } else { base.clone() };
        let file = Self::open_file(&path)?;
        let mut logger = Self { base, dated, timestamps, date, path, file };
        logger.banner("Log started")?;
        Ok(logger)
    }

    fn open_file(path: &Path) -> io::Result<BufWriter<File>> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        Ok(BufWriter::new(OpenOptions::new().create(true).append(true).open(path)?))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn banner(&mut self, what: &str) -> io::Result<()> {
        writeln!(self.file, "--- {what} {} ---", Local::now().format("%Y-%m-%d %H:%M:%S"))?;
        self.file.flush()
    }

    /// Writes one line of plain text (no ANSI codes).
    pub fn line(&mut self, text: &str) -> io::Result<()> {
        let now = Local::now();
        if self.dated && now.date_naive() != self.date {
            let new_date = now.date_naive();
            let new_path = Self::dated_path(&self.base, new_date);
            let new_file = Self::open_file(&new_path)?;
            self.banner("Continued in next file")?;
            self.date = new_date;
            self.path = new_path;
            self.file = new_file;
            self.banner("Log continued")?;
        }
        if self.timestamps {
            write!(self.file, "[{}] ", now.format("%H:%M:%S"))?;
        }
        writeln!(self.file, "{text}")?;
        self.file.flush()
    }

    pub fn close(mut self) -> io::Result<()> {
        self.banner("Log stopped")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_lines_with_banners() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = Logger::open(dir.path(), "Lambda MOO/1", false).unwrap();
        assert!(log.path().file_name().unwrap().to_str().unwrap().starts_with("Lambda_MOO_1-"));
        log.line("hello").unwrap();
        log.line("world").unwrap();
        let path = log.path().to_path_buf();
        log.close().unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert!(lines[0].starts_with("--- Log started"));
        assert_eq!(&lines[1..3], ["hello", "world"]);
        assert!(lines[3].starts_with("--- Log stopped"));
    }

    #[test]
    fn named_files_with_and_without_the_date() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("sub/StH.log");
        let log = Logger::open_path(&plain, false, false).unwrap();
        assert_eq!(log.path(), plain);
        log.close().unwrap();
        let log = Logger::open_path(&plain, true, false).unwrap();
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        assert_eq!(log.path(), dir.path().join(format!("sub/StH-{today}.log")));
        let log = Logger::open_path(&dir.path().join("noext"), true, false).unwrap();
        assert_eq!(log.path(), dir.path().join(format!("noext-{today}")));
    }

    #[test]
    fn timestamps_and_append() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = Logger::open(dir.path(), "w", true).unwrap();
        log.line("x").unwrap();
        let path = log.path().to_path_buf();
        log.close().unwrap();
        Logger::open(dir.path(), "w", true).unwrap().close().unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.lines().nth(1).unwrap().ends_with("] x"));
        assert_eq!(text.matches("Log started").count(), 2);
    }
}
