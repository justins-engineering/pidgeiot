//! The request side of `GET /device/pigeons/:pigeon_id/values`, a sign's poll for its data items.

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

/// The one place a values poll would count toward billing, and today it counts for nothing: a
/// poll is neither a billable message nor connected activity, as a shadow poll is not. Counting
/// polls as connected activity means stamping `last_billable_activity` here for an answer that
/// carries the seen header, which the pigeon object sets at most once a minute.
pub fn bill_values_poll(_response: &Response) {}

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

  #[test]
  fn each_format_has_its_own_object_path() {
    assert_eq!(ValuesFormat::Json.do_path(), "/device/values");
    assert_eq!(ValuesFormat::Xml.do_path(), "/device/values/xml");
  }
}
