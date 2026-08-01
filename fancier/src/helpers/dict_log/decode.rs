//! The binary message stream: a Rust port of Zephyr's
//! `dictionary_parser/log_parser_v3.py` (message headers, dropped-message
//! records, cbprintf package decoding, hexdump payloads) plus the va_list
//! sizing/alignment tables from `data_types.py`.
//!
//! Stream framing (all fields target-endian, `__packed`):
//!
//! ```text
//! type u8            0 = normal message, 1 = dropped-messages record
//! -- type 1 --
//! count u16
//! -- type 0 --
//! domain:4|level:4   u8 (nibble order swaps with target endianness)
//! package_len        u16
//! data_len           u16 (hexdump payload length)
//! source_id          u32 / u64 (target pointer width)
//! timestamp          u32 / u64 (CONFIG_LOG_TIMESTAMP_64BIT)
//! package            package_len bytes (cbprintf package: header, fmt ptr,
//!                    va_list args, ro/rw string indexes, appended strings)
//! data               data_len bytes (hexdump payload)
//! ```
//!
//! Chunks stored by dovecote are ring-buffer flush slices of this stream:
//! normally message-aligned, but a message can straddle a flush boundary
//! and a full ring buffer drops bytes mid-stream. [`decode_chunks`]
//! therefore parses the concatenation and, on a malformed/truncated region,
//! reports one [`LogEvent::Error`] and resynchronizes at the next chunk
//! boundary -- the reference parser just aborts there, but a rolling debug
//! buffer's tail shouldn't be lost to one corrupt flush.

use super::dictionary::LogDictionary;
use super::printf::{self, Arg};
use std::collections::HashMap;

const MSG_TYPE_NORMAL: u8 = 0;
const MSG_TYPE_DROPPED: u8 = 1;

/// One decoded log message (`type == 0`).
#[derive(Debug, Clone, PartialEq)]
pub struct LogMessage {
  /// Raw target ticks, exactly as carried on the wire -- the dictionary
  /// format does not standardize a tick frequency, and the reference
  /// parser prints the raw value too.
  pub timestamp: u64,
  pub domain: u8,
  /// 0 = none (printk passthrough -- `text` is the raw line, rendered
  /// without any prefix), 1..4 = err/wrn/inf/dbg.
  pub level: u8,
  pub source: String,
  pub text: String,
  /// `LOG_HEXDUMP_*` payload bytes, empty for plain messages.
  pub hexdump: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogEvent {
  Message(LogMessage),
  /// The device's log core dropped this many messages before the backend
  /// saw them (`log_dict_output_dropped_process`).
  Dropped(u16),
  /// A region of the stream that could not be decoded -- `offset` is into
  /// the concatenated stream; decoding resumed at the next chunk boundary
  /// (or stopped, if this was the tail).
  Error { offset: usize, reason: String },
}

enum ParseFailure {
  /// More bytes than the stream holds are needed -- a truncated tail or a
  /// drop-corrupted length field.
  Incomplete,
  Malformed(String),
}

/// cbprintf argument classes, mirroring `data_types.py`'s `DataTypes`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DType {
  Int,
  Uint,
  Long,
  Ulong,
  LongLong,
  UlongLong,
  Ptr,
  Double,
  LongDouble,
}

struct Decoder<'d> {
  dict: &'d LogDictionary,
  ptr_size: usize,
  int_size: usize,
  ts_size: usize,
}

impl<'d> Decoder<'d> {
  fn new(dict: &'d LogDictionary) -> Self {
    Self {
      dict,
      ptr_size: if dict.bits64 { 8 } else { 4 },
      int_size: 4,
      ts_size: if dict.ts_64bit { 8 } else { 4 },
    }
  }

  // -- primitive readers (target endianness, bounds-checked) --

  fn bytes<const N: usize>(&self, data: &[u8], off: usize) -> Result<[u8; N], ParseFailure> {
    data
      .get(off..off + N)
      .and_then(|s| s.try_into().ok())
      .ok_or(ParseFailure::Incomplete)
  }

