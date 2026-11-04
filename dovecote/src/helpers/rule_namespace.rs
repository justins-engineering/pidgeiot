//! The rules namespace from dovecote's side: the Cloudflare API calls that upload a flock's rules
//! script or a rule's dry-run script, delete one, re-tag one, and list them for the daily sweep.
//!
//! Every URL is built from [`CF_API_BASE`] and the environment's own vars, and every script name
//! passes [`is_script_name`], so no request text reaches a URL. The upload's metadata is
//! dovecote's own ([`upload_metadata`]): a rule contributes one module's source and nothing else,
//! so it names no binding, module path, flag or limit. The token is read by name per call and
//! never logged, and of a Cloudflare answer only the status and error codes are, since a load
//! error quotes the rule's own text.

use std::pin::pin;
use std::time::Duration;

use capsules::Rule;
use futures::future::{Either, select};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_postgres::Client;
use uuid::Uuid;
use worker::{
  AbortController, Delay, Env, Fetch, Method, Request, RequestInit, console_error, console_log,
};

use crate::helpers::cf_api::{CF_API_WINDOW_SECS, cf_api_allowed, cloudflare_wait};
use crate::helpers::firmware::FlockAccess;
use crate::helpers::get_db_client;
use crate::helpers::rule_admin::{
  BundleSource, DeployFinish, DeployRefusal, DeployStart, RebuildStart, abandon_deploy,
  finish_deploy, finish_rebuild, flock_owner, retag_bundle, start_deploy, start_rebuild,
  sweep_bundles,
};
use crate::helpers::rule_store::release_deploy_lease;
use crate::helpers::rules::{
  BundleRule, RulesEnv, ScriptKind, bundle_modules, is_lower_hex, is_uuid, multipart_body,
  multipart_content_type, script_name, script_tags, upload_metadata,
};

/// Every Cloudflare API URL starts here.
pub const CF_API_BASE: &str = "https://api.cloudflare.com/client/v4/accounts/";

/// The Worker secret holding the API token, read by this name and never logged.
pub const RULES_TOKEN: &str = "CF_RULES_API_TOKEN";

/// The production namespace's name, and the staging one's.
const NAMESPACES: [(&str, RulesEnv); 2] = [
  ("dovecote-rules", RulesEnv::Production),
  ("dovecote-rules-staging", RulesEnv::Staging),
];

const NAMESPACE_PATH: &str = "/workers/dispatch/namespaces/";
const SCRIPTS_PATH: &str = "/scripts";
const LIST_QUERY: &str = "?tags=pidgeiot-rule:yes";

/// How long one call may take before it is aborted. An upload is synchronous, so this bounds the
/// whole deploy, and it sits inside the deploy lease.
pub const CF_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Cloudflare's code for a script that failed validation at upload: a missing module, a top
/// level that throws or overruns its startup CPU.
const SCRIPT_INVALID_CODE: u64 = 10021;

/// Cloudflare's code for a script that does not exist.
const SCRIPT_MISSING_CODE: u64 = 10007;

/// Most bytes of a load error a person is shown.
pub const MAX_LOAD_ERROR_BYTES: usize = 1024;

/// Age past which a dry run's script is an orphan: a dry run deletes its own within seconds.
pub const TEST_SCRIPT_MAX_AGE_SECS: i64 = 3_600;

/// Pages of a listing the sweep reads at most, so a cursor that never ends cannot hold it.
pub const MAX_LIST_PAGES: usize = 50;

/// The cron's period, of which the sweep takes the first firing each UTC day.
const CRON_PERIOD_MS: f64 = 300_000.0;
const DAY_MS: f64 = 86_400_000.0;

/// One environment's namespace, from its `RULES_ACCOUNT_ID` and `RULES_NAMESPACE` vars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
  scripts_url: String,
  env: RulesEnv,
}

impl Namespace {
  /// `None` unless `account` is 32 lowercase hex and `namespace` one of the two names dovecote
  /// knows, which also says the environment the scripts are tagged with.
  pub fn from_vars(account: Option<&str>, namespace: Option<&str>) -> Option<Self> {
    let account = account.filter(|a| a.len() == 32 && is_lower_hex(a))?;
    let (name, env) = NAMESPACES
      .iter()
      .find(|(name, _)| Some(*name) == namespace)?;
    let mut scripts_url = String::with_capacity(
      CF_API_BASE.len() + account.len() + NAMESPACE_PATH.len() + name.len() + SCRIPTS_PATH.len(),
    );
    scripts_url.push_str(CF_API_BASE);
    scripts_url.push_str(account);
    scripts_url.push_str(NAMESPACE_PATH);
    scripts_url.push_str(name);
    scripts_url.push_str(SCRIPTS_PATH);
    Some(Self {
      scripts_url,
      env: *env,
    })
  }

  /// This Worker's namespace, `None` where its vars are unset or wrong.
  pub fn of(env: &Env) -> Option<Self> {
    let var = |name| env.var(name).ok().map(|v| v.to_string());
    Self::from_vars(
      var("RULES_ACCOUNT_ID").as_deref(),
      var("RULES_NAMESPACE").as_deref(),
    )
  }

  /// The environment its scripts are tagged with.
  pub fn env(&self) -> RulesEnv {
    self.env
  }

  /// `.../scripts/<name><tail>`, `None` unless `name` passes [`is_script_name`].
  fn script_url(&self, name: &str, tail: &str) -> Option<String> {
    if !is_script_name(name) {
      return None;
    }
    let mut url = String::with_capacity(self.scripts_url.len() + 1 + name.len() + tail.len());
    url.push_str(&self.scripts_url);
    url.push('/');
    url.push_str(name);
    url.push_str(tail);
    Some(url)
  }
}

/// Whether `name` is `flock-` or `test-` and 32 lowercase hex, the only scripts dovecote names.
pub fn is_script_name(name: &str) -> bool {
  ["flock-", "test-"].iter().any(|prefix| {
    name
      .strip_prefix(prefix)
      .is_some_and(|hex| hex.len() == 32 && is_lower_hex(hex))
  })
}

/// An API call's method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfMethod {
  Get,
  Put,
  Delete,
}

/// One Cloudflare API call, without its token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfRequest {
  pub method: CfMethod,
  pub url: String,
  pub content_type: Option<String>,
  pub body: Option<String>,
}

/// The upload of `modules` as script `name` under `metadata`; `None` for a name that fails
/// [`is_script_name`] or a body [`multipart_body`] refuses.
pub fn upload_request(
  ns: &Namespace,
  name: &str,
  boundary: &str,
  metadata: &str,
  modules: &[(String, String)],
) -> Option<CfRequest> {
  Some(CfRequest {
    method: CfMethod::Put,
    url: ns.script_url(name, "")?,
    content_type: Some(multipart_content_type(boundary)),
    body: Some(multipart_body(boundary, metadata, modules)?),
  })
}

/// The delete of script `name`. Without `force`: a rules script has no binding to remove with it.
pub fn delete_request(ns: &Namespace, name: &str) -> Option<CfRequest> {
  Some(CfRequest {
    method: CfMethod::Delete,
    url: ns.script_url(name, "")?,
    content_type: None,
    body: None,
  })
}

/// The replacement of script `name`'s tags with `tags`.
pub fn tags_request(ns: &Namespace, name: &str, tags: &[String]) -> Option<CfRequest> {
  Some(CfRequest {
    method: CfMethod::Put,
    url: ns.script_url(name, "/tags")?,
    content_type: Some("application/json".to_string()),
    body: Some(serde_json::to_string(tags).ok()?),
  })
}

/// One page of the scripts tagged [`crate::helpers::rules::RULE_SCRIPT_TAG`], after `cursor`
/// when given; `None` for a cursor that could change the query.
pub fn list_request(ns: &Namespace, cursor: Option<&str>) -> Option<CfRequest> {
  let cursor_ok = |c: &str| {
    c.bytes()
      .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'='))
  };
  if cursor.is_some_and(|c| !cursor_ok(c)) {
    return None;
  }
  let cursor = cursor.unwrap_or("");
  let mut url = String::with_capacity(ns.scripts_url.len() + LIST_QUERY.len() + 8 + cursor.len());
  url.push_str(&ns.scripts_url);
  url.push_str(LIST_QUERY);
  if !cursor.is_empty() {
    url.push_str("&cursor=");
    url.push_str(cursor);
  }
  Some(CfRequest {
    method: CfMethod::Get,
    url,
    content_type: None,
    body: None,
  })
}

