//! Gateway helpers for `GET /device/pigeons/:pigeon_id/values`, a sign's poll for its data items:
//! reading the poll, keying its limiter, and marking its answer for the connected stamp.

use worker::{Response, Url};

/// The body a values poll is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValuesFormat {
  /// One flat JSON object of name to text.
  Json,
  /// The `<DataItems>` XML document.
  Xml,
}

impl ValuesFormat {
  /// The pigeon object's path that answers in this format.
  pub fn do_path(self) -> &'static str {
    match self {
      ValuesFormat::Json => "/device/values",
      ValuesFormat::Xml => "/device/values/xml",
    }
  }
}

/// The 400 body for a `format` other than `json` or `xml`.
pub const VALUES_BAD_FORMAT: &str = "Bad Request: format must be json or xml";

/// The 400 body for a poll over plain HTTP.
pub const VALUES_NOT_HTTPS: &str = "Bad Request: poll this route over HTTPS";

/// Reads a values poll's URL into the format it asks for, or the 400 body that refuses it.
///
/// The first `format` parameter decides and an absent one is JSON; every other parameter is
/// ignored, so a sign can append its own (`&mac=#MACADDR#`). A plain `http` URL is refused outside
/// `local_dev`: nothing on file shows the zone redirecting it and libcurl follows no redirect by
/// default, so a sign set up that way would otherwise work with its token in clear.
pub fn values_request(url: &Url, local_dev: bool) -> Result<ValuesFormat, &'static str> {
  let format = match url.query_pairs().find(|(key, _)| key == "format") {
    None => ValuesFormat::Json,
    Some((_, value)) => match value.as_ref() {
      "json" => ValuesFormat::Json,
      "xml" => ValuesFormat::Xml,
      _ => return Err(VALUES_BAD_FORMAT),
    },
  };
  if url.scheme() == "http" && !local_dev {
    return Err(VALUES_NOT_HTTPS);
  }
  Ok(format)
}

/// The values limiter's key: the pigeon, and the address polling it when there is one.
///
/// The limiter counts before the pigeon object checks the token, so keyed on the pigeon alone
/// anyone who knows its id could spend a sign's few polls a minute with junk tokens and freeze its
/// values, while staying under the failed-auth budget. With the address, an outsider spends only
/// their own bucket and a misconfigured sign, one address, stays bounded. A request with no
/// address to count (the CoAP terminator, which already holds a per-pigeon key) uses the pigeon.
pub fn values_limit_key(pigeon_id: &str, address: Option<&str>) -> String {
  let Some(address) = address else {
    return pigeon_id.to_string();
  };
  let mut key = String::with_capacity(pigeon_id.len() + 1 + address.len());
  key.push_str(pigeon_id);
  key.push(' ');
  key.push_str(address);
  key
}

/// Internal response header marking a values poll that is due a connected stamp, valued as the
/// `SEEN_HEADER` it rode in on. `main` strips it and makes the stamp after the response.
pub const CONNECTED_HEADER: &str = "X-Pigeon-Connected";

/// The `CONNECTED_HEADER` value for an answered poll, if it is due one.
///
/// Due only with the seen stamp, which the pigeon's object sets at most once a minute, so a sign
/// costs at most one Postgres statement a minute whatever its cadence; the statement itself moves
/// the stamp at most every six hours. A pigeon whose other contact took that minute's seen stamp is
/// already stamping through its own billable reports, or is marked by a later poll.
pub fn values_poll_connected(status: u16, seen: Option<&str>) -> Option<&str> {
  let seen = seen?;
  ((200..300).contains(&status) && crate::helpers::parse_seen_header(seen).is_some())
    .then_some(seen)
}