  fn u8_at(&self, data: &[u8], off: usize) -> Result<u8, ParseFailure> {
    data.get(off).copied().ok_or(ParseFailure::Incomplete)
  }

  fn u16_at(&self, data: &[u8], off: usize) -> Result<u16, ParseFailure> {
    let b = self.bytes::<2>(data, off)?;
    Ok(if self.dict.little_endian {
      u16::from_le_bytes(b)
    } else {
      u16::from_be_bytes(b)
    })
  }

  fn u32_at(&self, data: &[u8], off: usize) -> Result<u32, ParseFailure> {
    let b = self.bytes::<4>(data, off)?;
    Ok(if self.dict.little_endian {
      u32::from_le_bytes(b)
    } else {
      u32::from_be_bytes(b)
    })
  }

  fn u64_at(&self, data: &[u8], off: usize) -> Result<u64, ParseFailure> {
    let b = self.bytes::<8>(data, off)?;
    Ok(if self.dict.little_endian {
      u64::from_le_bytes(b)
    } else {
      u64::from_be_bytes(b)
    })
  }

  fn ptr_at(&self, data: &[u8], off: usize) -> Result<u64, ParseFailure> {
    if self.ptr_size == 8 {
      self.u64_at(data, off)
    } else {
      self.u32_at(data, off).map(u64::from)
    }
  }

  // -- data_types.py port --

  fn size_of(&self, t: DType) -> usize {
    let long = if self.dict.bits64 { 8 } else { 4 };
    match t {
      DType::Int | DType::Uint => 4,
      DType::Long | DType::Ulong => long,
      DType::LongLong | DType::UlongLong | DType::Double => 8,
      DType::Ptr => self.ptr_size,
      // Python can't unpack a real long double either: it reads 8 bytes
      // ("d") but skips 16.
      DType::LongDouble => 16,
    }
  }

  /// The 'align' field: `max(4-or-8-by-bitness, sizeof)`.
  fn align_of(&self, t: DType) -> i64 {
    let base = if self.dict.bits64 { 8 } else { 4 };
    base.max(self.size_of(t) as i64)
  }

  /// The 'stack_align' field -- `VA_STACK_ALIGN`/`VA_STACK_MIN_ALIGN` per
  /// arch (`DataTypes.get_stack_min_align` + `get_data_type_align`). Only
  /// its `> 1`-ness matters downstream: it gates whether `align_of`
  /// rounding is applied at all. Quirks (ULONG_LONG mapping to the default
  /// 4, not 8) are reproduced from the reference on purpose.
  fn stack_align_of(&self, t: DType) -> i64 {
    let b64 = self.dict.bits64;
    let (min_align, further) = match self.dict.arch.as_str() {
      "arc" | "x86" => {
        if b64 {
          (8, true)
        } else {
          (1, false)
        }
      }
      "arm64" => (8, true),
      "sparc" | "riscv32e" => (1, false),
      "riscv" => (if b64 { 8 } else { 1 }, true),
      _ => (1, true),
    };
    if !further {
      return min_align;
    }
    match t {
      DType::LongLong => 8,
      DType::Long => {
        if b64 {
          8
        } else {
          4
        }
      }
      _ => 4,
    }
  }

  fn read_arg(&self, arg_list: &[u8], off: usize, t: DType) -> Result<Arg, ParseFailure> {
    Ok(match t {
      DType::Int => Arg::Int(self.u32_at(arg_list, off)? as i32 as i64),
      DType::Uint => Arg::Uint(self.u32_at(arg_list, off)? as u64),
      DType::Long => {
        if self.dict.bits64 {
          Arg::Int(self.u64_at(arg_list, off)? as i64)
        } else {
          Arg::Int(self.u32_at(arg_list, off)? as i32 as i64)
        }
      }
      DType::Ulong => {
        if self.dict.bits64 {
          Arg::Uint(self.u64_at(arg_list, off)?)
        } else {
          Arg::Uint(self.u32_at(arg_list, off)? as u64)
        }
      }
      DType::LongLong => Arg::Int(self.u64_at(arg_list, off)? as i64),
      DType::UlongLong => Arg::Uint(self.u64_at(arg_list, off)?),
      DType::Ptr => Arg::Uint(self.ptr_at(arg_list, off)?),
      // LONG_DOUBLE: read the first 8 bytes as a double (Python does the
      // same -- "probably incorrect, but still have to skip enough bytes").
      DType::Double | DType::LongDouble => {
        let bits = self.u64_at(arg_list, off)?;
        Arg::Double(f64::from_bits(bits))
      }
    })
  }