/// 32 random lowercase hex for a multipart boundary.
pub fn boundary() -> Option<String> {
  let mut bytes = [0u8; 16];
  getrandom::getrandom(&mut bytes).ok()?;
  const HEX: &[u8; 16] = b"0123456789abcdef";
  let mut out = String::with_capacity(32);
  for b in bytes {
    out.push(char::from(HEX[usize::from(b >> 4)]));
    out.push(char::from(HEX[usize::from(b & 0x0f)]));
  }
  Some(out)
}

/// What a Cloudflare API call came to.
#[derive(Debug, Clone, PartialEq)]
pub enum CfAnswer {
  /// Done; the whole answer, `result` and `result_info` included.
  Done(Value),
  /// Cloudflare's 429; the wait to tell, per [`cloudflare_wait`].
  Busy(u32),
  /// Refused at upload because the script did not load; Cloudflare's text, made readable.
  Invalid(String),
  /// No such script.
  Missing,
  /// Any other refusal: nothing changed.
  Refused { status: u16, codes: Vec<u64> },
  /// A 5xx, an unreadable success, no answer or no answer in time: the call may have landed.
  Unsure,
}

impl CfAnswer {
  /// Whether the call may have changed the script although it did not say so.
  pub fn may_have_landed(&self) -> bool {
    *self == CfAnswer::Unsure
  }
}

/// Reads one answer. `retry_after` is the header's raw value.
pub fn read_answer(status: u16, retry_after: Option<&str>, body: &str) -> CfAnswer {
  if let Some(wait) = cloudflare_wait(status, retry_after) {
    return CfAnswer::Busy(wait);
  }
  if status >= 500 {
    return CfAnswer::Unsure;
  }
  let parsed = serde_json::from_str::<Value>(body).ok();
  let errors: Vec<(u64, &str)> = parsed
    .as_ref()
    .and_then(|v| v.get("errors"))
    .and_then(Value::as_array)
    .map(|errors| {
      errors
        .iter()
        .map(|e| {
          let code = e.get("code").and_then(Value::as_u64).unwrap_or(0);
          (code, e.get("message").and_then(Value::as_str).unwrap_or(""))
        })
        .collect()
    })
    .unwrap_or_default();
  if let Some((_, message)) = errors.iter().find(|(code, _)| *code == SCRIPT_INVALID_CODE) {
    return CfAnswer::Invalid(readable_load_error(message));
  }
  if status == 404 || errors.iter().any(|(code, _)| *code == SCRIPT_MISSING_CODE) {
    return CfAnswer::Missing;
  }
  match parsed {
    Some(v) if (200..300).contains(&status) && v.get("success") == Some(&Value::Bool(true)) => {
      CfAnswer::Done(v)
    }
    Some(_) => CfAnswer::Refused {
      status,
      codes: errors.iter().map(|(code, _)| *code).collect(),
    },
    None if (200..300).contains(&status) => CfAnswer::Unsure,
    None => CfAnswer::Refused {
      status,
      codes: Vec::new(),
    },
  }
}

/// A load error as a person reads it: trailing whitespace dropped, each control character but a
/// newline made a space, cut to [`MAX_LOAD_ERROR_BYTES`] on a character boundary.
pub fn readable_load_error(message: &str) -> String {
  let message = message.trim_end();
  let mut out = String::with_capacity(message.len().min(MAX_LOAD_ERROR_BYTES));
  for c in message.chars() {
    let c = if c.is_control() && c != '\n' { ' ' } else { c };
    if out.len() + c.len_utf8() > MAX_LOAD_ERROR_BYTES {
      break;
    }
    out.push(c);
  }
  out
}

const LOAD_ERROR_HEAD: &str = "Unprocessable Content: the flock's rules did not load: ";
const NAMESPACE_REFUSED: &str =
  "Service Unavailable: the rules namespace refused the change; nothing was changed";
const NAMESPACE_UNSURE: &str =
  "Service Unavailable: the rules namespace did not confirm the change; retry";

/// The status and text for a call that did not come to [`CfAnswer::Done`]. Never 401, which signs
/// the dashboard out: Cloudflare refusing our own token is ours, not the person's.
pub fn failure_response(answer: &CfAnswer) -> (u16, String) {
  match answer {
    CfAnswer::Busy(_) => {
      let (status, text) = crate::helpers::cf_api::CF_API_BUSY;
      (status, text.to_string())
    }
    CfAnswer::Invalid(message) => {
      let mut text = String::with_capacity(LOAD_ERROR_HEAD.len() + message.len());
      text.push_str(LOAD_ERROR_HEAD);
      text.push_str(message);
      (422, text)
    }
    CfAnswer::Unsure => (503, NAMESPACE_UNSURE.to_string()),
    CfAnswer::Done(_) | CfAnswer::Missing | CfAnswer::Refused { .. } => {
      (503, NAMESPACE_REFUSED.to_string())
    }
  }
}

/// The `acct-` tag of an owning organization or user, as [`script_tags`] writes it and
/// `rule_bundles` records it.
pub fn account_tag(account: Uuid) -> String {
  let mut buf = Uuid::encode_buffer();
  let id = account.hyphenated().encode_lower(&mut buf);
  let mut tag = String::with_capacity(5 + id.len());
  tag.push_str("acct-");
  tag.push_str(id);
  tag
}

/// One script of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedScript {
  pub name: String,
  pub tags: Vec<String>,
  /// Unix seconds of its last change, when the listing gave a readable time.
  pub modified_at: Option<i64>,
}

/// A listing page's scripts, and the cursor of the next page when there is one. Reads an entry's
/// fields from its `script` object or from the entry itself; an entry without a name is skipped.
pub fn listing(page: &Value) -> (Vec<ListedScript>, Option<String>) {
  let field = |entry: &Value, key: &str| -> Option<Value> {
    entry
      .get("script")
      .and_then(|s| s.get(key))
      .filter(|v| !v.is_null())
      .or_else(|| entry.get(key).filter(|v| !v.is_null()))
      .cloned()
  };
  let scripts = page
    .get("result")
    .and_then(Value::as_array)
    .map(|entries| {
      entries
        .iter()
        .filter_map(|entry| {
          let name = field(entry, "id")?.as_str()?.to_string();
          let tags = field(entry, "tags")
            .and_then(|t| t.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|t| t.as_str().map(str::to_string))
            .collect();
          let modified_at = field(entry, "modified_on")
            .and_then(|t| OffsetDateTime::parse(t.as_str()?, &Rfc3339).ok())
            .map(OffsetDateTime::unix_timestamp);
          Some(ListedScript {
            name,
            tags,
            modified_at,
          })
        })
        .collect()
    })
    .unwrap_or_default();
  let cursor = page
    .get("result_info")
    .and_then(|i| i.get("cursor"))
    .and_then(Value::as_str)
    .filter(|c| !c.is_empty())
    .map(str::to_string);
  (scripts, cursor)
}

/// A flock's bundle row as the sweep reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepBundle {
  pub flock_id: Uuid,
  /// A bundle was uploaded at some point (`uploaded_at` set).
  pub uploaded: bool,
  /// The `acct-` tag it was uploaded under.
  pub account_tag: String,
  /// The flock's owning organization or user now.
  pub owner: Uuid,
  /// A deploy holds the lease: the sweep leaves the flock alone.
  pub leased: bool,
  /// A rule names a pending version that no live deploy will finish.
  pub stale_pending: bool,
}

/// One step of the daily sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepAction {
  /// Delete the script: a flock script with no bundle row, or an old dry-run script.
  Delete(String),
  /// Rebuild the flock's script from its rows.
  Reupload(Uuid),
  /// Re-tag the flock's script for its owner.
  Retag { flock: Uuid, owner: Uuid },
}

/// The flock a `flock-` script name is for.
fn flock_of(name: &str) -> Option<Uuid> {
  name
    .strip_prefix("flock-")
    .filter(|hex| hex.len() == 32 && is_lower_hex(hex))
    .and_then(|hex| Uuid::parse_str(hex).ok())
}

