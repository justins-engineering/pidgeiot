//! The v3 dictionary database (`log_dictionary.json`) -- a Rust port of
//! Zephyr's `dictionary_parser/log_database.py` reader, restricted to what
//! decoding needs: target layout, kconfigs, log-source names, and string
//! lookup (address -> string via `string_mappings`, with the same
//! partial/suffix matching the reference does, plus optional binary ELF
//! `sections` scanning).

use super::base64;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct RawTarget {
  little_endianness: bool,
  bits: u32,
}

#[derive(Deserialize)]
struct RawInstance {
  name: String,
}

#[derive(Deserialize)]
struct RawLogSubsys {
  #[serde(default)]
  log_instances: HashMap<String, RawInstance>,
}

#[derive(Deserialize)]
struct RawSection {
  start: u64,
  size: u64,
  data_b64: String,
}

#[derive(Deserialize)]
struct RawDb {
  version: i64,
  #[serde(default)]
  build_id: Option<serde_json::Value>,
  #[serde(default)]
  arch: Option<String>,
  target: RawTarget,
  #[serde(default)]
  kconfigs: HashMap<String, serde_json::Value>,
  #[serde(default)]
  log_subsys: Option<RawLogSubsys>,
  #[serde(default)]
  string_mappings: Option<HashMap<String, String>>,
  #[serde(default)]
  sections: Option<HashMap<String, RawSection>>,
}

/// One extracted ELF string section (only present in databases built where
/// `database_gen.py` fell back to embedding raw section bytes).
struct Section {
  start: u64,
  data: Vec<u8>,
}

/// A parsed, decode-ready v3 dictionary database. (No `version` field:
/// `parse` rejects everything but v3, so it would always be 3.)
pub struct LogDictionary {
  pub build_id: Option<String>,
  pub arch: String,
  pub little_endian: bool,
  pub bits64: bool,
  /// `CONFIG_LOG_TIMESTAMP_64BIT` -- selects the on-wire timestamp width.
  pub ts_64bit: bool,
  /// source_id -> log source (module) name. Keyed by source id alone, like
  /// the reference (`get_log_source_string` ignores domain for the lookup).
  sources: HashMap<u64, String>,
  /// (address, string), sorted by address for the partial-match scan.
  string_mappings: Vec<(u64, String)>,
  sections: Vec<Section>,
}

impl LogDictionary {
  /// Parses `log_dictionary.json` text. Errors are human-readable strings
  /// (surfaced verbatim in the dashboard's upload/validation UI).
  pub fn parse(json_text: &str) -> Result<Self, String> {
    let raw: RawDb =
      serde_json::from_str(json_text).map_err(|e| format!("not a dictionary database: {e}"))?;

    // v1/v2 databases predate this fleet's oldest firmware (Zephyr v3.x+
    // emits v3); supporting their different header layout would be dead
    // code we could never fixture-test against a real build.
    if raw.version != 3 {
      return Err(format!(
        "unsupported dictionary version {} (only version 3, Zephyr v3.x+, is supported)",
        raw.version
      ));
    }

    let bits64 = match raw.target.bits {
      32 => false,
      64 => true,
      other => return Err(format!("unsupported target bitness {other}")),
    };

    let build_id = raw.build_id.and_then(|v| match v {
      serde_json::Value::String(s) => Some(s),
      serde_json::Value::Number(n) => Some(n.to_string()),
      _ => None,
    });

    let sources = raw
      .log_subsys
      .map(|ls| {
        ls.log_instances
          .into_iter()
          .filter_map(|(id, inst)| id.parse::<u64>().ok().map(|id| (id, inst.name)))
          .collect()
      })
      .unwrap_or_default();

    let mut string_mappings: Vec<(u64, String)> = raw
      .string_mappings
      .unwrap_or_default()
      .into_iter()
      // JSON object keys are decimal address strings (log_database.py
      // converts them back with int() on read, same as here).
      .filter_map(|(addr, s)| addr.parse::<u64>().ok().map(|a| (a, s)))
      .collect();
    string_mappings.sort_unstable_by_key(|(a, _)| *a);

    let mut sections = Vec::new();
    for (name, sect) in raw.sections.unwrap_or_default() {
      let data = base64::decode(&sect.data_b64)
        .ok_or_else(|| format!("section '{name}' carries invalid base64 data"))?;
      // `size` is authoritative in the reference reader; trust the decoded
      // bytes but never scan past the declared size.
      let size = (sect.size as usize).min(data.len());
      sections.push(Section {
        start: sect.start,
        data: data[..size].to_vec(),
      });
    }

    let ts_64bit = raw.kconfigs.contains_key("CONFIG_LOG_TIMESTAMP_64BIT");

    Ok(Self {
      build_id,
      arch: raw.arch.unwrap_or_default(),
      little_endian: raw.target.little_endianness,
      bits64,
      ts_64bit,
      sources,
      string_mappings,
      sections,
    })
  }

