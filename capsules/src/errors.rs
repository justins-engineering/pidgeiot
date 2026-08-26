//! Client error-report types, normalizers, and signature grouping --
//! shared by dovecote's `POST /errors` route and fancier's panic hook /
//! JS shim payload shape.
//!
//! Pure string logic lives here (not in dovecote) for the same reason
//! `feedback`'s email formatter does: `dovecote` is a wasm-only `cdylib`
//! whose unit tests can't run on a host target, and this crate's
//! `cargo test -p capsules` can. It also keeps fancier and dovecote from
//! ever disagreeing about what a normalized route or a signature is --
//! both sides call the same functions.
//!
//! The server treats every field of an incoming report as untrusted: it
//! re-normalizes the message and route itself before storing, never
//! accepts a client-computed signature (there is deliberately no signature
//! field on `ErrorReport`), and validates `build` against the release
//! artifact's known hash shape.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Cap on the entire raw `POST /errors` request body, checked before any
/// parsing -- an abuse cap in the same spirit as `MAX_FEEDBACK_BODY_BYTES`.
pub const MAX_ERROR_REPORT_BYTES: usize = 16 * 1024;

/// Cap on the error `message` field. A panic payload or JS error message
/// is one line; anything longer is being smuggled, not reported.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 2 * 1024;

/// Cap on a JS `stack` string. Real browser stacks fit comfortably.
pub const MAX_ERROR_STACK_BYTES: usize = 8 * 1024;

/// Breadcrumb ring size, and the most breadcrumbs a report may carry.
pub const MAX_ERROR_BREADCRUMBS: usize = 20;

/// Cap on a single breadcrumb's `detail` string -- method + route template
/// + status, never bodies, so this is generous already.
pub const MAX_ERROR_BREADCRUMB_DETAIL_BYTES: usize = 160;

/// Cap on `location` (`file:line:col`), `route`, and `user_agent` fields.
pub const MAX_ERROR_FIELD_BYTES: usize = 256;

/// Client-side budget: at most this many automatic reports per page load.
/// A Rust panic can only ever produce one (the module is dead afterward);
/// this bounds the JS-exception-in-a-loop case.
pub const MAX_ERROR_REPORTS_PER_PAGE: usize = 5;

/// Which capture mechanism produced a report. Wire values are snake_case;
/// an unknown value fails deserialization and the route answers 400.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
  RustPanic,
  WasmBoot,
  JsException,
  UnhandledRejection,
  ApiFailure,
  /// Thrown by a script served from an origin other than the page's own
  /// -- an analytics beacon, a browser extension. Counted, never mailed:
  /// nothing in this codebase can fix it, and a minified third-party
  /// column moves with every one of their releases.
  ThirdParty,
  /// The browser failed the pre-boot capability probe, so the wasm bundle
  /// was never going to instantiate. Counted so the size of that audience
  /// is known; not a defect, so never mailed.
  UnsupportedBrowser,
}

impl ErrorKind {
  pub fn as_str(&self) -> &'static str {
    match self {
      ErrorKind::RustPanic => "rust_panic",
      ErrorKind::WasmBoot => "wasm_boot",
      ErrorKind::JsException => "js_exception",
      ErrorKind::UnhandledRejection => "unhandled_rejection",
      ErrorKind::ApiFailure => "api_failure",
      ErrorKind::ThirdParty => "third_party",
      ErrorKind::UnsupportedBrowser => "unsupported_browser",
    }
  }

  /// Whether a first sighting of this kind is worth an ops email. The two
  /// counting-only kinds describe the visitor's environment, not our code.
  pub fn notifies(self) -> bool {
    !matches!(self, ErrorKind::ThirdParty | ErrorKind::UnsupportedBrowser)
  }
}

/// Whose code an error came from, judged by the origin of its location
/// or, failing that, of the top stack frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorSource {
  /// The page's own origin, a wasm frame, a blob the page minted -- or no
  /// URL to judge by at all. Unknown counts as ours on purpose: a failure
  /// in our own glue must never be dropped for want of a filename.
  Own,
  /// A script from another origin, named so a folded group keys on the
  /// origin rather than on a minified column.
  Foreign(String),
}

