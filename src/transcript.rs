//! Bounded-memory positioning within local JSONL transcripts.
use anyhow::{Result, ensure};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Find the latest `records` nonblank JSONL lines without reading the whole file
/// into memory. Return their starting byte offset, preserving source order.
/// With `include_partial` false, an unterminated final line is left for a follower
/// to complete and does not consume one of the requested historical records.
/// Parsing and validating the selected records remains the caller's responsibility.
pub fn suffix_start(file: &mut File, records: usize, include_partial: bool) -> Result<u64> {
    ensure!(records > 0, "Transcript record count must be positive");
    let mut position = file.metadata()?.len();
    let mut remaining = records;
    let mut nonblank = false;
    let mut skip = !include_partial;
    let mut buffer = [0u8; 65536];
    while position > 0 {
        let length = position.min(buffer.len() as u64) as usize;
        position -= length as u64;
        file.seek(SeekFrom::Start(position))?;
        file.read_exact(&mut buffer[..length])?;
        for index in (0..length).rev() {
            let byte = buffer[index];
            if byte == b'\n' {
                if nonblank && !skip {
                    remaining -= 1;
                    if remaining == 0 {
                        return Ok(position + index as u64 + 1);
                    }
                }
                skip = false;
                nonblank = false;
            } else if !byte.is_ascii_whitespace() {
                nonblank = true;
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn suffix(contents: &str, records: usize, include_partial: bool) -> String {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        let offset = suffix_start(&mut file, records, include_partial).unwrap();
        contents[offset as usize..].to_owned()
    }

    #[test]
    fn suffix_counts_nonblank_records_and_keeps_source_order() {
        assert_eq!(suffix("a\nb\nc\n", 2, true), "b\nc\n");
        assert_eq!(suffix("a\n\n b \r\n\t\nc\n\n", 2, true), " b \r\n\t\nc\n\n");
        assert_eq!(suffix("a\nb\nc", 2, true), "b\nc");
        assert_eq!(suffix("a\nb\nc", 1, false), "b\nc");
        assert_eq!(suffix("a\nb\nc\n", 1, false), "c\n");
        assert_eq!(suffix("a\nb\n", 10, true), "a\nb\n");
        assert_eq!(suffix("", 1, true), "");
        assert_eq!(suffix("\n \t\n", 1, true), "\n \t\n");
    }

    #[test]
    fn suffix_handles_unicode_and_lines_crossing_read_boundaries() {
        let contents = format!("old\n{}\n東京\n", "x".repeat(65536 * 2));
        assert_eq!(suffix(&contents, 1, true), "東京\n");
        assert_eq!(suffix(&contents, 2, true), contents[4..]);
        let contents = format!("old\n{}", "é".repeat(65536));
        assert_eq!(suffix(&contents, 1, true), contents[4..]);
        assert_eq!(suffix(&contents, 1, false), contents);
    }
}