  /// Port of `__get_string`: dictionary lookup by address first, then the
  /// packaged-string table by va_list index, then the `<string@0x..>`
  /// placeholder.
  fn resolve_string(
    &self,
    addr: u64,
    arg_offset: i64,
    string_tbl: &HashMap<i64, String>,
  ) -> String {
    if let Some(s) = self.dict.find_string(addr) {
      return s;
    }
    let idx = (arg_offset + 2 * self.ptr_size as i64) / self.int_size as i64;
    string_tbl
      .get(&idx)
      .cloned()
      .unwrap_or_else(|| format!("<string@0x{addr:x}>"))
  }

  /// Port of `extract_string_table`: `[index u8][NUL-terminated string]`
  /// pairs, bytes as latin-1 like Python's `chr()` loop.
  fn extract_string_table(tbl_bytes: &[u8]) -> HashMap<i64, String> {
    let mut tbl = HashMap::new();
    let mut cur = String::new();
    let mut idx: i64 = 0;
    let mut expecting_index = true;
    for &b in tbl_bytes {
      if expecting_index {
        idx = b as i64;
        expecting_index = false;
        continue;
      }
      if b == 0 {
        tbl.insert(idx, std::mem::take(&mut cur));
        expecting_index = true;
        continue;
      }
      cur.push(b as char);
    }
    tbl
  }

  /// Port of `process_one_fmt_str`: walks the format string the same way
  /// `cbvprintf_package()` does, pulling each conversion's argument out of
  /// the packaged va_list bytes.
  fn extract_args(
    &self,
    fmt: &[char],
    arg_list: &[u8],
    string_tbl: &HashMap<i64, String>,
  ) -> Result<Vec<Arg>, ParseFailure> {
    let mut args = Vec::new();
    let mut arg_offset: i64 = 0;
    let mut dtype = DType::Int;
    let mut is_parsing = false;
    let mut do_extract = false;

    for idx in 0..fmt.len() {
      let c = fmt[idx];

      if !is_parsing {
        if c == '%' {
          is_parsing = true;
          dtype = DType::Int;
        }
        continue;
      } else if c == '%' {
        // '%%' literal.
        is_parsing = false;
        continue;
      } else if c == '*' {
        // Reference quirk: '*' neither consumes an argument nor ends the
        // spec (see printf.rs's matching quirk note).
      } else if c.is_ascii_digit()
        || c.to_ascii_lowercase() == 'l'
        || matches!(c, ' ' | '#' | '-' | '+' | '.' | 'h')
      {
        // Width/precision/flag/length characters ('L' included via the
        // lowercase check).
        continue;
      } else if matches!(c, 'j' | 'z' | 't') {
        // intmax_t/size_t/ptrdiff_t -- recorded, then (faithfully to the
        // reference) overwritten by the conversion character's own branch.
        dtype = DType::Long;
      } else if matches!(c, 'c' | 'd' | 'i' | 'o' | 'u' | 'x' | 'X') {
        let unsigned = matches!(c, 'c' | 'o' | 'u' | 'x' | 'X');
        let prev = idx.checked_sub(1).map(|i| fmt[i]);
        let prev2 = idx.checked_sub(2).map(|i| fmt[i]);
        dtype = if prev == Some('l') {
          if prev2 == Some('l') {
            if unsigned { DType::UlongLong } else { DType::LongLong }
          } else if unsigned {
            DType::Ulong
          } else {
            DType::Long
          }
        } else if unsigned {
          DType::Uint
        } else {
          DType::Int
        };
        is_parsing = false;
        do_extract = true;
      } else if matches!(c, 's' | 'p' | 'n') {
        dtype = DType::Ptr;
        is_parsing = false;
        do_extract = true;
      } else if matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'f' | 'g') {
        dtype = if idx.checked_sub(1).map(|i| fmt[i]) == Some('L') {
          DType::LongDouble
        } else {
          DType::Double
        };
        is_parsing = false;
        do_extract = true;
      } else {
        is_parsing = false;
        continue;
      }

      if do_extract {
        do_extract = false;

        let align = self.align_of(dtype);
        let size = self.size_of(dtype) as i64;
        let stack_align = self.stack_align_of(dtype);

        if stack_align > 1 {
          arg_offset = (arg_offset + (align - 1)) / align * align;
        }

        if arg_offset < 0 {
          return Err(ParseFailure::Malformed(
            "negative va_list offset".to_string(),
          ));
        }

        let arg = self.read_arg(arg_list, arg_offset as usize, dtype)?;
        let arg = if c == 's' {
          let addr = match arg {
            Arg::Uint(v) => v,
            _ => 0,
          };
          Arg::Str(self.resolve_string(addr, arg_offset, string_tbl))
        } else {
          arg
        };
        args.push(arg);

        arg_offset += size;
        if stack_align > 1 {
          arg_offset = (arg_offset + align - 1) / align * align;
        }
      }
    }