/// Closed scheme lists: a bare `main.js:1:2` location would otherwise
/// read as scheme `main.js`.
const WEB_SCHEMES: [&str; 2] = ["http", "https"];
const EXTENSION_SCHEMES: [&str; 6] = [
  "chrome-extension",
  "moz-extension",
  "safari-extension",
  "safari-web-extension",
  "ms-browser-extension",
  "webkit-masked-url",
];

/// Classifies a report by where its code ran. `own_origins` are the page
/// origins we serve (dovecote passes `ROOT_URL`); comparison is on
/// `scheme://host[:port]`, case-insensitive, trailing slash ignored.
pub fn error_source(
  own_origins: &[&str],
  location: Option<&str>,
  stack: Option<&str>,
) -> ErrorSource {
  let url = location
    .and_then(first_url)
    .or_else(|| stack.and_then(top_frame_url));
  match url {
    Some(url) => classify_url(own_origins, &url),
    None => ErrorSource::Own,
  }
}

fn is_known_scheme(scheme: &str) -> bool {
  WEB_SCHEMES.contains(&scheme)
    || EXTENSION_SCHEMES.contains(&scheme)
    || scheme == "blob"
    || scheme == "wasm"
}

/// The first token carrying a known scheme. Tokens split on the
/// punctuation V8 and Gecko wrap frame URLs in.
fn first_url(text: &str) -> Option<String> {
  text
    .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>' | ','))
    .find_map(|token| {
      let (scheme, _) = token.split_once(':')?;
      let scheme = scheme.to_ascii_lowercase();
      is_known_scheme(&scheme).then(|| token.to_string())
    })
}

/// The top frame's URL. V8 prefixes frames with `at `; Gecko and WebKit
/// write `function@url:line:col`, so only the part after the last `@` is
/// searched there -- which is also what keeps a URL or an address quoted
/// in the message line from being mistaken for a frame.
fn top_frame_url(stack: &str) -> Option<String> {
  stack.lines().find_map(|line| {
    let line = line.trim();
    if let Some(frame) = line.strip_prefix("at ") {
      first_url(frame)
    } else if let Some((_, after)) = line.rsplit_once('@') {
      first_url(after)
    } else {
      None
    }
  })
}

/// `scheme://authority` for a URL known to start with `scheme:`.
fn origin_of(scheme: &str, rest: &str) -> Option<String> {
  let authority = rest.strip_prefix("//")?;
  let end = authority
    .find(|c: char| matches!(c, '/' | '?' | '#'))
    .unwrap_or(authority.len());
  let authority = &authority[..end];
  if authority.is_empty() {
    return None;
  }
  Some(format!("{scheme}://{}", authority.to_ascii_lowercase()))
}

fn is_own_origin(own_origins: &[&str], origin: &str) -> bool {
  own_origins.iter().any(|own| {
    own
      .trim()
      .trim_end_matches('/')
      .eq_ignore_ascii_case(origin)
  })
}

