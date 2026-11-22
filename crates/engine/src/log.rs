//! Plain-text session logs. One file per world per day, named `<world>-<YYYY-MM-DD>.log`. A new
//! file starts at midnight.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};

pub struct Logger {
    dir: PathBuf,
    name: String,
    timestamps: bool,
    date: NaiveDate,
    path: PathBuf,
    file: BufWriter<File>,
}

impl Logger {
    pub fn path_for(dir: &Path, name: &str, date: NaiveDate) -> PathBuf {
        dir.join(format!("{}-{}.log", familiar_config::paths::file_name(name, "log"), date.format("%Y-%m-%d")))
    }

    /// Opens (appending) today's log for `name` in `dir`, creating the directory if needed.
    pub fn open(dir: &Path, name: &str, timestamps: bool) -> io::Result<Self> {
        let date = Local::now().date_naive();
        let path = Self::path_for(dir, name, date);
        let file = Self::open_file(&path)?;
        let mut logger = Self { dir: dir.into(), name: name.into(), timestamps, date, path, file };
        logger.banner("Log started")?;
        Ok(logger)
    }

    fn open_file(path: &Path) -> io::Result<BufWriter<File>> {
        if let Some(dir) = path.parent() {
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
        if now.date_naive() != self.date {
            self.banner("Continued in next file")?;
            self.date = now.date_naive();
            self.path = Self::path_for(&self.dir, &self.name, self.date);
            self.file = Self::open_file(&self.path)?;
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