/// Reconciles a listing with the bundle rows, listing deletes first, then each row's step. Never
/// touches a script whose name dovecote would not have made, nor a flock whose lease is held.
pub fn plan_sweep(scripts: &[ListedScript], bundles: &[SweepBundle], now: i64) -> Vec<SweepAction> {
  let mut plan = Vec::new();
  for script in scripts.iter().filter(|s| is_script_name(&s.name)) {
    let orphan = match flock_of(&script.name) {
      Some(flock) => !bundles.iter().any(|b| b.flock_id == flock),
      None => script
        .modified_at
        .is_none_or(|at| now - at > TEST_SCRIPT_MAX_AGE_SECS),
    };
    if orphan {
      plan.push(SweepAction::Delete(script.name.clone()));
    }
  }
  for bundle in bundles.iter().filter(|b| !b.leased) {
    let listed = scripts
      .iter()
      .any(|s| flock_of(&s.name) == Some(bundle.flock_id));
    if bundle.stale_pending || bundle.uploaded != listed {
      plan.push(SweepAction::Reupload(bundle.flock_id));
    } else if listed && bundle.account_tag != account_tag(bundle.owner) {
      plan.push(SweepAction::Retag {
        flock: bundle.flock_id,
        owner: bundle.owner,
      });
    }
  }
  plan
}

/// Whether the cron firing scheduled at `scheduled_ms` is the day's first, the one the sweep
/// rides. A missed firing skips that day's sweep; the next day's catches up.
pub fn sweep_due(scheduled_ms: f64) -> bool {
  scheduled_ms.rem_euclid(DAY_MS) < CRON_PERIOD_MS
}

/// The keys of a dry-run bundle: the candidate's `<rule>-v<version>`, and the deployed
/// version's beside it when there is one. `None` unless `rule` is a uuid and the two differ.
#[cfg_attr(
  not(test),
  expect(dead_code, reason = "the test route uses it once dry runs dispatch")
)]
pub fn test_bundle_keys(
  rule: &str,
  candidate: i32,
  deployed: Option<i32>,
) -> Option<(String, Option<String>)> {
  if !is_uuid(rule) || deployed == Some(candidate) {
    return None;
  }
  let key = |version: i32| {
    let version = version.to_string();
    let mut key = String::with_capacity(rule.len() + 2 + version.len());
    key.push_str(rule);
    key.push_str("-v");
    key.push_str(&version);
    key
  };
  Some((key(candidate), deployed.map(key)))
}

/// Sends one call with the token and reads its answer, aborting it at [`CF_CALL_TIMEOUT`].
pub async fn send(env: &Env, req: &CfRequest) -> CfAnswer {
  let Ok(token) = env.secret(RULES_TOKEN) else {
    console_error!("rules: {RULES_TOKEN} is not set");
    return CfAnswer::Refused {
      status: 0,
      codes: Vec::new(),
    };
  };
  let token = token.to_string();
  let mut bearer = String::with_capacity(7 + token.len());
  bearer.push_str("Bearer ");
  bearer.push_str(&token);
  let mut init = RequestInit::default();
  init.with_method(match req.method {
    CfMethod::Get => Method::Get,
    CfMethod::Put => Method::Put,
    CfMethod::Delete => Method::Delete,
  });
  let headers_ok = init.headers.set("Authorization", &bearer).is_ok()
    && req
      .content_type
      .as_deref()
      .is_none_or(|ct| init.headers.set("Content-Type", ct).is_ok());
  if !headers_ok {
    console_error!("rules: could not set the API call's headers");
    return CfAnswer::Refused {
      status: 0,
      codes: Vec::new(),
    };
  }
  if let Some(body) = &req.body {
    init.body = Some(body.as_str().into());
  }
  let Ok(request) = Request::new_with_init(&req.url, &init) else {
    console_error!("rules: could not build the API call");
    return CfAnswer::Refused {
      status: 0,
      codes: Vec::new(),
    };
  };
  let controller = AbortController::default();
  let signal = controller.signal();
  let call = async {
    let mut resp = Fetch::Request(request)
      .send_with_signal(&signal)
      .await
      .ok()?;
    let status = resp.status_code();
    let retry_after = resp.headers().get("retry-after").ok().flatten();
    let body = resp.text().await.ok()?;
    Some((status, retry_after, body))
  };
  let answer = match select(pin!(call), pin!(Delay::from(CF_CALL_TIMEOUT))).await {
    Either::Left((Some((status, retry_after, body)), _)) => {
      read_answer(status, retry_after.as_deref(), &body)
    }
    Either::Left((None, _)) => CfAnswer::Unsure,
    Either::Right(((), _)) => {
      controller.abort();
      CfAnswer::Unsure
    }
  };
  match &answer {
    CfAnswer::Done(_) | CfAnswer::Missing => {}
    CfAnswer::Refused { status, codes } => {
      console_error!("rules: API call refused, HTTP {status}, codes {codes:?}");
    }
    CfAnswer::Busy(_) => console_error!("rules: API call answered 429"),
    CfAnswer::Invalid(_) => console_error!("rules: upload refused, the script did not load"),
    CfAnswer::Unsure => console_error!("rules: API call unanswered or 5xx"),
  }
  answer
}

/// A canonical lowercase uuid, as script names and tags spell it.
fn uuid_str(id: Uuid) -> String {
  let mut buf = Uuid::encode_buffer();
  id.hyphenated().encode_lower(&mut buf).to_string()
}

/// The answer for a script dovecote could not build: never sent, nothing changed.
fn unbuildable() -> CfAnswer {
  console_error!("rules: could not build the script's upload");
  CfAnswer::Refused {
    status: 0,
    codes: Vec::new(),
  }
}

/// Uploads `flock`'s script built from `sources`, or deletes it when `sources` is empty. `Ok(true)`
/// when the flock has no script afterwards; `Err` carries the answer that refused or did not
/// confirm the change. Spends no limiter token: the caller has.
pub async fn put_flock_script(
  env: &Env,
  ns: &Namespace,
  flock: Uuid,
  account: Uuid,
  sources: &[BundleSource],
) -> Result<bool, CfAnswer> {
  let flock_id = uuid_str(flock);
  let Some(name) = script_name(ScriptKind::Flock, &flock_id) else {
    return Err(unbuildable());
  };
  if sources.is_empty() {
    let Some(req) = delete_request(ns, &name) else {
      return Err(unbuildable());
    };
    return match send(env, &req).await {
      CfAnswer::Done(_) | CfAnswer::Missing => Ok(true),
      other => Err(other),
    };
  }
  let keys: Vec<String> = sources.iter().map(|s| uuid_str(s.rule_id)).collect();
  let rules: Vec<BundleRule> = sources
    .iter()
    .zip(&keys)
    .map(|(s, key)| BundleRule {
      key,
      version: s.version,
      source: &s.source,
    })
    .collect();
  let req = bundle_modules(&rules).and_then(|modules| {
    let metadata = upload_metadata(ScriptKind::Flock, &uuid_str(account), &flock_id, ns.env())?;
    upload_request(ns, &name, &boundary()?, &metadata, &modules)
  });
  let Some(req) = req else {
    return Err(unbuildable());
  };
  match send(env, &req).await {
    CfAnswer::Done(_) => Ok(false),
    other => Err(other),
  }
}

/// A dry run's modules: the harness, the candidate at `candidate` and, when the rule has one,
/// the deployed version beside it, under the keys [`test_bundle_keys`] gives.
#[cfg_attr(
  not(test),
  expect(dead_code, reason = "the test route uses it once dry runs dispatch")
)]
pub fn test_script_modules(
  rule: Uuid,
  candidate: (i32, &str),
  deployed: Option<(i32, &str)>,
) -> Option<Vec<(String, String)>> {
  let (key, deployed_key) = test_bundle_keys(&uuid_str(rule), candidate.0, deployed.map(|d| d.0))?;
  let mut rules = vec![BundleRule {
    key: &key,
    version: candidate.0,
    source: candidate.1,
  }];
  if let (Some(dkey), Some((version, source))) = (deployed_key.as_deref(), deployed) {
    rules.push(BundleRule {
      key: dkey,
      version,
      source,
    });
  }
  bundle_modules(&rules)
}