fn classify_url(own_origins: &[&str], url: &str) -> ErrorSource {
  let Some((scheme, rest)) = url.split_once(':') else {
    return ErrorSource::Own;
  };
  let scheme = scheme.to_ascii_lowercase();
  if scheme == "wasm" {
    return ErrorSource::Own;
  }
  // A blob URL names the origin that minted it: `blob:https://host/id`.
  let (scheme, rest) = if scheme == "blob" {
    match rest.split_once(':') {
      Some((inner, inner_rest)) => (inner.to_ascii_lowercase(), inner_rest),
      None => return ErrorSource::Own,
    }
  } else {
    (scheme, rest)
  };
  if WEB_SCHEMES.contains(&scheme.as_str()) {
    return match origin_of(&scheme, rest) {
      Some(origin) if is_own_origin(own_origins, &origin) => ErrorSource::Own,
      Some(origin) => ErrorSource::Foreign(origin),
      None => ErrorSource::Own,
    };
  }
  if EXTENSION_SCHEMES.contains(&scheme.as_str()) {
    return ErrorSource::Foreign(
      origin_of(&scheme, rest).unwrap_or_else(|| format!("{scheme}://")),
    );
  }
  ErrorSource::Own
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BreadcrumbKind {
  Nav,
  Api,
  Ui,
}

/// One bit of session context -- never an identity. The server resolves a
/// real user id only on the identified manual path, never from this.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
  SignedIn,
  Anonymous,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Breadcrumb {
  /// Milliseconds before the error, not a wall clock -- the trail's shape
  /// matters, absolute times don't.
  pub age_ms: u32,
  pub kind: BreadcrumbKind,
  /// Shape only -- method, route template, status. Never request or
  /// response bodies, and the server treats it as untrusted free text.
  pub detail: String,
}

/// The automatic-report envelope. `deny_unknown_fields` is load-bearing:
/// `text/plain` is CORS-safelisted, so any page on the internet can POST
/// it cross-origin with credentials -- the anonymous ingest branch must
/// have no way to smuggle an identity claim, and a body carrying extra
/// fields (a note, a user id, contact details) fails parsing outright
/// instead of being partially honored.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ErrorReport {
  pub kind: ErrorKind,
  pub message: String,
  /// `src/views/pigeon.rs:412:18` for a panic; `file:line:col` for JS.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub location: Option<String>,
  /// JS stack when there is one; wasm panics have none (panic=abort, no
  /// unwinding, no backtrace at any price).
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub stack: Option<String>,
  pub route: String,
  /// The release artifact's content hash (`dxh` + unpadded u64 hex). The
  /// server blanks anything not matching that shape.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub build: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub user_agent: Option<String>,
  #[serde(default)]
  pub breadcrumbs: Vec<Breadcrumb>,
  pub session_kind: SessionKind,
  pub occurred_at_ms: u64,
  /// Client-minted correlation id shown on the crash screen, so a user's
  /// follow-up note can be joined to the crash it describes. A hint, not
  /// a key -- ids are attacker-reusable, so notes attach alongside, never
  /// overwrite.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub client_event_id: Option<Uuid>,
}

/// The identified manual body -- accepted only as `application/json`
/// (preflighted, CORS-gated to `ROOT_URL`), which is what makes an
/// identified report unforgeable cross-origin. Carries the full report so
/// the note lands in the same signature group as the crash it describes.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ErrorNoteRequest {
  pub note: String,
  pub report: ErrorReport,
}

/// Truncates on a char boundary so a multibyte cap never panics.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
  if s.len() <= max {
    return s;
  }
  let mut end = max;
  while end > 0 && !s.is_char_boundary(end) {
    end -= 1;
  }
  &s[..end]
}

fn is_hex_char(c: char) -> bool {
  c.is_ascii_hexdigit()
}

fn is_uuid_shaped(s: &str) -> bool {
  let b = s.as_bytes();
  if b.len() != 36 {
    return false;
  }
  s.char_indices().all(|(i, c)| match i {
    8 | 13 | 18 | 23 => c == '-',
    _ => is_hex_char(c),
  })
}

/// Normalizes a location path into a route template: query string and
/// fragment dropped entirely (never normalized -- `?flow=`/`?token=` must
/// not land in an error store), and identifier-shaped segments replaced
/// with placeholders. Doubles as grouping key material and as the thing
/// that strips tenant identifiers out of stored reports.
pub fn normalize_route(path: &str) -> String {
  let path = path.split(['?', '#']).next().unwrap_or("").trim();
  let mut out = String::with_capacity(path.len().min(MAX_ERROR_FIELD_BYTES));
  for segment in path.split('/') {
    if segment.is_empty() {
      continue;
    }
    out.push('/');
    if is_uuid_shaped(segment) {
      out.push_str(":uuid");
    } else if segment.len() >= 16 && segment.chars().all(is_hex_char) {
      out.push_str(":hex");
    } else if segment.chars().all(|c| c.is_ascii_digit()) {
      out.push_str(":int");
    } else {
      out.push_str(truncate_bytes(segment, 64));
    }
  }
  if out.is_empty() {
    out.push('/');
  }
  truncate_bytes(&out, MAX_ERROR_FIELD_BYTES).to_string()
}

/// Replaces identifier- and secret-shaped substrings with placeholders:
/// UUIDs, email addresses, long hex runs, long base64-ish runs, and bare
/// integers. Two jobs in one pass -- grouping (`pigeon <uuid> not found`
/// is one group, not one per pigeon) and redaction (a panic message that
/// interpolated an email or a token must not be retained verbatim,
/// especially as the group exemplar, which is kept indefinitely). This
/// function is the control; the "panic messages must not interpolate user
/// data" rule is only a hope about future code.
pub fn normalize_message(message: &str) -> String {
  let message = truncate_bytes(message.trim(), MAX_ERROR_MESSAGE_BYTES);
  // Emails first, as their own pass -- an address spans '.' and '@', which
  // the run pass below treats as boundaries, so it would only ever see the
  // pieces and could leave the half a redaction exists to remove.
  classify_runs(&redact_emails(message))
}