/// Bills a values poll: never a billable message, but a sign that polls is a connected device for
/// the period, as one that reports is.
///
/// Marks the answer rather than stamping from here: a route closure has no `wait_until`, and an
/// awaited `UPDATE` would hold the sign's answer. The mark goes on a copy of the headers because
/// the object's answer arrives with immutable ones, which refuse a `set`.
pub fn bill_values_poll(response: Response) -> Response {
  let seen = crate::helpers::seen_of(&response);
  let Some(value) = values_poll_connected(response.status_code(), seen.as_deref()) else {
    return response;
  };
  let headers = response.headers().clone();
  if let Err(e) = headers.set(CONNECTED_HEADER, value) {
    worker::console_error!("Failed to mark a values poll connected: {e}");
    return response;
  }
  response.with_headers(headers)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn read(url: &str, local_dev: bool) -> Result<ValuesFormat, &'static str> {
    values_request(&Url::parse(url).expect("test URL parses"), local_dev)
  }

  const BASE: &str = "https://api.pidgeiot.com/device/pigeons/abc/values";

  fn with_query(query: &str) -> String {
    let mut url = String::with_capacity(BASE.len() + query.len());
    url.push_str(BASE);
    url.push_str(query);
    url
  }

  #[test]
  fn an_absent_format_is_json() {
    assert_eq!(read(BASE, false), Ok(ValuesFormat::Json));
    assert_eq!(read(&with_query("?"), false), Ok(ValuesFormat::Json));
  }

  #[test]
  fn json_and_xml_are_read() {
    assert_eq!(
      read(&with_query("?format=json"), false),
      Ok(ValuesFormat::Json)
    );
    assert_eq!(
      read(&with_query("?format=xml"), false),
      Ok(ValuesFormat::Xml)
    );
  }

  #[test]
  fn any_other_format_is_refused() {
    for query in ["?format=", "?format=XML", "?format=csv", "?format=xml%20"] {
      assert_eq!(
        read(&with_query(query), false),
        Err(VALUES_BAD_FORMAT),
        "{query}"
      );
    }
  }

  #[test]
  fn other_parameters_are_ignored_and_the_first_format_decides() {
    assert_eq!(
      read(&with_query("?mac=00-11-22&format=xml&x=1"), false),
      Ok(ValuesFormat::Xml)
    );
    assert_eq!(
      read(&with_query("?mac=00-11-22"), false),
      Ok(ValuesFormat::Json)
    );
    assert_eq!(
      read(&with_query("?format=xml&format=json"), false),
      Ok(ValuesFormat::Xml)
    );
  }

  #[test]
  fn plain_http_is_refused_except_under_local_dev() {
    let http = "http://api.pidgeiot.com/device/pigeons/abc/values?format=xml";
    assert_eq!(read(http, false), Err(VALUES_NOT_HTTPS));
    let dev = "http://127.0.0.1:8787/device/pigeons/abc/values?format=xml";
    assert_eq!(read(dev, true), Ok(ValuesFormat::Xml));
  }

  #[test]
  fn the_limit_key_separates_addresses_and_pigeons() {
    let sign = values_limit_key("abc", Some("203.0.113.7"));
    assert_eq!(sign, "abc 203.0.113.7");
    assert_ne!(sign, values_limit_key("abc", Some("198.51.100.9")));
    assert_ne!(sign, values_limit_key("abd", Some("203.0.113.7")));
    assert_ne!(sign, values_limit_key("abc", None));
  }

  #[test]
  fn with_no_address_the_limit_key_is_the_pigeon() {
    assert_eq!(values_limit_key("abc", None), "abc");
  }

  const SEEN: &str = "1760000000 abc";

  #[test]
  fn an_answered_poll_carrying_a_seen_stamp_marks_the_sign_connected() {
    assert_eq!(values_poll_connected(200, Some(SEEN)), Some(SEEN));
  }

  #[test]
  fn a_poll_without_a_seen_stamp_waits_for_the_next_minute() {
    assert_eq!(values_poll_connected(200, None), None);
  }

  #[test]
  fn a_refused_or_failed_poll_marks_nothing() {
    for status in [400, 401, 429, 500, 503] {
      assert_eq!(values_poll_connected(status, Some(SEEN)), None, "{status}");
    }
  }

  #[test]
  fn a_malformed_seen_stamp_marks_nothing() {
    for seen in ["", "abc", "0 abc", "1760000000 a b"] {
      assert_eq!(values_poll_connected(200, Some(seen)), None, "{seen:?}");
    }
  }

  #[test]
  fn the_connected_mark_is_neither_a_seen_stamp_nor_a_message_count() {
    assert_ne!(CONNECTED_HEADER, crate::helpers::SEEN_HEADER);
    assert_ne!(CONNECTED_HEADER, crate::helpers::nidd::HEADER_BILLED);
  }

  #[test]
  fn each_format_has_its_own_object_path() {
    assert_eq!(ValuesFormat::Json.do_path(), "/device/values");
    assert_eq!(ValuesFormat::Xml.do_path(), "/device/values/xml");
  }
}