    Ok(args)
  }

  /// Byte length of `type + header + timestamp`.
  fn full_hdr_size(&self) -> usize {
    1 + 1 + 2 + 2 + self.ptr_size + self.ts_size
  }

  /// Port of `parse_one_normal_msg`; `offset` points just past the type
  /// byte, caller has already verified the full message is in-bounds.
  fn parse_normal(&self, data: &[u8], mut offset: usize) -> Result<(LogMessage, usize), ParseFailure> {
    let domain_lvl = self.u8_at(data, offset)?;
    let pkg_len = self.u16_at(data, offset + 1)? as usize;
    let data_len = self.u16_at(data, offset + 3)? as usize;
    let source_id = self.ptr_at(data, offset + 5)?;
    offset += 5 + self.ptr_size;

    let timestamp = if self.ts_size == 8 {
      self.u64_at(data, offset)?
    } else {
      self.u32_at(data, offset)? as u64
    };
    offset += self.ts_size;

    let (domain, level) = if self.dict.little_endian {
      (domain_lvl & 0x0f, (domain_lvl >> 4) & 0x0f)
    } else {
      ((domain_lvl >> 4) & 0x0f, domain_lvl & 0x0f)
    };

    let pkg_start = offset;
    let next_msg_offset = pkg_start + pkg_len + data_len;

    // cbprintf package header: [0] total arg-area length in 32-bit words,
    // [1] appended-string count, [2]/[3] ro/rw string index counts.
    let mut end_of_args =
      pkg_start + self.u8_at(data, pkg_start)? as usize * self.int_size;
    let num_packed_strings = self.u8_at(data, pkg_start + 1)? as usize;
    end_of_args += self.u8_at(data, pkg_start + 2)? as usize; // ro indexes
    end_of_args += self.u8_at(data, pkg_start + 3)? as usize; // rw indexes

    let pkg_end = pkg_start + pkg_len;
    if end_of_args > pkg_end || pkg_end > data.len() {
      return Err(ParseFailure::Malformed(
        "cbprintf package lengths out of bounds".to_string(),
      ));
    }

    let string_tbl = Self::extract_string_table(&data[end_of_args..pkg_end]);
    if string_tbl.len() != num_packed_strings {
      return Err(ParseFailure::Malformed(
        "packaged string table count mismatch".to_string(),
      ));
    }

    // Skip the package header (one pointer width), then the format-string
    // pointer itself. The fmt string's own table index sits one pointer
    // *before* the va_list, hence the negative offset.
    let fmt_ptr = self.ptr_at(data, pkg_start + self.ptr_size)?;
    let fmt_str = self.resolve_string(fmt_ptr, -(self.ptr_size as i64), &string_tbl);
    if fmt_str.is_empty() {
      return Err(ParseFailure::Malformed(format!(
        "empty format string at 0x{fmt_ptr:x}"
      )));
    }

    let args_start = pkg_start + 2 * self.ptr_size;
    if args_start > end_of_args {
      return Err(ParseFailure::Malformed(
        "cbprintf package too short for its header".to_string(),
      ));
    }
    let fmt_chars: Vec<char> = fmt_str.chars().collect();
    let args = self.extract_args(&fmt_chars, &data[args_start..end_of_args], &string_tbl)?;

    let text = printf::format_message(&fmt_str, &args);
    let hexdump = data[pkg_end..next_msg_offset].to_vec();

    Ok((
      LogMessage {
        timestamp,
        domain,
        level,
        source: self.dict.source_name(domain, source_id),
        text,
        hexdump,
      },
      next_msg_offset,
    ))
  }

  /// Port of `parse_one_msg`: returns the event plus the offset of the next
  /// message.
  fn parse_one(&self, data: &[u8], offset: usize) -> Result<(LogEvent, usize), ParseFailure> {
    let msg_type = self.u8_at(data, offset)?;

    match msg_type {
      MSG_TYPE_DROPPED => {
        let count = self.u16_at(data, offset + 1)?;
        Ok((LogEvent::Dropped(count), offset + 3))
      }
      MSG_TYPE_NORMAL => {
        if offset + self.full_hdr_size() > data.len() {
          return Err(ParseFailure::Incomplete);
        }
        let pkg_len = self.u16_at(data, offset + 2)? as usize;
        let data_len = self.u16_at(data, offset + 4)? as usize;
        if offset + self.full_hdr_size() + pkg_len + data_len > data.len() {
          return Err(ParseFailure::Incomplete);
        }
        let (msg, next) = self.parse_normal(data, offset + 1)?;
        Ok((LogEvent::Message(msg), next))
      }
      other => Err(ParseFailure::Malformed(format!(
        "unknown message type {other}"
      ))),
    }
  }
}

