//! Data items: the named text values a sign that polls a web data source shows, served from a
//! pigeon's effective config by `GET /device/pigeons/:pigeon_id/values`.
//!
//! A top-level key of the effective config that starts with [`DATA_ITEM_PREFIX`] is one data
//! item, named by the rest of the key. One top-level key per item is what lets a pigeon override
//! one item and keep the flock's others, since layers merge per top-level key.

use serde_json::{Map, Value};

/// The key prefix that marks a data item in an effective config.
pub const DATA_ITEM_PREFIX: &str = "item.";

const XML_DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
const XML_OPEN: &str = "<DataItems>\n";
const XML_CLOSE: &str = "</DataItems>";
const ITEM_OPEN: &str = "  <DataItem name=\"";
const ITEM_NAME_END: &str = "\">";
const ITEM_CLOSE: &str = "</DataItem>\n";

/// The data items an effective config carries, as `(name, value)` pairs sorted by name.
///
/// A string is served as is; a number or a boolean as its JSON text, so a price kept as the
/// number `2.50` comes out `2.5` and belongs in a string. An object, an array, a `null` and the
/// bare prefix are skipped: a sign's data item holds one line of text.
///
/// ```
/// use capsules::data_items::data_items;
///
/// let config = serde_json::json!({"item.lot.capacity": 500, "telemetry_interval": 300});
/// let items = data_items(config.as_object().unwrap());
/// assert_eq!(items, vec![("lot.capacity".to_string(), "500".to_string())]);
/// ```
pub fn data_items(config: &Map<String, Value>) -> Vec<(String, String)> {
  let mut items: Vec<(String, String)> = config
    .iter()
    .filter_map(|(key, value)| {
      let name = key.strip_prefix(DATA_ITEM_PREFIX)?;
      if name.is_empty() {
        return None;
      }
      let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) => return None,
      };
      Some((name.to_string(), text))
    })
    .collect();
  // The map is already ordered, but a sort here keeps the answer stable whatever map
  // ordering a dependency's feature flag picks.
  items.sort_unstable_by(|a, b| a.0.cmp(&b.0));
  items
}

/// `items` as one flat JSON object of name to text, on one line.
pub fn data_items_json(items: &[(String, String)]) -> String {
  let object: Map<String, Value> = items
    .iter()
    .map(|(name, value)| (name.clone(), Value::String(value.clone())))
    .collect();
  Value::Object(object).to_string()
}

/// `items` as the `<DataItems>` document a sign's web data source reads, names and values
/// escaped, and characters XML 1.0 forbids dropped.
///
/// Tab, newline and carriage return go out as character references: a parser turns a literal one
/// in an attribute into a space and a literal carriage return in text into a newline, so a name
/// holding one could never match the sign's data item.
pub fn data_items_xml(items: &[(String, String)]) -> String {
  let mut out = String::with_capacity(xml_len(items));
  out.push_str(XML_DECLARATION);
  out.push_str(XML_OPEN);
  for (name, value) in items {
    out.push_str(ITEM_OPEN);
    push_escaped(&mut out, name);
    out.push_str(ITEM_NAME_END);
    push_escaped(&mut out, value);
    out.push_str(ITEM_CLOSE);
  }
  out.push_str(XML_CLOSE);
  out
}

/// The exact length of [`data_items_xml`]'s answer.
fn xml_len(items: &[(String, String)]) -> usize {
  let per_item = ITEM_OPEN.len() + ITEM_NAME_END.len() + ITEM_CLOSE.len();
  XML_DECLARATION.len()
    + XML_OPEN.len()
    + XML_CLOSE.len()
    + items
      .iter()
      .map(|(name, value)| per_item + escaped_len(name) + escaped_len(value))
      .sum::<usize>()
}

/// XML 1.0's `Char` production: what a document may hold at all.
fn xml_allowed(c: char) -> bool {
  !matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}')
}

/// The reference that writes `c` so a parser reads it back as written, the same in text and in a
/// quoted attribute.
fn xml_entity(c: char) -> Option<&'static str> {
  match c {
    '\t' => Some("&#9;"),
    '\n' => Some("&#10;"),
    '\r' => Some("&#13;"),
    '&' => Some("&amp;"),
    '<' => Some("&lt;"),
    '>' => Some("&gt;"),
    '"' => Some("&quot;"),
    '\'' => Some("&apos;"),
    _ => None,
  }
}

fn escaped_len(text: &str) -> usize {
  text
    .chars()
    .filter(|&c| xml_allowed(c))
    .map(|c| xml_entity(c).map_or(c.len_utf8(), str::len))
    .sum()
}

