//! Client-side decoder for Zephyr dictionary-based logging.
//!
//! Devices ship `CONFIG_LOG_DICTIONARY_SUPPORT` binary log records (see the
//! sibling `~/pigeon` library's `pigeon_log_backend.c`), which dovecote
//! stores as opaque chunks (`GET /pigeons/:id/logs`). This module decodes
//! those chunks in-browser against the firmware build's own
//! `log_dictionary.json` (uploaded via `PUT /pigeons/:id/log-dictionary`),
//! so the log viewer can render readable lines instead of only offering a
//! raw download.
//!
//! It is a faithful Rust port of Zephyr's reference implementation
//! (`zephyr/scripts/logging/dictionary/`, v4.4.1):
//!
//! - [`dictionary`] ports `log_database.py` (the v3 JSON database schema:
//!   target bits/endianness, arch, kconfigs, log-source names, string
//!   mappings, optional base64 ELF string sections).
//! - [`decode`] ports `log_parser_v3.py` (the `log_output_dict` message
//!   stream: normal-message headers, dropped-message records, cbprintf
//!   package decoding incl. appended-string tables, hexdump payloads) and
//!   `data_types.py` (per-arch va_list sizing/alignment).
//! - [`printf`] replaces Python's `%` operator: a C-style printf formatter
//!   covering the conversions the reference parser supports.
//!
//! Parity with the reference is enforced by unit tests against fixtures
//! generated with Zephyr's own tooling (`testdata/` -- see `parity` tests
//! below): the rendered text must match `log_parser.py`'s output byte for
//! byte. Where the reference parser has quirks (e.g. `%z` handling, `*`
//! width never consuming an argument), this port deliberately reproduces
//! them rather than "fixing" them, so the dashboard and the offline tooling
//! never disagree about the same bytes.
//!
//! Only version-3 databases are supported (Zephyr v3.x+ generates v3; v1/v2
//! predate the fleet's oldest firmware).

mod base64;
mod decode;
mod dictionary;
mod printf;
mod render;

pub use decode::{LogEvent, decode_chunks};
pub use dictionary::LogDictionary;
pub use render::{level_str, render_hexdump, render_plaintext};

#[cfg(test)]
mod parity {
  use super::*;

  // Fixture provenance (Zephyr v4.4.1 via the ~/pigeon-examples
  // west workspace, samples/subsys/logging/dictionary):
  //
  // - dictm3.*: qemu_cortex_m3 (arm, 32-bit LE -- same arch family as the
  //   fleet's real nRF9160/thumb devices). Built with the sample's stock
  //   prj.conf (UART dictionary-hex backend), run under the Zephyr SDK's
  //   qemu-system-arm, hex stream captured from the serial console and
  //   un-hexed exactly the way log_parser.py's --hex path does. Full
  //   string/source resolution: static strs, dynamic (packaged) strs,
  //   int8..int64, char, %p, and a hexdump payload.
  // - dict64.*: native_sim/native/64 (posix, 64-bit LE). Built with the
  //   UART dictionary-hex backend (the native-posix console backend mangles
  //   binary output) and captured via -uart_stdinout. native_sim is a host
  //   PIE binary, so its database carries no usable string
  //   mappings/log-instance addresses -- which makes this fixture exercise
  //   exactly the fallback paths (packaged-string table hits,
  //   `<string@0x..>` misses, `unknown<d:s>` sources) plus a genuine
  //   type-1 "messages dropped" record and 64-bit header/pointer layout.
  //
  // The .expected.txt files are the byte-for-byte stdout of Zephyr's own
  // log_parser.py over the same .bin + .json (ANSI colors stripped).
  const M3_JSON: &str = include_str!("testdata/dictm3.json");
  const M3_BIN: &[u8] = include_bytes!("testdata/dictm3.bin");
  const M3_EXPECTED: &str = include_str!("testdata/dictm3.expected.txt");
  const P64_JSON: &str = include_str!("testdata/dict64.json");
  const P64_BIN: &[u8] = include_bytes!("testdata/dict64.bin");
  const P64_EXPECTED: &str = include_str!("testdata/dict64.expected.txt");