fn redact_emails(s: &str) -> String {
  let chars: Vec<char> = s.chars().collect();
  let mut out = String::with_capacity(s.len());
  // Char index from which `out`'s tail mirrors `chars` verbatim -- the
  // local part is emitted before its '@' is reached, so replacing it means
  // truncating `out`, which is only sound over the verbatim tail.
  let mut verbatim_from = 0usize;
  let mut i = 0;
  while i < chars.len() {
    if chars[i] == '@'
      && let Some((start, end)) = email_span(&chars, i)
      && start >= verbatim_from
    {
      let local_bytes: usize = chars[start..i].iter().map(|c| c.len_utf8()).sum();
      out.truncate(out.len() - local_bytes);
      out.push_str("<email>");
      i = end;
      verbatim_from = end;
      continue;
    }
    out.push(chars[i]);
    i += 1;
  }
  out
}

fn classify_runs(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  let mut run = String::new();
  for c in s.chars() {
    if is_run_char(c) {
      run.push(c);
    } else {
      if !run.is_empty() {
        out.push_str(&classify_run(&run));
        run.clear();
      }
      out.push(c);
    }
  }
  if !run.is_empty() {
    out.push_str(&classify_run(&run));
  }
  out
}

/// Characters that can form an identifier/token run. Deliberately
/// excludes '.' so prose and file paths survive; a dotted JWT still loses
/// each of its three segments to the base64 rule.
fn is_run_char(c: char) -> bool {
  c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-')
}

fn classify_run(run: &str) -> String {
  if is_uuid_shaped(run) {
    return "<uuid>".to_string();
  }
  let len = run.chars().count();
  let all_hex = run.chars().all(|c| is_hex_char(c) || c == '-');
  if all_hex && len >= 16 && run.chars().any(is_hex_char) {
    return "<hex>".to_string();
  }
  // Base64-ish: long, token-charset, and carrying at least one digit --
  // the digit requirement is what keeps long English words (and
  // identifiers like `unhandled_rejection`) out of the net.
  if len >= 24 && run.chars().any(|c| c.is_ascii_digit()) {
    return "<b64>".to_string();
  }
  if !run.is_empty() && run.chars().all(|c| c.is_ascii_digit()) {
    return "<int>".to_string();
  }
  // Mixed word with digits (e.g. `utf8`, `sha256`) stays -- it's part of
  // the message's identity, not an identifier.
  run.to_string()
}

/// If `chars[i]` is the '@' of an email-shaped substring, returns the full
/// address's span. Requires a nonempty local part and a dotted domain, so
/// a bare '@' in prose stays put.
fn email_span(chars: &[char], i: usize) -> Option<(usize, usize)> {
  if chars[i] != '@' {
    return None;
  }
  let local_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-');
  let domain_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
  let mut start = i;
  while start > 0 && local_char(chars[start - 1]) {
    start -= 1;
  }
  if start == i {
    return None;
  }
  let mut end = i + 1;
  while end < chars.len() && domain_char(chars[end]) {
    end += 1;
  }
  let domain: String = chars[i + 1..end].iter().collect();
  if domain.contains('.') && domain.len() >= 3 {
    Some((start, end))
  } else {
    None
  }
}

