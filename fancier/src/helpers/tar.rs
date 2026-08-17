/// Builds an uncompressed POSIX ustar archive in memory, one regular-file
/// entry per `(name, bytes)` pair, in the order given.
///
/// Exists so the device log viewer's "download all raw chunks" button can
/// hand the browser a single file instead of one `download_bytes` call per
/// chunk (browsers throttle/drop bulk programmatic downloads with no error,
/// and a gap in a debug log reads as a gap in device behaviour). A tar
/// preserves each chunk as its own archive member with its own length --
/// unlike a bare concatenation of the chunks, which desyncs Zephyr's
/// `log_parser.py`: each chunk is an independent ring-buffer flush, not
/// necessarily message-aligned at its edges (a record can straddle a flush
/// boundary), so joining raw bytes end to end can hand the parser a garbage
/// header wherever two chunks meet.
///
/// To decode on the host, extract and feed the parser one member at a time,
/// exactly as it already expects (it takes one dbfile and one logfile per
/// invocation, no batch mode):
///
/// ```sh
/// tar xf pigeon-<id>-logs.tar
/// for f in pigeon-*.bin; do
///   python log_parser.py log_dictionary.json "$f"
/// done
/// ```
///
/// No archive crate: ustar is a fixed 512-byte-record layout simple enough
/// to emit directly, so a debug-only download path doesn't grow the wasm
/// bundle for everyone (same tradeoff `download.rs` makes using `atob`
/// instead of a base64 crate).
const BLOCK_SIZE: usize = 512;

fn octal_field(value: u64, width: usize) -> Vec<u8> {
  // Numeric ustar fields are ASCII octal, NUL-terminated, zero-padded to
  // fill the field up to the terminator.
  let mut field = vec![0u8; width];
  let digits = format!("{value:0>width$o}", width = width - 1);
  field[..width - 1].copy_from_slice(digits.as_bytes());
  field
}

fn checksum(header: &[u8; BLOCK_SIZE]) -> u32 {
  header.iter().map(|&b| b as u32).sum()
}

fn write_header(name: &str, size: usize) -> [u8; BLOCK_SIZE] {
  let mut header = [0u8; BLOCK_SIZE];

  let name_bytes = name.as_bytes();
  header[0..name_bytes.len().min(100)].copy_from_slice(&name_bytes[..name_bytes.len().min(100)]);

  header[100..108].copy_from_slice(&octal_field(0o644, 8)); // mode
  header[108..116].copy_from_slice(&octal_field(0, 8)); // uid
  header[116..124].copy_from_slice(&octal_field(0, 8)); // gid
  header[124..136].copy_from_slice(&octal_field(size as u64, 12)); // size
  header[136..148].copy_from_slice(&octal_field(0, 12)); // mtime
  header[148..156].copy_from_slice(b"        "); // chksum placeholder, filled below
  header[156] = b'0'; // typeflag: regular file
  header[257..263].copy_from_slice(b"ustar\0");
  header[263..265].copy_from_slice(b"00");

  let sum = checksum(&header);
  let sum_field = format!("{sum:06o}\0 ");
  header[148..156].copy_from_slice(sum_field.as_bytes());

  header
}

/// Panics if `name` doesn't fit ustar's 100-byte name field -- every caller
/// in this codebase builds names from `chunk_filename`, which is short and
/// fixed-shape, so this is a real invariant, not user input.
pub fn build_tar(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
  let mut out = Vec::new();

  for (name, bytes) in entries {
    assert!(
      name.len() <= 100,
      "tar entry name {name:?} exceeds the ustar 100-byte name field"
    );
    out.extend_from_slice(&write_header(name, bytes.len()));
    out.extend_from_slice(bytes);
    let padding = (BLOCK_SIZE - (bytes.len() % BLOCK_SIZE)) % BLOCK_SIZE;
    out.extend(std::iter::repeat_n(0u8, padding));
  }

  // Archive ends with two consecutive zero-filled records.
  out.extend(std::iter::repeat_n(0u8, BLOCK_SIZE * 2));

  out
}

#[cfg(test)]
mod tests {
  use super::build_tar;

  #[test]
  fn empty_archive_is_two_zero_blocks() {
    let archive = build_tar(&[]);
    assert_eq!(archive.len(), 1024);
    assert!(archive.iter().all(|&b| b == 0));
  }

  #[test]
  fn single_entry_round_trips_through_a_manual_parse() {
    let archive = build_tar(&[("chunk-1.bin".to_string(), vec![1, 2, 3, 4, 5])]);

    // Mirrors what a real tar reader does: name is a NUL-padded C string,
    // size is octal ASCII in the header, data follows padded to a 512
    // boundary, then the archive ends with two zero blocks.
    let name = std::str::from_utf8(&archive[0..11]).unwrap();
    assert_eq!(name, "chunk-1.bin");

    let size_field = std::str::from_utf8(&archive[124..135]).unwrap();
    let size = u64::from_str_radix(size_field.trim_end_matches('\0'), 8).unwrap();
    assert_eq!(size, 5);

    let data = &archive[512..517];
    assert_eq!(data, &[1, 2, 3, 4, 5]);

    // One data block (padded) plus the two trailing zero blocks.
    assert_eq!(archive.len(), 512 + 512 + 1024);
    assert!(archive[512 + 1024..].iter().all(|&b| b == 0));
  }

  #[test]
  fn multiple_entries_preserve_boundaries() {
    let archive = build_tar(&[
      ("a.bin".to_string(), vec![0xAA; 10]),
      ("b.bin".to_string(), vec![0xBB; 600]), // spans more than one 512-byte block
    ]);

    let first_data = &archive[512..522];
    assert!(first_data.iter().all(|&b| b == 0xAA));

    // Second header starts after the first entry's padded data block.
    let second_header_offset = 512 + 512;
    let second_name =
      std::str::from_utf8(&archive[second_header_offset..second_header_offset + 5]).unwrap();
    assert_eq!(second_name, "b.bin");

    let second_data_offset = second_header_offset + 512;
    let second_data = &archive[second_data_offset..second_data_offset + 600];
    assert!(second_data.iter().all(|&b| b == 0xBB));
  }

  #[test]
  fn header_checksum_matches_manual_sum() {
    let archive = build_tar(&[("x.bin".to_string(), vec![7; 3])]);
    let header = &archive[0..512];

    let mut expected: u32 = 0;
    for (i, &b) in header.iter().enumerate() {
      if (148..156).contains(&i) {
        expected += b' ' as u32;
      } else {
        expected += b as u32;
      }
    }

    let sum_field = std::str::from_utf8(&header[148..154]).unwrap();
    let stored = u32::from_str_radix(sum_field, 8).unwrap();
    assert_eq!(stored, expected);
  }
}
