//! dovecote's rule routes (docs/api.md, "Rules"). Every function hands back the server's own text
//! on a refusal: a 403 at the rule limit, a 409 for an untested deploy, a 422 for rules that do
//! not load, a 429 when deploys are busy and a 503 where rules do not run are each something the
//! person has to read. Nothing here caches; each page builds its
//! state from the answers, a mutation's included (Hyperdrive may serve a re-read from before it).

use crate::api::fetch_json_any_status;
use crate::api::orgs::{error_text, parse, to_body};
use capsules::{
  Rule, RuleCreateRequest, RuleDeployRequest, RuleDetail, RuleRun, RuleUpdateRequest, RuleValue,
  RuleVersion, RuleVersionCreateRequest,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

/// `/flocks/<flock_id>/rules`, then `/<rule_id>` and `tail` when given.
fn rule_path(flock_id: Uuid, rule_id: Option<Uuid>, tail: &str) -> String {
  let mut buf = Uuid::encode_buffer();
  let mut path = String::with_capacity(8 + 36 + 7 + 37 + tail.len());
  path.push_str("/flocks/");
  path.push_str(flock_id.hyphenated().encode_lower(&mut buf));
  path.push_str("/rules");
  if let Some(rule_id) = rule_id {
    path.push('/');
    path.push_str(rule_id.hyphenated().encode_lower(&mut buf));
  }
  path.push_str(tail);
  path
}

/// `/versions/<version>` followed by `action`.
fn version_tail(version: i32, action: &str) -> String {
  let version = version.to_string();
  let mut tail = String::with_capacity(10 + version.len() + action.len());
  tail.push_str("/versions/");
  tail.push_str(&version);
  tail.push_str(action);
  tail
}

/// One request: a 2xx body parsed as `T`, anything else the server's text.
async fn call<T: DeserializeOwned>(
  method: &str,
  path: &str,
  body: Option<&impl Serialize>,
) -> Result<T, String> {
  let body = match body {
    Some(body) => {
      let Some(json) = to_body(body) else {
        return Err("Failed to encode request".to_string());
      };
      Some(json)
    }
    None => None,
  };
  let Some(response) = fetch_json_any_status(method, path, body.as_ref()).await else {
    return Err("Network error".to_string());
  };
  if !response.ok() {
    return Err(error_text(&response).await);
  }
  let Some(parsed) = parse::<T>(response).await else {
    return Err("Failed to parse response".to_string());
  };
  Ok(parsed)
}

/// `GET /flocks/:flock_id/rules`, newest first.
pub async fn list(flock_id: Uuid) -> Result<Vec<Rule>, String> {
  call("GET", &rule_path(flock_id, None, ""), None::<&()>).await
}

/// `POST /flocks/:flock_id/rules`: a disabled rule with no version.
pub async fn create(flock_id: Uuid, req: &RuleCreateRequest) -> Result<Rule, String> {
  call("POST", &rule_path(flock_id, None, ""), Some(req)).await
}

/// `GET /flocks/:flock_id/rules/:rule_id`: the rule and its versions, without their source.
pub async fn detail(flock_id: Uuid, rule_id: Uuid) -> Result<RuleDetail, String> {
  call("GET", &rule_path(flock_id, Some(rule_id), ""), None::<&()>).await
}

/// `PUT /flocks/:flock_id/rules/:rule_id`: rename, re-address, enable or disable.
pub async fn update(
  flock_id: Uuid,
  rule_id: Uuid,
  req: &RuleUpdateRequest,
) -> Result<Rule, String> {
  call("PUT", &rule_path(flock_id, Some(rule_id), ""), Some(req)).await
}

/// `DELETE /flocks/:flock_id/rules/:rule_id`. The answer has no body.
pub async fn delete(flock_id: Uuid, rule_id: Uuid) -> Result<(), String> {
  let path = rule_path(flock_id, Some(rule_id), "");
  let Some(response) = fetch_json_any_status("DELETE", &path, None).await else {
    return Err("Network error".to_string());
  };
  if !response.ok() {
    return Err(error_text(&response).await);
  }
  Ok(())
}

/// `POST /flocks/:flock_id/rules/:rule_id/versions`: the saved version, source included.
pub async fn save_version(
  flock_id: Uuid,
  rule_id: Uuid,
  req: &RuleVersionCreateRequest,
) -> Result<RuleVersion, String> {
  call(
    "POST",
    &rule_path(flock_id, Some(rule_id), "/versions"),
    Some(req),
  )
  .await
}

/// `GET /flocks/:flock_id/rules/:rule_id/versions/:version`, with its source.
pub async fn version(flock_id: Uuid, rule_id: Uuid, version: i32) -> Result<RuleVersion, String> {
  let path = rule_path(flock_id, Some(rule_id), &version_tail(version, ""));
  call("GET", &path, None::<&()>).await
}

/// `POST .../versions/:version/test`: the version with its new test record and summary.
pub async fn test(flock_id: Uuid, rule_id: Uuid, version: i32) -> Result<RuleVersion, String> {
  let path = rule_path(flock_id, Some(rule_id), &version_tail(version, "/test"));
  call("POST", &path, None::<&()>).await
}

/// `POST .../versions/:version/deploy`: the rule as the deploy left it. A refusal's text is the
/// server's, Cloudflare's load error for a rule that does not load included.
pub async fn deploy(
  flock_id: Uuid,
  rule_id: Uuid,
  version: i32,
  req: &RuleDeployRequest,
) -> Result<Rule, String> {
  let path = rule_path(flock_id, Some(rule_id), &version_tail(version, "/deploy"));
  call("POST", &path, Some(req)).await
}

/// `GET /flocks/:flock_id/rules/:rule_id/runs`, newest first.
pub async fn runs(flock_id: Uuid, rule_id: Uuid) -> Result<Vec<RuleRun>, String> {
  call(
    "GET",
    &rule_path(flock_id, Some(rule_id), "/runs"),
    None::<&()>,
  )
  .await
}

/// `POST /flocks/:flock_id/rules/:rule_id/watch`: record every run for the next hour.
pub async fn watch(flock_id: Uuid, rule_id: Uuid) -> Result<Rule, String> {
  call(
    "POST",
    &rule_path(flock_id, Some(rule_id), "/watch"),
    None::<&()>,
  )
  .await
}

/// `GET /flocks/:flock_id/rules/:rule_id/values`: the latest value per pigeon and key.
pub async fn values(flock_id: Uuid, rule_id: Uuid) -> Result<Vec<RuleValue>, String> {
  call(
    "GET",
    &rule_path(flock_id, Some(rule_id), "/values"),
    None::<&()>,
  )
  .await
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn paths_match_the_api_reference() {
    let flock = Uuid::parse_str("8b0e4c9e-05ff-4a3b-9d5e-111111111111").unwrap();
    let rule = Uuid::parse_str("1c2d3e4f-0000-4000-8000-222222222222").unwrap();
    assert_eq!(
      rule_path(flock, None, ""),
      "/flocks/8b0e4c9e-05ff-4a3b-9d5e-111111111111/rules"
    );
    assert_eq!(
      rule_path(flock, Some(rule), &version_tail(12, "/deploy")),
      "/flocks/8b0e4c9e-05ff-4a3b-9d5e-111111111111/rules/1c2d3e4f-0000-4000-8000-222222222222/versions/12/deploy"
    );
  }
}