  /// Port of `LogDatabase.find_string`: exact `string_mappings` hit, then
  /// the combined-string suffix scan (`ptr <= addr < ptr + len`), then the
  /// binary sections. `None` if nowhere.
  pub fn find_string(&self, addr: u64) -> Option<String> {
    if let Ok(idx) = self
      .string_mappings
      .binary_search_by_key(&addr, |(a, _)| *a)
    {
      return Some(self.string_mappings[idx].1.clone());
    }

    // Partial match: `addr` may point into the middle of a longer mapped
    // string (compiler-merged literals). The reference scans every entry
    // (`ptr <= str_ptr < ptr + len(string)`) in arbitrary dict order and
    // returns the first hit; when regions overlap we deterministically
    // prefer the closest (largest) start instead, which is the more
    // specific suffix. Offsets are in characters, matching Python's
    // codepoint slicing -- mapped strings are ASCII in practice.
    let candidates = &self.string_mappings[..self.string_mappings.partition_point(|(a, _)| *a < addr)];
    for (start, s) in candidates.iter().rev() {
      let offset = (addr - start) as usize;
      if offset < s.chars().count() {
        return Some(s.chars().skip(offset).collect());
      }
    }

    self.find_string_in_sections(addr)
  }

  fn find_string_in_sections(&self, addr: u64) -> Option<String> {
    for sect in &self.sections {
      if addr < sect.start {
        continue;
      }
      let offset = (addr - sect.start) as usize;
      if offset >= sect.data.len() {
        continue;
      }
      // NUL-terminated scan, bytes rendered as latin-1 like the reference's
      // chr() loop.
      let s: String = sect.data[offset..]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
      return Some(s);
    }
    None
  }

  /// Port of `get_log_source_string`.
  pub fn source_name(&self, domain_id: u8, source_id: u64) -> String {
    match self.sources.get(&source_id) {
      Some(name) => name.clone(),
      None => format!("unknown<{domain_id}:{source_id}>"),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn minimal(extra: &str) -> String {
    format!(
      r#"{{"version": 3, "target": {{"little_endianness": true, "bits": 32}},
          "build_id": "test", "arch": "arm", "kconfigs": {{}},
          "log_subsys": {{"log_instances": {{"1": {{"source_id": 1, "name": "app", "level": 4, "addr": 100}}}}}}
          {extra}}}"#
    )
  }

  #[test]
  fn parses_minimal_database() {
    let d = LogDictionary::parse(&minimal("")).unwrap();
    assert_eq!(d.build_id.as_deref(), Some("test"));
    assert_eq!(d.arch, "arm");
    assert!(!d.bits64);
    assert!(!d.ts_64bit);
    assert_eq!(d.source_name(0, 1), "app");
    assert_eq!(d.source_name(2, 9), "unknown<2:9>");
  }

  #[test]
  fn numeric_build_id_is_stringified() {
    let json = minimal("").replacen("\"test\"", "12345", 1);
    let d = LogDictionary::parse(&json).unwrap();
    assert_eq!(d.build_id.as_deref(), Some("12345"));
  }

  #[test]
  fn timestamp_width_follows_kconfig() {
    let json = minimal("").replacen(
      r#""kconfigs": {}"#,
      r#""kconfigs": {"CONFIG_LOG_TIMESTAMP_64BIT": "y"}"#,
      1,
    );
    assert!(LogDictionary::parse(&json).unwrap().ts_64bit);
  }

  #[test]
  fn string_mapping_exact_and_partial_match() {
    let d = LogDictionary::parse(&minimal(
      r#", "string_mappings": {"1000": "hello world", "2000": "other"}"#,
    ))
    .unwrap();
    assert_eq!(d.find_string(1000).as_deref(), Some("hello world"));
    assert_eq!(d.find_string(1006).as_deref(), Some("world"));
    assert_eq!(d.find_string(2000).as_deref(), Some("other"));
    assert_eq!(d.find_string(1011), None); // one past the end
    assert_eq!(d.find_string(999), None);
    assert_eq!(d.find_string(3000), None);
  }

  #[test]
  fn section_scan_finds_nul_terminated_strings() {
    // "abc\0def\0" at 0x100, base64 "YWJjAGRlZgA=".
    let d = LogDictionary::parse(&minimal(
      r#", "sections": {"rodata": {"start": 256, "size": 8, "percent_used": "1.0", "data_b64": "YWJjAGRlZgA="}}"#,
    ))
    .unwrap();
    assert_eq!(d.find_string(256).as_deref(), Some("abc"));
    assert_eq!(d.find_string(257).as_deref(), Some("bc"));
    assert_eq!(d.find_string(260).as_deref(), Some("def"));
    assert_eq!(d.find_string(300), None);
  }

  #[test]
  fn rejects_non_v3_and_garbage() {
    assert!(LogDictionary::parse("{}").is_err());
    assert!(LogDictionary::parse("not json").is_err());
    let v1 = minimal("").replacen("\"version\": 3", "\"version\": 1", 1);
    assert!(LogDictionary::parse(&v1).is_err());
  }
}