/// Uploads `rule`'s dry-run script, `test-<rule>`, at the dry run's CPU limit and tagged `test`.
#[expect(dead_code, reason = "the test route calls it once dry runs dispatch")]
pub async fn put_test_script(
  env: &Env,
  ns: &Namespace,
  rule: Uuid,
  account: Uuid,
  modules: &[(String, String)],
) -> Result<(), CfAnswer> {
  let rule_id = uuid_str(rule);
  let req = script_name(ScriptKind::Test, &rule_id).and_then(|name| {
    let metadata = upload_metadata(ScriptKind::Test, &uuid_str(account), &rule_id, ns.env())?;
    upload_request(ns, &name, &boundary()?, &metadata, modules)
  });
  let Some(req) = req else {
    return Err(unbuildable());
  };
  match send(env, &req).await {
    CfAnswer::Done(_) => Ok(()),
    other => Err(other),
  }
}

/// Deletes `rule`'s dry-run script; one already gone counts as deleted. A failure is left to the
/// daily sweep, which deletes test scripts older than [`TEST_SCRIPT_MAX_AGE_SECS`].
#[expect(dead_code, reason = "the test route calls it once dry runs dispatch")]
pub async fn delete_test_script(env: &Env, ns: &Namespace, rule: Uuid) -> Result<(), CfAnswer> {
  let Some(req) =
    script_name(ScriptKind::Test, &uuid_str(rule)).and_then(|name| delete_request(ns, &name))
  else {
    return Err(unbuildable());
  };
  match send(env, &req).await {
    CfAnswer::Done(_) | CfAnswer::Missing => Ok(()),
    other => Err(other),
  }
}

/// Why a rule route that reached the namespace refused.
#[derive(Debug)]
pub enum Refusal {
  /// The flock has no such rule.
  NotFound,
  /// Over the API budget, ours or Cloudflare's: 429 with this wait.
  Busy(u32),
  /// Any other refusal, its status and text.
  Status(u16, String),
}

impl Refusal {
  fn of(answer: &CfAnswer) -> Self {
    match answer {
      CfAnswer::Busy(wait) => Refusal::Busy(*wait),
      other => {
        let (status, text) = failure_response(other);
        Refusal::Status(status, text)
      }
    }
  }

  fn internal() -> Self {
    Refusal::Status(500, "Internal Server Error".to_string())
  }
}

const DEPLOY_BUSY: &str = "Conflict: a deploy of this flock's rules is in progress";
const DEPLOY_OVERTAKEN: &str =
  "Conflict: another deploy of this flock's rules finished first; try again";

/// The owning organization of a flock with rules, which its script is tagged for: rules open only
/// to listed organizations, so a personal flock never gets this far.
fn rules_account(access: &FlockAccess) -> Result<(Uuid, Uuid), Refusal> {
  let flock = Uuid::parse_str(access.flock_id()).map_err(|_| Refusal::internal())?;
  let account = access.org_id().ok_or_else(Refusal::internal)?;
  Ok((flock, account))
}

/// Deploys `version` of a rule: takes the flock's lease, uploads the flock's script with the
/// version in it, then promotes it. On any failure the old script stays live and the version is
/// no longer pending, unless the upload may have landed, when it stays pending for a retry or the
/// sweep. The caller has already refused what [`super::rule_admin::deploy_refusal`] refuses.
pub async fn deploy_version(
  env: &Env,
  ns: &Namespace,
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
  version: i32,
  reset_state: bool,
) -> Result<Rule, Refusal> {
  let (flock, account) = rules_account(access)?;
  let started = start_deploy(client, access, rule_id, version, account).await;
  let (lease, sources) = match started.map_err(|_| Refusal::internal())? {
    DeployStart::Busy => return Err(Refusal::Status(409, DEPLOY_BUSY.to_string())),
    DeployStart::Gone => return Err(Refusal::NotFound),
    DeployStart::Started { lease, sources } => (lease, sources),
  };
  let refused = if !sources.iter().any(|s| s.rule_id == rule_id) {
    // Blocked since the route read it: the script would be rebuilt without the candidate.
    let (status, text) = DeployRefusal::Blocked.response();
    Some((Refusal::Status(status, text.to_string()), false))
  } else if !cf_api_allowed(env).await {
    Some((Refusal::Busy(CF_API_WINDOW_SECS), false))
  } else {
    match put_flock_script(env, ns, flock, account, &sources).await {
      Ok(_) => None,
      Err(answer) => Some((Refusal::of(&answer), answer.may_have_landed())),
    }
  };
  if let Some((refusal, may_have_landed)) = refused {
    if let Err(e) = abandon_deploy(client, access, rule_id, version, lease, may_have_landed).await {
      console_error!("rules: deploy abandon failed: {e}");
    }
    return Err(refusal);
  }
  match finish_deploy(
    client,
    access,
    rule_id,
    version,
    lease,
    reset_state,
    account,
  )
  .await
  {
    Ok(DeployFinish::Promoted(rule)) => Ok(rule),
    Ok(DeployFinish::LeaseLost) => Err(Refusal::Status(409, DEPLOY_OVERTAKEN.to_string())),
    Err(_) => Err(Refusal::internal()),
  }
}

/// Deletes a rule the flock's script holds: disables it, rebuilds the script without it (or
/// deletes the script when it was the last), then deletes the rows. A failure leaves the rule
/// disabled and in place, to delete again.
pub async fn delete_scripted_rule(
  env: &Env,
  ns: &Namespace,
  client: &mut Client,
  access: &FlockAccess,
  rule_id: Uuid,
) -> Result<(), Refusal> {
  let (flock, account) = rules_account(access)?;
  let started = start_rebuild(client, flock, Some(rule_id), account).await;
  let (lease, sources) = match started.map_err(|_| Refusal::internal())? {
    RebuildStart::Busy => return Err(Refusal::Status(409, DEPLOY_BUSY.to_string())),
    RebuildStart::NotFound => return Err(Refusal::NotFound),
    RebuildStart::Started { lease, sources } => (lease, sources),
  };
  let outcome = if cf_api_allowed(env).await {
    put_flock_script(env, ns, flock, account, &sources)
      .await
      .map_err(|answer| Refusal::of(&answer))
  } else {
    Err(Refusal::Busy(CF_API_WINDOW_SECS))
  };
  let script_gone = match outcome {
    Ok(gone) => gone,
    Err(refusal) => {
      if let Err(e) = release_deploy_lease(client, flock, lease).await {
        console_error!("Rule lease release error: {e}");
      }
      return Err(refusal);
    }
  };
  match finish_rebuild(client, flock, lease, Some(rule_id), script_gone, account).await {
    Ok(true) => Ok(()),
    Ok(false) => Err(Refusal::Status(409, DEPLOY_OVERTAKEN.to_string())),
    Err(_) => Err(Refusal::internal()),
  }
}

/// Deletes a deleted flock's script, best effort: called after the flock's own commit, so a
/// failure is logged and left to the daily sweep. It spends a token whatever the limiter says,
/// since skipping it would leave the customer's code behind.
pub async fn delete_flock_script(env: &Env, ns: &Namespace, flock: Uuid) {
  let _ = cf_api_allowed(env).await;
  let Some(req) =
    script_name(ScriptKind::Flock, &uuid_str(flock)).and_then(|name| delete_request(ns, &name))
  else {
    return;
  };
  if !matches!(send(env, &req).await, CfAnswer::Done(_) | CfAnswer::Missing) {
    console_error!("rules: a deleted flock's script was not deleted; the sweep will");
  }
}

/// Rebuilds one flock's script from its rows for the sweep, under the flock's lease.
async fn rebuild_flock(env: &Env, ns: &Namespace, client: &mut Client, flock: Uuid) -> bool {
  let Ok(Some(owner)) = flock_owner(client, flock).await else {
    return false;
  };
  let Ok(RebuildStart::Started { lease, sources }) =
    start_rebuild(client, flock, None, owner).await
  else {
    return false;
  };
  match put_flock_script(env, ns, flock, owner, &sources).await {
    Ok(gone) => finish_rebuild(client, flock, lease, None, gone, owner)
      .await
      .unwrap_or(false),
    Err(_) => {
      if let Err(e) = release_deploy_lease(client, flock, lease).await {
        console_error!("Rule lease release error: {e}");
      }
      false
    }
  }
}