fn push_escaped(out: &mut String, text: &str) {
  for c in text.chars().filter(|&c| xml_allowed(c)) {
    match xml_entity(c) {
      Some(entity) => out.push_str(entity),
      None => out.push(c),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::flock_config::effective_config;
  use serde_json::json;

  fn map(value: Value) -> Map<String, Value> {
    match value {
      Value::Object(map) => map,
      _ => panic!("test config is not an object"),
    }
  }

  fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
      .iter()
      .map(|(name, value)| (name.to_string(), value.to_string()))
      .collect()
  }

  /// The poll route design's worked example: a flock layer, one sign's layer, the merge.
  fn worked_example() -> Map<String, Value> {
    let flock = map(json!({
      "item.lot.status": "OPEN",
      "item.notice": "Pay at kiosk & keep ticket",
      "item.rate.hourly": "2.50",
      "telemetry_interval": 300
    }));
    let pigeon = map(json!({
      "item.lot.capacity": 500,
      "item.lot.name": "Lot B <North>",
      "item.lot.status": "FULL"
    }));
    effective_config(&flock, &pigeon)
  }

  #[test]
  fn the_worked_example_maps_to_its_items() {
    assert_eq!(
      data_items(&worked_example()),
      pairs(&[
        ("lot.capacity", "500"),
        ("lot.name", "Lot B <North>"),
        ("lot.status", "FULL"),
        ("notice", "Pay at kiosk & keep ticket"),
        ("rate.hourly", "2.50"),
      ])
    );
  }

  #[test]
  fn the_worked_example_renders_as_json_byte_for_byte() {
    assert_eq!(
      data_items_json(&data_items(&worked_example())),
      concat!(
        "{\"lot.capacity\":\"500\",\"lot.name\":\"Lot B <North>\",\"lot.status\":\"FULL\",",
        "\"notice\":\"Pay at kiosk & keep ticket\",\"rate.hourly\":\"2.50\"}"
      )
    );
  }

  #[test]
  fn the_worked_example_renders_as_xml_byte_for_byte() {
    assert_eq!(
      data_items_xml(&data_items(&worked_example())),
      concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<DataItems>\n",
        "  <DataItem name=\"lot.capacity\">500</DataItem>\n",
        "  <DataItem name=\"lot.name\">Lot B &lt;North&gt;</DataItem>\n",
        "  <DataItem name=\"lot.status\">FULL</DataItem>\n",
        "  <DataItem name=\"notice\">Pay at kiosk &amp; keep ticket</DataItem>\n",
        "  <DataItem name=\"rate.hourly\">2.50</DataItem>\n",
        "</DataItems>"
      )
    );
  }

  #[test]
  fn xml_escapes_all_five_markup_characters_in_names_and_values() {
    let xml = data_items_xml(&pairs(&[("a&<>\"'b", "c&<>\"'d")]));
    assert!(xml.contains(
      "<DataItem name=\"a&amp;&lt;&gt;&quot;&apos;b\">c&amp;&lt;&gt;&quot;&apos;d</DataItem>"
    ));
  }

  #[test]
  fn xml_drops_forbidden_characters() {
    let xml = data_items_xml(&pairs(&[("n\u{1}m", "a\u{0}b\u{1f}c\u{fffe}\u{ffff}d")]));
    assert!(xml.contains("<DataItem name=\"nm\">abcd</DataItem>"));
  }

  #[test]
  fn xml_writes_tab_newline_and_return_as_references_in_names_and_values() {
    let xml = data_items_xml(&pairs(&[("a\tb\nc\rd", "e\tf\r\ng")]));
    assert!(xml.contains("<DataItem name=\"a&#9;b&#10;c&#13;d\">e&#9;f&#13;&#10;g</DataItem>"));
  }

  #[test]
  fn xml_keeps_utf8_past_ascii() {
    let xml = data_items_xml(&pairs(&[("temp", "72°F café 🚗")]));
    assert!(xml.contains("<DataItem name=\"temp\">72°F café 🚗</DataItem>"));
  }

  #[test]
  fn xml_is_sized_exactly_from_its_parts() {
    let items = pairs(&[("a&b\t", "x<y é \u{1}\r\n"), ("plain", "text")]);
    assert_eq!(xml_len(&items), data_items_xml(&items).len());
  }

  #[test]
  fn numbers_and_booleans_are_served_as_their_json_text() {
    let config = map(json!({
      "item.count": 500,
      "item.price": 2.50,
      "item.negative": -3,
      "item.open": true,
      "item.closed": false
    }));
    assert_eq!(
      data_items(&config),
      pairs(&[
        ("closed", "false"),
        ("count", "500"),
        ("negative", "-3"),
        ("open", "true"),
        ("price", "2.5"),
      ])
    );
  }

  #[test]
  fn objects_arrays_nulls_the_bare_prefix_and_other_keys_are_skipped() {
    let config = map(json!({
      "item.": "bare",
      "item.object": {"a": "b"},
      "item.array": ["a"],
      "item.null": null,
      "telemetry_interval": 60,
      "firmware": {"version": "1.0.0"},
      "items.not": "close but no",
      "Item.case": "prefix is case-sensitive",
      "item.kept": "yes"
    }));
    assert_eq!(data_items(&config), pairs(&[("kept", "yes")]));
  }

  #[test]
  fn no_items_render_as_an_empty_object_and_an_empty_document() {
    let items = data_items(&map(json!({"telemetry_interval": 60})));
    assert!(items.is_empty());
    assert_eq!(data_items_json(&items), "{}");
    assert_eq!(
      data_items_xml(&items),
      "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<DataItems>\n</DataItems>"
    );
  }

  #[test]
  fn json_escapes_what_json_requires() {
    let json = data_items_json(&pairs(&[("q\"n", "line\nbreak \\ \u{1}")]));
    assert_eq!(json, "{\"q\\\"n\":\"line\\nbreak \\\\ \\u0001\"}");
  }
}
