//! The request side of `GET /device/pigeons/:pigeon_id/values`, a sign's poll for its data items.

/// The body a values poll is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValuesFormat {
  /// One flat JSON object of name to text.
  Json,
  /// The `<DataItems>` XML document.
  Xml,
}
