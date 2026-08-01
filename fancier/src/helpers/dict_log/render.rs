//! Plain-text rendering of decoded events, byte-for-byte compatible with
//! Zephyr's `log_parser.py` stdout (minus ANSI colors) -- the parity tests
//! in `mod.rs` diff this against the reference parser's captured output,
//! and the log viewer's "download decoded text" uses it too, so a user can
//! diff the dashboard's decode against an offline `log_parser.py` run of
//! the same chunks and see zero differences.

use super::decode::{LogEvent, LogMessage};

const HEX_BYTES_IN_LINE: usize = 16;

/// Numeric level -> Zephyr's short level tag.
pub fn level_str(level: u8) -> &'static str {
  match level {
    0 => "none",
    1 => "err",
    2 => "wrn",
    3 => "inf",
    4 => "dbg",
    _ => "unk",
  }
}

/// The `[timestamp] <lvl> source: ` prefix for a normal message; empty for
/// level 0 (printk passthrough), matching the reference.
pub fn message_prefix(m: &LogMessage) -> String {
  if m.level == 0 {
    String::new()
  } else {
    format!(
      "[{:>10}] <{}> {}: ",
      m.timestamp,
      level_str(m.level),
      m.source
    )
  }
}

/// Port of `print_hexdump`: 16 bytes per line, extra gap after 8, char
/// column as latin-1 (the reference renders every byte with `chr()`).
pub fn render_hexdump(data: &[u8], prefix_len: usize) -> String {
  let mut out = String::new();
  let mut hex_vals = String::new();
  let mut chr_vals = String::new();
  let mut done = 0usize;

  for &b in data {
    hex_vals.push_str(&format!("{b:02x} "));
    chr_vals.push(b as char);
    done += 1;

    if done == HEX_BYTES_IN_LINE / 2 {
      hex_vals.push(' ');
      chr_vals.push(' ');
    } else if done == HEX_BYTES_IN_LINE {
      out.push_str(&" ".repeat(prefix_len));
      out.push_str(&hex_vals);
      out.push('|');
      out.push_str(&chr_vals);
      out.push('\n');
      hex_vals.clear();
      chr_vals.clear();
      done = 0;
    }
  }

  if !chr_vals.is_empty() {
    out.push_str(&" ".repeat(prefix_len));
    out.push_str(&hex_vals);
    out.push_str(&"   ".repeat(HEX_BYTES_IN_LINE - done));
    out.push('|');
    out.push_str(&chr_vals);
    out.push('\n');
  }

  out
}

/// Renders the full event list as the reference parser would print it.
/// `Error` events (a decoder addition -- the reference just aborts) render
/// as their own clearly-marked line.
pub fn render_plaintext(events: &[LogEvent]) -> String {
  let mut out = String::new();
  for event in events {
    match event {
      LogEvent::Dropped(n) => {
        out.push_str(&format!("--- {n} messages dropped ---\n"));
      }
      LogEvent::Error { offset, reason } => {
        out.push_str(&format!("--- decode error at byte {offset}: {reason} ---\n"));
      }
      LogEvent::Message(m) => {
        let prefix = message_prefix(m);
        out.push_str(&prefix);
        out.push_str(&m.text);
        if m.level != 0 {
          out.push('\n');
        }
        if !m.hexdump.is_empty() {
          out.push_str(&render_hexdump(&m.hexdump, prefix.chars().count()));
        }
      }
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  fn msg(level: u8, text: &str, hexdump: &[u8]) -> LogEvent {
    LogEvent::Message(LogMessage {
      timestamp: 0,
      domain: 0,
      level,
      source: "app".to_string(),
      text: text.to_string(),
      hexdump: hexdump.to_vec(),
    })
  }

  #[test]
  fn printk_lines_carry_no_prefix_or_added_newline() {
    let out = render_plaintext(&[msg(0, "raw line\n", &[])]);
    assert_eq!(out, "raw line\n");
  }

  #[test]
  fn leveled_lines_get_prefix_and_newline() {
    let out = render_plaintext(&[msg(3, "hello", &[])]);
    assert_eq!(out, "[         0] <inf> app: hello\n");
  }

  #[test]
  fn dropped_and_error_lines() {
    let out = render_plaintext(&[
      LogEvent::Dropped(2),
      LogEvent::Error {
        offset: 7,
        reason: "x".to_string(),
      },
    ]);
    assert_eq!(
      out,
      "--- 2 messages dropped ---\n--- decode error at byte 7: x ---\n"
    );
  }

  #[test]
  fn hexdump_matches_reference_layout() {
    // 10 bytes: one partial line with the mid-line gap and 18 pad spaces.
    let out = render_hexdump(b"@ HEXDUMP#", 0);
    assert_eq!(
      out,
      "40 20 48 45 58 44 55 4d  50 23                   |@ HEXDUM P#\n"
    );
    // Exactly 16 bytes: one full line, no padding.
    let out = render_hexdump(b"HEXDUMP! HEXDUMP", 2);
    assert_eq!(
      out,
      "  48 45 58 44 55 4d 50 21  20 48 45 58 44 55 4d 50 |HEXDUMP!  HEXDUMP\n"
    );
  }

  #[test]
  fn level_tags() {
    assert_eq!(level_str(1), "err");
    assert_eq!(level_str(4), "dbg");
    assert_eq!(level_str(9), "unk");
  }
}