/// The grouping key: a truncated SHA-256 over kind + normalized message +
/// location. Line numbers in `location` mean a group's identity doesn't
/// survive a refactor -- deliberate: a new build's recurrence surfaces as
/// a new group with a fresh alert instead of silently merging into one
/// someone already marked resolved.
pub fn error_signature(
  kind: ErrorKind,
  normalized_message: &str,
  location: Option<&str>,
) -> String {
  let mut hasher = Sha256::new();
  hasher.update(kind.as_str().as_bytes());
  hasher.update([0u8]);
  hasher.update(normalized_message.as_bytes());
  hasher.update([0u8]);
  hasher.update(location.unwrap_or("").as_bytes());
  let digest = hasher.finalize();
  digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// `dx` names the release wasm `fancier_bg-dxh<hex>.wasm`; that hash is
/// the build identity. The hex run is a u64 formatted WITHOUT zero
/// padding, so its length varies up to 16 -- an exact-16 check rejects
/// real builds. Anything not matching gets blanked rather than stored, so
/// "which builds still throw this" stays meaningful.
pub fn is_valid_build(build: &str) -> bool {
  let Some(hex) = build.strip_prefix("dxh") else {
    return false;
  };
  (1..=16).contains(&hex.len())
    && hex
      .chars()
      .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn route_replaces_uuid_and_hex_segments() {
    assert_eq!(
      normalize_route(
        "/flocks/c84932d0-1a2b-4c3d-8e4f-567890abcdef/pigeons/59d0c929aabbccddeeff00112233445566778899aabbccddeeff001122334455"
      ),
      "/flocks/:uuid/pigeons/:hex"
    );
  }

  #[test]
  fn route_drops_query_and_fragment() {
    assert_eq!(
      normalize_route("/registration?flow=abc-123-secret#top"),
      "/registration"
    );
    assert_eq!(normalize_route("/invite?token=xyz"), "/invite");
  }

  #[test]
  fn route_replaces_bare_integers() {
    assert_eq!(normalize_route("/things/42/detail"), "/things/:int/detail");
  }

  #[test]
  fn route_root_and_empty_stay_root() {
    assert_eq!(normalize_route("/"), "/");
    assert_eq!(normalize_route(""), "/");
    assert_eq!(normalize_route("/dashboard"), "/dashboard");
  }

  #[test]
  fn message_replaces_uuids_and_integers() {
    assert_eq!(
      normalize_message("pigeon 59d0c929-13d2-4f60-a1a2-93b1a86f0f11 not found (attempt 3)"),
      "pigeon <uuid> not found (attempt <int>)"
    );
  }

  #[test]
  fn message_groups_two_ids_into_one_signature() {
    let a = normalize_message("pigeon 59d0c929-13d2-4f60-a1a2-93b1a86f0f11 not found");
    let b = normalize_message("pigeon a3f21b04-8c1e-4d2a-9b3c-1234567890ab not found");
    assert_eq!(a, b);
    assert_eq!(
      error_signature(ErrorKind::RustPanic, &a, Some("src/views/pigeon.rs:412:18")),
      error_signature(ErrorKind::RustPanic, &b, Some("src/views/pigeon.rs:412:18")),
    );
  }

  #[test]
  fn message_redacts_email_addresses() {
    assert_eq!(
      normalize_message("no identity for owner.name+tag@example-corp.com here"),
      "no identity for <email> here"
    );
  }

  #[test]
  fn message_redacts_long_hex_runs() {
    assert_eq!(
      normalize_message("token 59d0c929aabbccddeeff0011 rejected"),
      "token <hex> rejected"
    );
  }

  #[test]
  fn message_redacts_base64_runs() {
    assert_eq!(
      normalize_message("bearer AQhkZXZpY2UtMDLCoTQ9x71fXk02 expired"),
      "bearer <b64> expired"
    );
  }

  #[test]
  fn message_keeps_prose_and_short_words_with_digits() {
    assert_eq!(
      normalize_message("called Option::unwrap() on a None value"),
      "called Option::unwrap() on a None value"
    );
    assert_eq!(
      normalize_message("invalid utf8 in sha256 input"),
      "invalid utf8 in sha256 input"
    );
    // Long snake_case identifiers carry no digits, so they survive.
    assert_eq!(
      normalize_message("unhandled_rejection_from_somewhere_long happened"),
      "unhandled_rejection_from_somewhere_long happened"
    );
  }

  #[test]
  fn message_is_byte_capped() {
    let long = "x".repeat(MAX_ERROR_MESSAGE_BYTES + 100);
    assert!(normalize_message(&long).len() <= MAX_ERROR_MESSAGE_BYTES);
  }

  #[test]
  fn signature_differs_by_kind_and_location() {
    let msg = "boom";
    let a = error_signature(ErrorKind::RustPanic, msg, Some("a.rs:1:1"));
    let b = error_signature(ErrorKind::JsException, msg, Some("a.rs:1:1"));
    let c = error_signature(ErrorKind::RustPanic, msg, Some("a.rs:2:1"));
    assert_ne!(a, b);
    assert_ne!(a, c);
    assert_eq!(a.len(), 32);
  }

  #[test]
  fn build_shape_validation() {
    assert!(is_valid_build("dxh7a1e5a63c0523eb1"));
    // Unpadded u64 hex: shorter runs are real builds, not noise.
    assert!(is_valid_build("dxha5135ae36ce712"));
    assert!(is_valid_build("dxh1"));
    assert!(!is_valid_build("dxh7A1E5A63C0523EB1"));
    assert!(!is_valid_build("dxh7a1e5a63c0523eb11"));
    assert!(!is_valid_build("dxh"));
    assert!(!is_valid_build("release-2026-08-19"));
    assert!(!is_valid_build(""));
  }

  #[test]
  fn report_wire_format_rejects_unknown_fields() {
    let ok = r#"{"kind":"rust_panic","message":"boom","route":"/dashboard","session_kind":"anonymous","occurred_at_ms":1}"#;
    assert!(serde_json::from_str::<ErrorReport>(ok).is_ok());
    // The H2 shape rule: a text/plain body claiming an identity (or any
    // field outside the automatic envelope) must fail to parse at all.
    let forged = r#"{"kind":"rust_panic","message":"boom","route":"/","session_kind":"signed_in","occurred_at_ms":1,"report_note":"contact me"}"#;
    assert!(serde_json::from_str::<ErrorReport>(forged).is_err());
    let forged_user = r#"{"kind":"rust_panic","message":"boom","route":"/","session_kind":"signed_in","occurred_at_ms":1,"user_id":"abc"}"#;
    assert!(serde_json::from_str::<ErrorReport>(forged_user).is_err());
  }

  const OWN: &[&str] = &["https://pidgeiot.com"];

  #[test]
  fn own_origin_location_is_ours() {
    assert_eq!(
      error_source(
        OWN,
        Some("https://pidgeiot.com/assets/fancier-dxh1.js:12:5"),
        None
      ),
      ErrorSource::Own
    );
  }

  #[test]
  fn analytics_beacon_location_is_foreign_and_keyed_on_its_origin() {
    for col in ["14186", "7806"] {
      assert_eq!(
        error_source(
          OWN,
          Some(&format!(
            "https://static.cloudflareinsights.com/beacon.min.js:1:{col}"
          )),
          None
        ),
        ErrorSource::Foreign("https://static.cloudflareinsights.com".to_string())
      );
    }
  }

  #[test]
  fn rejection_with_no_location_is_judged_by_the_top_frame() {
    let stack = "TypeError: t.entries.at is not a function\n    at t.entries.at (https://cdn.jsdelivr.net/npm/thing@1.2.3/dist/x.min.js:1:14186)\n    at https://pidgeiot.com/assets/fancier-dxh1.js:3:4";
    assert_eq!(
      error_source(OWN, None, Some(stack)),
      ErrorSource::Foreign("https://cdn.jsdelivr.net".to_string())
    );
    let ours_on_top = "Error: boom\n    at https://pidgeiot.com/assets/fancier-dxh1.js:3:4\n    at https://cdn.jsdelivr.net/x.js:1:1";
    assert_eq!(error_source(OWN, None, Some(ours_on_top)), ErrorSource::Own);
  }

  #[test]
  fn gecko_and_webkit_frames_are_read_after_the_at_sign() {
    assert_eq!(
      error_source(
        OWN,
        None,
        Some(
          "t@https://static.cloudflareinsights.com/beacon.min.js:1:14186\nf@https://pidgeiot.com/a.js:1:1"
        )
      ),
      ErrorSource::Foreign("https://static.cloudflareinsights.com".to_string())
    );
    assert_eq!(
      error_source(OWN, None, Some("global code@https://pidgeiot.com/a.js:1:1")),
      ErrorSource::Own
    );
    // WebKit's opaque frames carry no URL at all.
    assert_eq!(
      error_source(OWN, None, Some("wasm-stub@[wasm code]\n@[native code]")),
      ErrorSource::Own
    );
  }

  #[test]
  fn wasm_frames_are_ours_in_every_browser_spelling() {
    assert_eq!(
      error_source(
        OWN,
        None,
        Some("RuntimeError: unreachable\n    at wasm://wasm/0009e8ba:wasm-function[3812]:0x1a2b3c")
      ),
      ErrorSource::Own
    );
    assert_eq!(
      error_source(
        OWN,
        None,
        Some("f@https://pidgeiot.com/wasm/fancier_bg-dxh1.wasm:wasm-function[3812]:0x1a2b3c")
      ),
      ErrorSource::Own
    );
  }

  #[test]
  fn blob_urls_carry_the_minting_origin() {
    assert_eq!(
      error_source(
        OWN,
        Some("blob:https://pidgeiot.com/8f1e2c1a-1111-4222-8333-444455556666:1:2"),
        None
      ),
      ErrorSource::Own
    );
    assert_eq!(
      error_source(
        OWN,
        Some("blob:https://evil.example/8f1e2c1a-1111-4222-8333-444455556666:1:2"),
        None
      ),
      ErrorSource::Foreign("https://evil.example".to_string())
    );
    assert_eq!(
      error_source(OWN, Some("blob:null/abc:1:2"), None),
      ErrorSource::Own
    );
  }

  #[test]
  fn browser_extensions_are_foreign() {
    assert_eq!(
      error_source(
        OWN,
        Some("chrome-extension://abcdefghijklmnopabcdefghijklmnop/content.js:5:9"),
        None
      ),
      ErrorSource::Foreign("chrome-extension://abcdefghijklmnopabcdefghijklmnop".to_string())
    );
    assert_eq!(
      error_source(OWN, None, Some("f@webkit-masked-url://hidden/:1:1")),
      ErrorSource::Foreign("webkit-masked-url://hidden".to_string())
    );
  }

  #[test]
  fn a_report_with_nothing_to_judge_by_is_ours() {
    assert_eq!(error_source(OWN, None, None), ErrorSource::Own);
    assert_eq!(
      error_source(OWN, Some("src/views/pigeon.rs:412:18"), None),
      ErrorSource::Own
    );
    assert_eq!(
      error_source(OWN, Some("main.js:12:3"), None),
      ErrorSource::Own
    );
    assert_eq!(
      error_source(OWN, None, Some("Error: something\n    at <anonymous>:1:1")),
      ErrorSource::Own
    );
  }

  #[test]
  fn a_url_quoted_in_the_message_line_is_not_a_frame() {
    let stack = "Error: could not load https://cdn.example.com/x.js for owner@example.com\n    at https://pidgeiot.com/assets/fancier-dxh1.js:3:4";
    assert_eq!(error_source(OWN, None, Some(stack)), ErrorSource::Own);
  }

  #[test]
  fn origin_comparison_ignores_case_and_a_trailing_slash_but_not_the_port() {
    assert_eq!(
      error_source(
        &["https://PidgeIoT.com/"],
        Some("https://pidgeiot.com/a.js:1:1"),
        None
      ),
      ErrorSource::Own
    );
    assert_eq!(
      error_source(
        &["http://localhost:4455"],
        Some("http://localhost:8787/a.js:1:1"),
        None
      ),
      ErrorSource::Foreign("http://localhost:8787".to_string())
    );
    assert_eq!(
      error_source(
        &["http://localhost:4455"],
        Some("http://localhost:4455/assets/a.js:1:1"),
        None
      ),
      ErrorSource::Own
    );
  }

  #[test]
  fn only_the_counting_kinds_stay_silent() {
    assert!(!ErrorKind::ThirdParty.notifies());
    assert!(!ErrorKind::UnsupportedBrowser.notifies());
    for kind in [
      ErrorKind::RustPanic,
      ErrorKind::WasmBoot,
      ErrorKind::JsException,
      ErrorKind::UnhandledRejection,
      ErrorKind::ApiFailure,
    ] {
      assert!(kind.notifies(), "{}", kind.as_str());
    }
  }

  #[test]
  fn counting_kinds_round_trip_on_the_wire() {
    for (kind, wire) in [
      (ErrorKind::ThirdParty, "third_party"),
      (ErrorKind::UnsupportedBrowser, "unsupported_browser"),
    ] {
      assert_eq!(kind.as_str(), wire);
      assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{wire}\""));
      assert_eq!(
        serde_json::from_str::<ErrorKind>(&format!("\"{wire}\"")).unwrap(),
        kind
      );
    }
  }

  #[test]
  fn truncate_bytes_respects_char_boundaries() {
    let s = "aé£€😀";
    for max in 0..=s.len() {
      let t = truncate_bytes(s, max);
      assert!(t.len() <= max);
      assert!(s.starts_with(t));
    }
  }
}