/// Decodes dovecote's stored chunks (already base64-decoded to raw bytes,
/// oldest first) against `dict`. Chunks are parsed as one concatenated
/// stream -- a message may straddle a flush boundary -- and a corrupt or
/// truncated region yields one [`LogEvent::Error`] with decoding resumed at
/// the next chunk boundary, so one bad flush doesn't hide the rest of the
/// buffer.
pub fn decode_chunks(dict: &LogDictionary, chunks: &[Vec<u8>]) -> Vec<LogEvent> {
  let mut data = Vec::with_capacity(chunks.iter().map(Vec::len).sum());
  let mut boundaries = Vec::with_capacity(chunks.len());
  for chunk in chunks {
    boundaries.push(data.len());
    data.extend_from_slice(chunk);
  }

  let decoder = Decoder::new(dict);
  let mut events = Vec::new();
  let mut offset = 0usize;

  while offset < data.len() {
    match decoder.parse_one(&data, offset) {
      Ok((event, next)) => {
        // A parser that doesn't advance would loop forever; treat it as
        // malformed (cannot happen with well-formed input).
        if next <= offset {
          events.push(LogEvent::Error {
            offset,
            reason: "parser made no progress".to_string(),
          });
          break;
        }
        events.push(event);
        offset = next;
      }
      Err(failure) => {
        let reason = match failure {
          ParseFailure::Incomplete => "truncated message".to_string(),
          ParseFailure::Malformed(r) => r,
        };
        // Resync at the next chunk boundary after the failure point; the
        // ring buffer flushes whole ranges, so the next chunk usually
        // starts on a clean message boundary.
        match boundaries.iter().copied().find(|&b| b > offset) {
          Some(resume) => {
            events.push(LogEvent::Error {
              offset,
              reason: format!("{reason}; skipped {} bytes to next chunk", resume - offset),
            });
            offset = resume;
          }
          None => {
            events.push(LogEvent::Error {
              offset,
              reason: format!("{reason}; {} bytes undecoded at end of stream", data.len() - offset),
            });
            break;
          }
        }
      }
    }
  }

  events
}