/// Re-tags one flock's script for `owner` and records it.
async fn retag_flock(env: &Env, ns: &Namespace, client: &Client, flock: Uuid, owner: Uuid) -> bool {
  let flock_id = uuid_str(flock);
  let req = script_name(ScriptKind::Flock, &flock_id).and_then(|name| {
    let tags = script_tags(ScriptKind::Flock, &uuid_str(owner), &flock_id, ns.env())?;
    tags_request(ns, &name, &tags)
  });
  let Some(req) = req else {
    return false;
  };
  matches!(send(env, &req).await, CfAnswer::Done(_))
    && retag_bundle(client, flock, owner).await.is_ok()
}

/// The daily sweep, on the cron's first firing of each UTC day: reconciles the namespace with
/// Postgres per [`plan_sweep`]. Stops when the API budget is spent, or when the listing cannot be
/// read whole, since a script missing from a partial listing would read as lost.
pub async fn sweep_rule_scripts(env: &Env, scheduled_ms: f64) -> worker::Result<()> {
  if !sweep_due(scheduled_ms) {
    return Ok(());
  }
  let Some(ns) = Namespace::of(env) else {
    return Ok(());
  };
  let mut scripts = Vec::new();
  let mut cursor: Option<String> = None;
  let mut whole = false;
  for _ in 0..MAX_LIST_PAGES {
    if !cf_api_allowed(env).await {
      console_error!("rules sweep: the API budget is spent; tomorrow's sweep catches up");
      return Ok(());
    }
    let Some(req) = list_request(&ns, cursor.as_deref()) else {
      break;
    };
    let CfAnswer::Done(page) = send(env, &req).await else {
      break;
    };
    let (mut listed, next) = listing(&page);
    scripts.append(&mut listed);
    match next {
      Some(next) => cursor = Some(next),
      None => {
        whole = true;
        break;
      }
    }
  }
  if !whole {
    console_error!("rules sweep: the listing was not read whole; nothing swept");
    return Ok(());
  }
  let mut client = get_db_client(env).await?;
  let bundles = sweep_bundles(&client).await?;
  let now = OffsetDateTime::now_utc().unix_timestamp();
  let plan = plan_sweep(&scripts, &bundles, now);
  let (mut done, mut failed) = (0u32, 0u32);
  for action in &plan {
    if !cf_api_allowed(env).await {
      console_error!("rules sweep: the API budget is spent; tomorrow's sweep catches up");
      break;
    }
    let ok = match action {
      SweepAction::Delete(name) => match delete_request(&ns, name) {
        Some(req) => matches!(send(env, &req).await, CfAnswer::Done(_) | CfAnswer::Missing),
        None => false,
      },
      SweepAction::Reupload(flock) => rebuild_flock(env, &ns, &mut client, *flock).await,
      SweepAction::Retag { flock, owner } => retag_flock(env, &ns, &client, *flock, *owner).await,
    };
    if ok {
      done += 1;
    } else {
      failed += 1;
    }
  }
  console_log!(
    "rules sweep: {} scripts listed, {} steps planned, {done} done, {failed} failed",
    scripts.len(),
    plan.len()
  );
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::helpers::rules::{
    BundleRule, RULE_CPU_MS, RULE_SCRIPT_TAG, RULES_COMPATIBILITY_DATE, TEST_RULE_CPU_MS,
    bundle_modules, upload_metadata,
  };
  use serde_json::json;

  const ACCT: &str = "7fc00cca2ed8361b4a472cbe3c24fb5f";
  const FLOCK: &str = "11111111-2222-4333-8444-555555555555";
  const FLOCK_NAME: &str = "flock-11111111222243338444555555555555";
  const RULE: &str = "0b5e7c1a-2d3f-4a5b-8c6d-7e8f9a0b1c2d";
  const ACCOUNT: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
  const OTHER: &str = "bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee";
  const BOUNDARY: &str = "0123456789abcdef0123456789abcdef";
  const SCRIPTS: &str = "https://api.cloudflare.com/client/v4/accounts/7fc00cca2ed8361b4a472cbe3c24fb5f/workers/dispatch/namespaces/dovecote-rules-staging/scripts";

  fn staging() -> Namespace {
    Namespace::from_vars(Some(ACCT), Some("dovecote-rules-staging")).expect("staging vars")
  }

  #[test]
  fn the_namespace_comes_only_from_its_two_names() {
    assert_eq!(staging().env(), RulesEnv::Staging);
    let prod = Namespace::from_vars(Some(ACCT), Some("dovecote-rules")).expect("prod vars");
    assert_eq!(prod.env(), RulesEnv::Production);
    for (acct, ns) in [
      (None, Some("dovecote-rules-staging")),
      (Some(ACCT), None),
      (Some(ACCT), Some("dovecote-rules-dev")),
      (Some(ACCT), Some("dovecote-rules-staging/../x")),
      (Some(&ACCT[1..]), Some("dovecote-rules")),
      (
        Some("7FC00CCA2ED8361B4A472CBE3C24FB5F"),
        Some("dovecote-rules"),
      ),
      (
        Some("7fc00cca2ed8361b4a472cbe3c24fb5f/"),
        Some("dovecote-rules"),
      ),
    ] {
      assert!(Namespace::from_vars(acct, ns).is_none(), "{acct:?} {ns:?}");
    }
  }

  #[test]
  fn script_names_match_one_pattern() {
    for good in [FLOCK_NAME, "test-0b5e7c1a2d3f4a5b8c6d7e8f9a0b1c2d"] {
      assert!(is_script_name(good), "{good}");
    }
    for bad in [
      "",
      "dovecote",
      "dovecote-staging",
      "flock-",
      "flock-11111111-2222-4333-8444-555555555555",
      "flock-1111111122224333844455555555555",
      "flock-111111112222433384445555555555555",
      "FLOCK-11111111222243338444555555555555",
      "flock-11111111222243338444555555555555/tags",
      "flock-1111111122224333844455555555555G",
      "other-11111111222243338444555555555555",
    ] {
      assert!(!is_script_name(bad), "{bad}");
    }
  }

  #[test]
  fn every_request_names_the_namespace_and_a_checked_script() {
    let ns = staging();
    let modules = vec![("main.js".to_string(), "export default {};".to_string())];
    let up = upload_request(&ns, FLOCK_NAME, BOUNDARY, "{}", &modules).expect("valid");
    assert_eq!(up.method, CfMethod::Put);
    assert_eq!(up.url, format!("{SCRIPTS}/{FLOCK_NAME}"));
    assert_eq!(
      up.content_type.as_deref(),
      Some(format!("multipart/form-data; boundary={BOUNDARY}").as_str())
    );

    let del = delete_request(&ns, FLOCK_NAME).expect("valid");
    assert_eq!(
      (del.method, del.url.as_str()),
      (CfMethod::Delete, format!("{SCRIPTS}/{FLOCK_NAME}").as_str())
    );
    assert_eq!(del.body, None);

    let tags = ["pidgeiot-rule".to_string(), "acct-x".to_string()];
    let put = tags_request(&ns, FLOCK_NAME, &tags).expect("valid");
    assert_eq!(put.method, CfMethod::Put);
    assert_eq!(put.url, format!("{SCRIPTS}/{FLOCK_NAME}/tags"));
    assert_eq!(put.content_type.as_deref(), Some("application/json"));
    assert_eq!(
      serde_json::from_str::<Value>(put.body.as_deref().expect("body")).expect("JSON"),
      json!(["pidgeiot-rule", "acct-x"])
    );

    assert!(LIST_QUERY.ends_with(&format!("={RULE_SCRIPT_TAG}:yes")));
    let list = list_request(&ns, None).expect("first page");
    assert_eq!(
      (list.method, list.url.as_str()),
      (
        CfMethod::Get,
        format!("{SCRIPTS}?tags=pidgeiot-rule:yes").as_str()
      )
    );
    let next = list_request(&ns, Some("AbC-_.9=")).expect("cursor");
    assert_eq!(
      next.url,
      format!("{SCRIPTS}?tags=pidgeiot-rule:yes&cursor=AbC-_.9=")
    );
    for cursor in ["a&b", "a b", "a/b", "a%2f", "a#b"] {
      assert!(list_request(&ns, Some(cursor)).is_none(), "{cursor}");
    }

    for bad in ["dovecote-staging", "flock-x", "../dovecote"] {
      assert!(
        upload_request(&ns, bad, BOUNDARY, "{}", &modules).is_none(),
        "{bad}"
      );
      assert!(delete_request(&ns, bad).is_none(), "{bad}");
      assert!(tags_request(&ns, bad, &tags).is_none(), "{bad}");
    }
  }

  /// The parts of a multipart body under its own boundary: (Content-Disposition line, payload).
  fn parts(body: &str, boundary: &str) -> Vec<(String, String)> {
    let delimiter = format!("--{boundary}");
    let tail = format!("{delimiter}--\r\n");
    let body = body.strip_suffix(&tail).expect("closing delimiter");
    body
      .split(&delimiter)
      .filter(|p| !p.is_empty())
      .map(|p| {
        let p = p.strip_prefix("\r\n").expect("part opens");
        let p = p.strip_suffix("\r\n").expect("part closes");
        let (head, payload) = p.split_once("\r\n\r\n").expect("head");
        (
          head.lines().next().expect("disposition").to_string(),
          payload.to_string(),
        )
      })
      .collect()
  }

  #[test]
  fn an_upload_carries_dovecotes_metadata_and_the_rule_only_as_a_module() {
    // A rule that tries to reach the metadata: a forged part under a guessed
    // boundary, a binding on our own Durable Object class, a compatibility flag.
    let hostile = "// --ffffffffffffffffffffffffffffffff\r\n\
      Content-Disposition: form-data; name=\"metadata\"\r\n\r\n\
      {\"bindings\":[{\"type\":\"durable_object_namespace\",\"name\":\"P\",\
      \"class_name\":\"Pigeons\",\
      \"script_name\":\"dovecote-staging\"}],\"compatibility_flags\":[\"nodejs_compat\"]}\r\n\
      export default () => ({});\n";
    let rules = [BundleRule {
      key: RULE,
      version: 2,
      source: hostile,
    }];
    let modules = bundle_modules(&rules).expect("valid keys");
    let metadata =
      upload_metadata(ScriptKind::Flock, ACCOUNT, FLOCK, RulesEnv::Staging).expect("uuids");
    let req = upload_request(&staging(), FLOCK_NAME, BOUNDARY, &metadata, &modules).expect("valid");
    let parts = parts(req.body.as_deref().expect("body"), BOUNDARY);
    let metas: Vec<_> = parts
      .iter()
      .filter(|(d, _)| d.contains("name=\"metadata\""))
      .collect();
    assert_eq!(metas.len(), 1, "one metadata part, dovecote's");
    assert_eq!(metas[0].1, metadata);
    let meta: Value = serde_json::from_str(&metas[0].1).expect("JSON");
    assert_eq!(meta["bindings"], json!([]));
    assert!(meta.get("compatibility_flags").is_none());
    assert!(meta.get("migrations").is_none());
    let names: Vec<String> = parts
      .iter()
      .filter(|(d, _)| !d.contains("name=\"metadata\""))
      .map(|(d, _)| d.clone())
      .collect();
    let expected: Vec<String> = modules
      .iter()
      .map(|(n, _)| format!("Content-Disposition: form-data; name=\"{n}\"; filename=\"{n}\""))
      .collect();
    assert_eq!(names, expected, "only the bundle's own modules");
    let rule_part = format!("r-{RULE}.js");
    for (disposition, payload) in &parts {
      assert_eq!(
        payload.contains("Pigeons"),
        disposition.contains(&rule_part),
        "the rule's text stays inside its own module: {disposition}"
      );
    }
  }

  #[test]
  fn a_boundary_is_fresh_lowercase_hex() {
    let a = boundary().expect("random");
    let b = boundary().expect("random");
    assert_eq!(a.len(), 32);
    assert!(
      a.bytes()
        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    );
    assert_ne!(a, b);
  }

  #[test]
  fn answers_read_as_the_route_needs_them() {
    let ok = r#"{"result":{"id":"flock-x"},"success":true,"errors":[],"messages":[]}"#;
    assert_eq!(
      read_answer(200, None, ok),
      CfAnswer::Done(serde_json::from_str(ok).expect("JSON"))
    );
    assert_eq!(read_answer(429, Some("120"), "{}"), CfAnswer::Busy(120));
    assert_eq!(read_answer(429, None, "not json"), CfAnswer::Busy(300));

    let invalid = r#"{"success":false,"errors":[{"code":10021,"message":"Uncaught Error: No such module \"missing.js\".\n  imported from \"index.js\""}]}"#;
    assert_eq!(
      read_answer(400, None, invalid),
      CfAnswer::Invalid(
        "Uncaught Error: No such module \"missing.js\".\n  imported from \"index.js\"".into()
      )
    );
    let startup = r#"{"success":false,"errors":[{"code":10021,"message":"Error: Script startup exceeded CPU time limit."}]}"#;
    assert_eq!(
      read_answer(400, None, startup),
      CfAnswer::Invalid("Error: Script startup exceeded CPU time limit.".into())
    );

    let gone =
      r#"{"success":false,"errors":[{"code":10007,"message":"This Worker does not exist"}]}"#;
    assert_eq!(read_answer(404, None, gone), CfAnswer::Missing);
    assert_eq!(read_answer(404, None, ""), CfAnswer::Missing);

    let denied = r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}]}"#;
    assert_eq!(
      read_answer(403, None, denied),
      CfAnswer::Refused {
        status: 403,
        codes: vec![10000]
      }
    );
    assert_eq!(
      read_answer(200, None, r#"{"success":false,"errors":[]}"#),
      CfAnswer::Refused {
        status: 200,
        codes: vec![]
      }
    );
    assert_eq!(read_answer(200, None, "<html>"), CfAnswer::Unsure);
    for status in [500, 502, 503, 504] {
      assert_eq!(
        read_answer(status, None, invalid),
        CfAnswer::Unsure,
        "{status}"
      );
    }
  }

  #[test]
  fn a_load_error_reads_cleanly_and_bounded() {
    let noisy = "a\u{0}b\u{1b}[31mc\r\nd\te\n";
    assert_eq!(readable_load_error(noisy), "a b [31mc \nd e");
    let long = "é".repeat(2_000);
    let cut = readable_load_error(&long);
    assert!(cut.len() <= MAX_LOAD_ERROR_BYTES && cut.chars().all(|c| c == 'é'));
    let (status, text) = failure_response(&CfAnswer::Invalid("Error: boom".into()));
    assert_eq!(status, 422);
    assert!(text.starts_with("Unprocessable Content: ") && text.ends_with("Error: boom"));
  }

  #[test]
  fn no_failure_answers_401_and_only_an_unsure_answer_keeps_the_deploy_pending() {
    let all = [
      CfAnswer::Busy(60),
      CfAnswer::Invalid("x".into()),
      CfAnswer::Missing,
      CfAnswer::Refused {
        status: 403,
        codes: vec![10000],
      },
      CfAnswer::Refused {
        status: 401,
        codes: vec![],
      },
      CfAnswer::Unsure,
    ];
    for answer in &all {
      let (status, _) = failure_response(answer);
      assert!(matches!(status, 422 | 429 | 503), "{answer:?} -> {status}");
      assert_eq!(
        answer.may_have_landed(),
        *answer == CfAnswer::Unsure,
        "{answer:?}"
      );
    }
    assert_eq!(failure_response(&CfAnswer::Busy(60)).0, 429);
  }

  #[test]
  fn a_listing_reads_names_tags_times_and_its_cursor() {
    let page = json!({
      "result": [
        { "created_on": "2026-11-03T22:00:00Z", "modified_on": "2026-11-03T22:02:20.871877Z",
          "script": { "id": FLOCK_NAME, "tags": ["pidgeiot-rule", "acct-x"],
                      "modified_on": "2026-11-03T22:02:20.871877Z" } },
        { "id": "test-0b5e7c1a2d3f4a5b8c6d7e8f9a0b1c2d", "tags": null,
          "modified_on": "2026-11-03T21:00:00Z" },
        { "script": { "tags": [] } }
      ],
      "result_info": { "count": 3, "cursor": "next-1" },
      "success": true
    });
    let (scripts, cursor) = listing(&page);
    assert_eq!(cursor.as_deref(), Some("next-1"));
    assert_eq!(scripts.len(), 2, "an entry without a name is skipped");
    assert_eq!(scripts[0].name, FLOCK_NAME);
    assert_eq!(scripts[0].tags, ["pidgeiot-rule", "acct-x"]);
    assert_eq!(scripts[0].modified_at, Some(1_793_743_340));
    assert!(scripts[1].tags.is_empty());
    assert_eq!(scripts[1].modified_at, Some(1_793_739_600));
    let last = json!({ "result": [], "result_info": { "count": 0, "cursor": "" } });
    assert_eq!(listing(&last), (vec![], None));
  }

  fn listed(name: &str, tags: &[&str], modified_at: Option<i64>) -> ListedScript {
    ListedScript {
      name: name.to_string(),
      tags: tags.iter().map(|t| t.to_string()).collect(),
      modified_at,
    }
  }

  fn bundle(flock: &str, uploaded: bool, tag: &str, owner: &str) -> SweepBundle {
    SweepBundle {
      flock_id: flock.parse().expect("uuid"),
      uploaded,
      account_tag: tag.to_string(),
      owner: owner.parse().expect("uuid"),
      leased: false,
      stale_pending: false,
    }
  }

  #[test]
  fn the_sweep_reconciles_the_listing_with_the_rows() {
    const NOW: i64 = 1_900_000_000;
    let acct = account_tag(ACCOUNT.parse().unwrap());
    assert_eq!(acct, format!("acct-{ACCOUNT}"));
    let kept = "22222222-2222-4333-8444-555555555555";
    let kept_name = "flock-22222222222243338444555555555555";
    let lost = "33333333-2222-4333-8444-555555555555";
    let moved = "44444444-2222-4333-8444-555555555555";
    let moved_name = "flock-44444444222243338444555555555555";
    let stale = "55555555-2222-4333-8444-555555555555";
    let stale_name = "flock-55555555222243338444555555555555";
    let busy = "66666666-2222-4333-8444-555555555555";
    let busy_name = "flock-66666666222243338444555555555555";
    let old_test = "test-0b5e7c1a2d3f4a5b8c6d7e8f9a0b1c2d";
    let new_test = "test-9f8e7d6c5b4a43218fedcba987654321";
    let mut stale_row = bundle(stale, true, &acct, ACCOUNT);
    stale_row.stale_pending = true;
    let mut busy_row = bundle(busy, false, &acct, ACCOUNT);
    busy_row.leased = true;
    let rows = [
      bundle(kept, true, &acct, ACCOUNT),
      bundle(lost, true, &acct, ACCOUNT),
      bundle(moved, true, &acct, OTHER),
      stale_row,
      busy_row,
    ];
    let scripts = [
      listed(FLOCK_NAME, &[], Some(NOW - 10)),
      listed(kept_name, &[], Some(NOW - 10)),
      listed(moved_name, &[], Some(NOW - 10)),
      listed(stale_name, &[], Some(NOW - 10)),
      listed(busy_name, &[], Some(NOW - 10)),
      listed(old_test, &[], Some(NOW - TEST_SCRIPT_MAX_AGE_SECS - 1)),
      listed(new_test, &[], Some(NOW - 60)),
      listed("dovecote-staging", &[RULE_SCRIPT_TAG], Some(0)),
    ];
    let plan = plan_sweep(&scripts, &rows, NOW);
    assert_eq!(
      plan,
      vec![
        SweepAction::Delete(FLOCK_NAME.to_string()),
        SweepAction::Delete(old_test.to_string()),
        SweepAction::Reupload(lost.parse().unwrap()),
        SweepAction::Retag {
          flock: moved.parse().unwrap(),
          owner: OTHER.parse().unwrap()
        },
        SweepAction::Reupload(stale.parse().unwrap()),
      ]
    );

    // A test script whose time did not read is old enough: it can only be an orphan.
    let untimed = [listed(old_test, &[], None)];
    assert_eq!(
      plan_sweep(&untimed, &[], NOW),
      vec![SweepAction::Delete(old_test.to_string())]
    );
    // A row never uploaded, with nothing pending, is a refused first deploy: nothing to do.
    let refused = [bundle(lost, false, &acct, ACCOUNT)];
    assert!(plan_sweep(&[], &refused, NOW).is_empty());
    // The same row with a script listed: whatever landed is rebuilt from the rows.
    let landed = [listed(
      "flock-33333333222243338444555555555555",
      &[],
      Some(NOW),
    )];
    assert_eq!(
      plan_sweep(&landed, &refused, NOW),
      vec![SweepAction::Reupload(lost.parse().unwrap())]
    );
  }

  #[test]
  fn the_sweep_runs_on_the_first_firing_of_each_utc_day() {
    const DAY_MS: f64 = 86_400_000.0;
    let midnight = 20_000.0 * DAY_MS;
    assert!(sweep_due(midnight));
    assert!(sweep_due(midnight + 299_999.0));
    assert!(!sweep_due(midnight + 300_000.0));
    assert!(!sweep_due(midnight - 1.0));
    let firings = (0..288)
      .filter(|i| sweep_due(midnight + f64::from(*i) * 300_000.0))
      .count();
    assert_eq!(firings, 1);
  }

  #[test]
  fn a_dry_run_bundle_holds_the_candidate_beside_the_deployed_version() {
    let keys = test_bundle_keys(RULE, 4, Some(3)).expect("uuid");
    assert_eq!(keys, (format!("{RULE}-v4"), Some(format!("{RULE}-v3"))));
    assert_eq!(test_bundle_keys(RULE, 4, None).expect("uuid").1, None);
    assert!(test_bundle_keys("x", 4, None).is_none());
    assert!(
      test_bundle_keys(RULE, 4, Some(4)).is_none(),
      "one version twice"
    );
  }

  #[test]
  fn a_dry_run_script_holds_both_versions_under_their_own_keys() {
    let rule: Uuid = RULE.parse().unwrap();
    let modules = test_script_modules(rule, (4, "new"), Some((3, "old"))).expect("valid");
    let names: Vec<&str> = modules.iter().map(|(n, _)| n.as_str()).collect();
    let (new, old) = (format!("r-{RULE}-v4.js"), format!("r-{RULE}-v3.js"));
    assert_eq!(
      names,
      [
        "main.js",
        "prelude.js",
        "pidgeiot.js",
        "rules.js",
        &new,
        &old
      ]
    );
    assert_eq!(
      (modules[4].1.as_str(), modules[5].1.as_str()),
      ("new", "old")
    );
    let alone = test_script_modules(rule, (1, "first"), None).expect("valid");
    assert_eq!(alone.len(), 5);
  }

  /// The staging integration run: `RULES_IT_DIR` holds requests.json, written here for the
  /// fixture flock `RULES_IT_FLOCK`, rule `RULES_IT_RULE` and accounts `RULES_IT_ACCOUNT` and
  /// `RULES_IT_OTHER`; `rules/namespace-it.mjs` sends them and writes answers.json, which
  /// [`namespace_it_answers`] reads.
  struct It {
    dir: std::path::PathBuf,
    flock: Uuid,
    rule: Uuid,
    account: Uuid,
    other: Uuid,
  }

  /// `None`, so the test passes untouched, where `RULES_IT_DIR` is unset: the Postgres gate runs
  /// every ignored test and must not need the staging namespace.
  fn it() -> Option<It> {
    std::env::var_os("RULES_IT_DIR")?;
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is set"));
    let id = |name: &str| var(name).parse::<Uuid>().expect("a uuid");
    Some(It {
      dir: var("RULES_IT_DIR").into(),
      flock: id("RULES_IT_FLOCK"),
      rule: id("RULES_IT_RULE"),
      account: id("RULES_IT_ACCOUNT"),
      other: id("RULES_IT_OTHER"),
    })
  }

  const SLOW_TOP: &str = "let x = 0;\nfor (let i = 0; i < 1e10; i++) x ^= i;\n\
    export default () => ({ values: [{ key: \"x\", value: String(x) }] });\n";
  const THROWING_TOP: &str =
    "throw new Error(\"the rule's own words\");\nexport default () => ({});\n";

  fn flock_upload(it: &It, ns: &Namespace, account: Uuid, source: &str) -> CfRequest {
    let key = uuid_str(it.rule);
    let rules = [BundleRule {
      key: &key,
      version: 1,
      source,
    }];
    let modules = bundle_modules(&rules).expect("valid");
    let flock = uuid_str(it.flock);
    let metadata =
      upload_metadata(ScriptKind::Flock, &uuid_str(account), &flock, ns.env()).expect("uuids");
    let name = script_name(ScriptKind::Flock, &flock).expect("uuid");
    upload_request(ns, &name, &boundary().expect("random"), &metadata, &modules).expect("valid")
  }

  #[test]
  #[ignore = "needs RULES_IT_*: the staging namespace run, rules/namespace-it.mjs"]
  fn namespace_it_requests() {
    let Some(it) = it() else {
      return;
    };
    let ns = staging();
    let flock_id = uuid_str(it.flock);
    let flock_name = script_name(ScriptKind::Flock, &flock_id).expect("uuid");
    let test_name = script_name(ScriptKind::Test, &uuid_str(it.rule)).expect("uuid");
    let r6 = include_str!("../../rules/r6-watchdog.js");
    let r7 = include_str!("../../rules/r7-reboot-loop.js");
    let test_modules = test_script_modules(it.rule, (2, r7), Some((1, r6))).expect("valid");
    let test_metadata = upload_metadata(
      ScriptKind::Test,
      &uuid_str(it.account),
      &uuid_str(it.rule),
      ns.env(),
    )
    .expect("uuids");
    let retag =
      script_tags(ScriptKind::Flock, &uuid_str(it.other), &flock_id, ns.env()).expect("uuids");
    let steps: Vec<(&str, CfRequest)> = vec![
      ("upload-flock", flock_upload(&it, &ns, it.account, r6)),
      (
        "upload-test",
        upload_request(
          &ns,
          &test_name,
          &boundary().unwrap(),
          &test_metadata,
          &test_modules,
        )
        .expect("valid"),
      ),
      ("list-1", list_request(&ns, None).expect("list")),
      (
        "retag",
        tags_request(&ns, &flock_name, &retag).expect("valid"),
      ),
      ("list-2", list_request(&ns, None).expect("list")),
      ("upload-slow", flock_upload(&it, &ns, it.account, SLOW_TOP)),
      (
        "upload-throw",
        flock_upload(&it, &ns, it.account, THROWING_TOP),
      ),
      (
        "delete-test",
        delete_request(&ns, &test_name).expect("valid"),
      ),
      (
        "delete-flock",
        delete_request(&ns, &flock_name).expect("valid"),
      ),
      (
        "delete-flock-again",
        delete_request(&ns, &flock_name).expect("valid"),
      ),
      ("list-3", list_request(&ns, None).expect("list")),
    ];
    let out: Vec<Value> = steps
      .iter()
      .map(|(label, r)| {
        json!({
          "label": label,
          "method": match r.method { CfMethod::Get => "GET", CfMethod::Put => "PUT",
                                     CfMethod::Delete => "DELETE" },
          "url": r.url,
          "content_type": r.content_type,
          "body": r.body,
        })
      })
      .collect();
    std::fs::write(
      it.dir.join("requests.json"),
      serde_json::to_string(&out).unwrap(),
    )
    .expect("requests written");
  }

  #[test]
  #[ignore = "needs RULES_IT_*: the staging namespace run, rules/namespace-it.mjs"]
  fn namespace_it_answers() {
    let Some(it) = it() else {
      return;
    };
    let text = std::fs::read_to_string(it.dir.join("answers.json")).expect("answers read");
    let answers: Vec<Value> = serde_json::from_str(&text).expect("JSON");
    let answer = |label: &str| {
      let a = answers.iter().find(|a| a["label"] == label).expect(label);
      read_answer(
        a["status"].as_u64().unwrap() as u16,
        a["retry_after"].as_str(),
        a["body"].as_str().unwrap(),
      )
    };
    let page = |label: &str| match answer(label) {
      CfAnswer::Done(page) => listing(&page),
      other => panic!("{label}: {other:?}"),
    };
    let flock_id = uuid_str(it.flock);
    let flock_name = script_name(ScriptKind::Flock, &flock_id).unwrap();
    let test_name = script_name(ScriptKind::Test, &uuid_str(it.rule)).unwrap();
    let tags = |kind, account, subject: &str| -> Vec<String> {
      script_tags(kind, &uuid_str(account), subject, RulesEnv::Staging)
        .unwrap()
        .to_vec()
    };

    assert!(matches!(answer("upload-flock"), CfAnswer::Done(_)));
    assert!(matches!(answer("upload-test"), CfAnswer::Done(_)));

    let (listed, cursor) = page("list-1");
    assert_eq!(cursor, None, "one page");
    // The limit the metadata set is the one the script runs under.
    let CfAnswer::Done(raw) = answer("list-1") else {
      panic!("listed");
    };
    let cpu_ms = |name: &str| {
      raw["result"]
        .as_array()
        .and_then(|r| r.iter().find(|e| e["id"] == name))
        .map(|e| {
          (
            e["limits"]["cpu_ms"].clone(),
            e["compatibility_date"].clone(),
          )
        })
    };
    assert_eq!(
      cpu_ms(&flock_name),
      Some((json!(RULE_CPU_MS), json!(RULES_COMPATIBILITY_DATE)))
    );
    assert_eq!(
      cpu_ms(&test_name),
      Some((json!(TEST_RULE_CPU_MS), json!(RULES_COMPATIBILITY_DATE)))
    );
    let find = |listed: &[ListedScript], name: &str| {
      listed.iter().find(|s| s.name == name).cloned().expect(name)
    };
    let flock = find(&listed, &flock_name);
    assert_eq!(flock.tags, tags(ScriptKind::Flock, it.account, &flock_id));
    let test = find(&listed, &test_name);
    assert_eq!(
      test.tags,
      tags(ScriptKind::Test, it.account, &uuid_str(it.rule))
    );
    let now = OffsetDateTime::now_utc().unix_timestamp();
    assert!(
      flock.modified_at.is_some_and(|at| (now - at).abs() < 3_600),
      "a readable time"
    );

    // What the sweep would do with this listing: no bundle row, so the flock script is an
    // orphan; the dry-run script is young and stays. With a row tagged for the old account it
    // would re-tag instead.
    let ours: Vec<ListedScript> = listed
      .iter()
      .filter(|s| s.name == flock_name || s.name == test_name)
      .cloned()
      .collect();
    assert_eq!(
      plan_sweep(&ours, &[], now),
      [SweepAction::Delete(flock_name.clone())]
    );
    let row = SweepBundle {
      flock_id: it.flock,
      uploaded: true,
      account_tag: account_tag(it.account),
      owner: it.other,
      leased: false,
      stale_pending: false,
    };
    assert_eq!(
      plan_sweep(&ours, &[row], now),
      [SweepAction::Retag {
        flock: it.flock,
        owner: it.other
      }]
    );

    assert!(matches!(answer("retag"), CfAnswer::Done(_)));
    let (listed, _) = page("list-2");
    assert_eq!(
      find(&listed, &flock_name).tags,
      tags(ScriptKind::Flock, it.other, &flock_id)
    );

    let CfAnswer::Invalid(slow) = answer("upload-slow") else {
      panic!("a slow top level is refused at upload");
    };
    assert!(slow.contains("startup"), "{slow}");
    let (status, text) = failure_response(&CfAnswer::Invalid(slow));
    assert_eq!(status, 422);
    assert!(text.len() <= LOAD_ERROR_HEAD.len() + MAX_LOAD_ERROR_BYTES);
    let CfAnswer::Invalid(threw) = answer("upload-throw") else {
      panic!("a throwing top level is refused at upload");
    };
    assert!(threw.contains("the rule's own words"), "{threw}");

    assert!(matches!(answer("delete-test"), CfAnswer::Done(_)));
    assert!(matches!(answer("delete-flock"), CfAnswer::Done(_)));
    assert_eq!(answer("delete-flock-again"), CfAnswer::Missing);
    let (listed, cursor) = page("list-3");
    assert!(
      listed.is_empty() && cursor.is_none(),
      "0 scripts left: {listed:?}"
    );
  }
}