  #[test]
  fn arm32_stream_matches_reference_parser() {
    let dict = LogDictionary::parse(M3_JSON).expect("fixture dictionary parses");
    assert!(!dict.bits64);
    assert!(dict.little_endian);
    let events = decode_chunks(&dict, &[M3_BIN.to_vec()]);
    assert!(
      !events.iter().any(|e| matches!(e, LogEvent::Error { .. })),
      "no decode errors expected: {events:?}"
    );
    assert_eq!(render_plaintext(&events), M3_EXPECTED);
  }

  #[test]
  fn posix64_stream_matches_reference_parser() {
    let dict = LogDictionary::parse(P64_JSON).expect("fixture dictionary parses");
    assert!(dict.bits64);
    let events = decode_chunks(&dict, &[P64_BIN.to_vec()]);
    assert!(
      !events.iter().any(|e| matches!(e, LogEvent::Error { .. })),
      "no decode errors expected: {events:?}"
    );
    // This stream contains a genuine type-1 dropped-messages record.
    assert!(
      events
        .iter()
        .any(|e| matches!(e, LogEvent::Dropped(n) if *n > 0))
    );
    assert_eq!(render_plaintext(&events), P64_EXPECTED);
  }

  #[test]
  fn chunk_boundaries_are_transparent() {
    // The device flushes its ring buffer in arbitrary-size batches; the
    // decoder concatenates chunks, so splitting the same stream anywhere --
    // including mid-message -- must decode identically to the whole.
    let dict = LogDictionary::parse(M3_JSON).unwrap();
    let whole = render_plaintext(&decode_chunks(&dict, &[M3_BIN.to_vec()]));
    for split in [1usize, 7, 64, 333, M3_BIN.len() - 3] {
      let chunks = vec![M3_BIN[..split].to_vec(), M3_BIN[split..].to_vec()];
      assert_eq!(
        render_plaintext(&decode_chunks(&dict, &chunks)),
        whole,
        "split at {split} changed the decode"
      );
    }
  }

  #[test]
  fn corrupt_chunk_resyncs_at_next_chunk_boundary() {
    let dict = LogDictionary::parse(M3_JSON).unwrap();
    // A first chunk of garbage (invalid message type) followed by the real
    // stream: the decoder must report one error for the garbage chunk and
    // still decode the real chunk fully.
    let chunks = vec![vec![0xAAu8; 10], M3_BIN.to_vec()];
    let events = decode_chunks(&dict, &chunks);
    let errors: Vec<_> = events
      .iter()
      .filter(|e| matches!(e, LogEvent::Error { .. }))
      .collect();
    assert_eq!(errors.len(), 1, "{events:?}");
    let rest: Vec<LogEvent> = events
      .into_iter()
      .filter(|e| !matches!(e, LogEvent::Error { .. }))
      .collect();
    assert_eq!(render_plaintext(&rest), M3_EXPECTED);
  }

  #[test]
  fn truncated_tail_reports_error_without_losing_prior_messages() {
    let dict = LogDictionary::parse(M3_JSON).unwrap();
    // Chop the stream mid-way: everything before the cut decodes, the
    // dangling partial message surfaces as an Error event, nothing panics.
    let cut = M3_BIN.len() / 2;
    let events = decode_chunks(&dict, &[M3_BIN[..cut].to_vec()]);
    assert!(
      events.iter().any(|e| matches!(e, LogEvent::Message(_))),
      "prefix should still decode"
    );
    assert!(
      matches!(events.last(), Some(LogEvent::Error { .. })),
      "dangling partial message should be reported: {events:?}"
    );
  }

  #[test]
  fn rejects_unsupported_database_version() {
    let json = M3_JSON.replacen("\"version\": 3", "\"version\": 1", 1);
    assert!(LogDictionary::parse(&json).is_err());
  }
}
