//! Bounded boot diagnostics, independent of firmware APIs so it can be host tested.
extern crate alloc;

use alloc::string::String;
use core::fmt::{self, Write};

pub const MAX_REPORT_BYTES: usize = 16 * 1024;

pub struct Report {
    text: String,
    truncated: bool,
}

impl Report {
    pub fn new() -> Self {
        Self {
            text: String::with_capacity(MAX_REPORT_BYTES),
            truncated: false,
        }
    }

    pub fn line(&mut self, args: fmt::Arguments<'_>) {
        let _ = self.write_fmt(args);
        let _ = self.write_str("\n");
    }

    pub fn finish(mut self) -> String {
        if self.truncated {
            self.text.push_str("\n[report truncated]\n");
        }
        self.text
    }
}

impl Write for Report {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.truncated || value.len() > MAX_REPORT_BYTES - 32 - self.text.len() {
            self.truncated = true;
            return Err(fmt::Error);
        }
        self.text.push_str(value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_bounded_and_keep_utf8_intact() {
        let mut report = Report::new();
        report.line(format_args!("start {}", "é"));
        for _ in 0..MAX_REPORT_BYTES {
            report.line(format_args!("é"));
        }
        let text = report.finish();
        assert!(text.starts_with("start é\n"));
        assert!(text.len() <= MAX_REPORT_BYTES);
        assert!(text.ends_with("[report truncated]\n"));
    }
}
