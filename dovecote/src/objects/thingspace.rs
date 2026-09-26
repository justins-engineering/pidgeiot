//! `ThingSpaceSession`, the one Durable Object per environment that holds Verizon ThingSpace
//! tokens and sends NIDD downlinks, and `send`, the client pigeon objects call it through.
//!
//! It exists for one reason: Verizon locks the account's contact record after five consecutive
//! failed logins, and per-isolate caches cannot count failures across isolates. So logins are
//! single-flight behind a mutex here, and a latch stops them after two failures per credential
//! set and login epoch. Tokens never leave this object and are never logged.

use crate::helpers::nidd::{NIDD_MT_DELIVERY_SECS, within};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use thingspace_sdk::api::{get_access_token, get_session_token, send_nidd};
use thingspace_sdk::models::{
  DeviceID, Error as SdkError, LoginResponse, NiddMessage, NiddRequest, Session, SessionRequestBody,
};
use worker::{
  Date, DurableObject, Env, Method, Request, RequestInit, Response, ResponseBuilder, Result, State,
  console_error, console_log, durable_object, wasm_bindgen,
};

/// The name the singleton's id derives from.
const SESSION_OBJECT_NAME: &str = "session";
/// Storage key of the cached tokens.
const TOKENS_KEY: &str = "tokens";
/// Storage key of the failed-login count, present only after a failed login.
const LOGIN_FAILURES_KEY: &str = "login_failures";
/// Failed logins per credential set and epoch before the latch holds. Verizon locks at five.
const LOGIN_FAILURE_LIMIT: u32 = 2;
/// An access token is refreshed this long before it expires.
const ACCESS_MARGIN_SECS: i64 = 300;
/// A session is reused while it was last used this recently; Verizon expires it at 20 idle
/// minutes.
const SESSION_IDLE_SECS: i64 = 900;
/// The ops email a latch sends, either side of the last HTTP status.
const LATCH_EMAIL_HEAD: &str = concat!(
  "dovecote stopped logging in to Verizon ThingSpace after a refused or repeated failed login ",
  "(last HTTP status "
);
const LATCH_EMAIL_TAIL: &str = concat!(
  "). Every NIDD downlink answers 503 until a ThingSpace secret changes value or ",
  "THINGSPACE_LOGIN_EPOCH is bumped. Check the account in the ThingSpace portal before either: ",
  "Verizon locks it after five failed logins."
);
/// How long any one ThingSpace call may take. The SDK takes no abort signal, so a call that
/// outlives it is abandoned, not aborted.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// The account's API credentials and this environment's login epoch.
struct Credentials {
  public_key: String,
  private_key: String,
  uws_username: String,
  uws_password: String,
  account_name: String,
  login_epoch: String,
}

impl Credentials {
  /// Every credential, or `None` when any is unset or blank: NIDD is off in this environment.
  fn from_env(env: &Env) -> Option<Self> {
    let secret = |name: &str| {
      env
        .secret(name)
        .ok()
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
    };
    Some(Self {
      public_key: secret("THINGSPACE_PUBLIC_KEY")?,
      private_key: secret("THINGSPACE_PRIVATE_KEY")?,
      uws_username: secret("THINGSPACE_UWS_USERNAME")?,
      uws_password: secret("THINGSPACE_UWS_PASSWORD")?,
      account_name: secret("THINGSPACE_ACCOUNT_NAME")?,
      login_epoch: env
        .var("THINGSPACE_LOGIN_EPOCH")
        .map(|value| value.to_string())
        .unwrap_or_default(),
    })
  }
}

/// The salted digest a failed-login count is bound to. Any changed secret value or a bumped
/// epoch changes it, which re-arms the latch; the digest is of values, so putting a secret again
/// unchanged does not.
fn fingerprint(salt: &[u8], creds: &Credentials) -> String {
  let mut hasher = Sha256::new();
  hasher.update(salt);
  hasher.update(creds.login_epoch.as_bytes());
  for part in [
    &creds.public_key,
    &creds.private_key,
    &creds.uws_username,
    &creds.uws_password,
  ] {
    hasher.update([0u8]);
    hasher.update(part.as_bytes());
  }
  hex(&hasher.finalize())
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
  const DIGITS: &[u8; 16] = b"0123456789abcdef";
  let mut out = String::with_capacity(bytes.len() * 2);
  for byte in bytes {
    out.push(char::from(DIGITS[usize::from(byte >> 4)]));
    out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
  }
  out
}

/// The cached tokens. No `Debug`, so neither can reach a log.
#[derive(Serialize, Deserialize, Default)]
struct Tokens {
  access: String,
  access_expires_at: i64,
  session: String,
  session_used_at: i64,
}

impl Tokens {
  /// Whether the access token has more than the refresh margin left.
  fn access_fresh(&self, now: i64) -> bool {
    !self.access.is_empty() && now < self.access_expires_at - ACCESS_MARGIN_SECS
  }

  /// Whether the session was used recently enough not to have idled out.
  fn session_fresh(&self, now: i64) -> bool {
    !self.session.is_empty() && now - self.session_used_at < SESSION_IDLE_SECS
  }
}

/// Consecutive failed logins under one fingerprint.
#[derive(Serialize, Deserialize)]
struct LoginFailures {
  salt: String,
  fingerprint: String,
  count: u32,
}

impl LoginFailures {
  /// Whether logins are stopped.
  fn latched(&self) -> bool {
    self.count >= LOGIN_FAILURE_LIMIT
  }
}

/// A stored count, when it was recorded against these exact credentials and epoch. Any changed
/// secret value or a bumped epoch makes it `None`, which re-arms the latch.
fn held_failures(stored: LoginFailures, creds: &Credentials) -> Option<LoginFailures> {
  let salt = salt_bytes(&stored.salt)?;
  (fingerprint(&salt, creds) == stored.fingerprint).then_some(stored)
}

/// The count one failed login leaves, given the count `held` against the same credentials (none
/// after a success, which deletes it), or `None` when the attempt does not count.
fn count_after_failure(held: Option<u32>, verdict: &LoginVerdict) -> Option<u32> {
  match verdict {
    LoginVerdict::Exempt | LoginVerdict::RetryWithFreshAccess => None,
    LoginVerdict::Latch => Some(LOGIN_FAILURE_LIMIT),
    LoginVerdict::Count => Some(held.unwrap_or(0) + 1),
  }
}

/// How a ThingSpace call failed.
#[derive(Debug, Clone, PartialEq)]
enum CallFailure {
  /// Refused before the request left the Worker.
  PreSend,
  /// The fetch failed; the request may still have reached Verizon.
  Network,
  /// No answer inside `CALL_TIMEOUT`; the request may still have reached Verizon.
  Timeout,
  /// Verizon answered an error status, with its error code when the body carried one.
  Api { status: u16, code: Option<String> },
  /// Verizon answered 2xx without the token.
  NoToken(u16),
}

impl CallFailure {
  /// The SDK's error as a failure the latch can classify.
  fn from_sdk(error: &SdkError) -> Self {
    match error {
      SdkError::Api { status, code, .. } => CallFailure::Api {
        status: *status,
        code: code.clone(),
      },
      // Also what building the request fails with, which cannot be told apart from a failed
      // fetch, so it is counted like one.
      SdkError::Worker(_) => CallFailure::Network,
      _ => CallFailure::PreSend,
    }
  }

  /// The HTTP status Verizon answered, if it answered.
  fn status(&self) -> Option<u16> {
    match self {
      CallFailure::Api { status, .. } | CallFailure::NoToken(status) => Some(*status),
      _ => None,
    }
  }

  /// Verizon's error code, if its body carried one.
  fn code(&self) -> Option<&str> {
    match self {
      CallFailure::Api { code, .. } => code.as_deref(),
      _ => None,
    }
  }

  /// The failure's name for log lines.
  fn kind(&self) -> &'static str {
    match self {
      CallFailure::PreSend => "pre_send",
      CallFailure::Network => "network",
      CallFailure::Timeout => "timeout",
      CallFailure::Api { .. } => "api",
      CallFailure::NoToken(_) => "no_token",
    }
  }
}

/// Which login call an attempt was.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LoginStep {
  OAuth,
  Session,
}

/// What a failed login attempt means for the latch.
#[derive(Debug, PartialEq)]
enum LoginVerdict {
  /// The bearer was stale or wrong, which says nothing about the UWS password: fetch a new
  /// access token and retry the login once.
  RetryWithFreshAccess,
  /// Verizon refused the credentials themselves.
  Latch,
  /// Its fate at Verizon is unknown, so it counts toward the lockout.
  Count,
  /// It never left the Worker.
  Exempt,
}

/// An API gateway fault that means the bearer was stale or wrong: `900901` Invalid Credentials,
/// `900902` Missing Credentials.
fn is_bearer_fault(code: Option<&str>) -> bool {
  matches!(code, Some("900901" | "900902"))
}

/// An M2M error code, as opposed to a numeric gateway fault or an OAuth error word.
fn is_m2m_code(code: Option<&str>) -> bool {
  code.is_some_and(|c| c.contains('.'))
}

/// Classifies a failed login attempt. `retried` is whether this attempt was already the retry
/// after a bearer fault, which is never retried again.
fn classify_login(step: LoginStep, failure: &CallFailure, retried: bool) -> LoginVerdict {
  match (step, failure) {
    (_, CallFailure::PreSend) => LoginVerdict::Exempt,
    (
      LoginStep::OAuth,
      CallFailure::Api {
        status: 400 | 401, ..
      },
    ) => LoginVerdict::Latch,
    (LoginStep::Session, CallFailure::Api { status: 401, code })
      if is_bearer_fault(code.as_deref()) =>
    {
      if retried {
        LoginVerdict::Count
      } else {
        LoginVerdict::RetryWithFreshAccess
      }
    }
    (LoginStep::Session, CallFailure::Api { code, .. }) if is_m2m_code(code.as_deref()) => {
      LoginVerdict::Latch
    }
    _ => LoginVerdict::Count,
  }
}

/// What a failed send means.
#[derive(Debug, PartialEq)]
enum SendVerdict {
  /// A stale token: drop it, log in again once, send once more.
  Retry {
    drop_access: bool,
    drop_session: bool,
  },
  /// Worth trying again later: answered 503, so the push is due on the next uplink.
  Unreachable,
  /// ThingSpace refused the message itself: answered 502, since the same bytes fail the same way.
  Refused,
}

/// Classifies a send's error status and Verizon code.
fn classify_send(status: u16, code: Option<&str>) -> SendVerdict {
  if status == 401 && is_bearer_fault(code) {
    return SendVerdict::Retry {
      drop_access: true,
      drop_session: false,
    };
  }
  if code.is_some_and(|c| c.contains(".SessionToken.")) {
    return SendVerdict::Retry {
      drop_access: false,
      drop_session: true,
    };
  }
  match status {
    408 | 429 | 500..=599 => SendVerdict::Unreachable,
    _ => SendVerdict::Refused,
  }
}

/// Why no tokens could be had.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Unavailable {
  NotConfigured,
  Latched,
  Unreachable,
}

impl Unavailable {
  fn reason(self) -> &'static str {
    match self {
      Unavailable::NotConfigured => "not_configured",
      Unavailable::Latched => "latched",
      Unavailable::Unreachable => "unreachable",
    }
  }
}

/// The internal `/send` body.
#[derive(Serialize, Deserialize)]
struct SendRequest {
  pigeon_id: String,
  imei: String,
  frame_b64: String,
  max_delivery_secs: i32,
}

/// The internal `/send` answer: a request id on 200, a reason otherwise.
#[derive(Serialize, Deserialize, Default)]
struct SendAnswer {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  request_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  reason: Option<String>,
}

/// What became of a downlink.
#[derive(Debug, PartialEq)]
pub enum SendOutcome {
  /// ThingSpace accepted it, under this request id.
  Sent(String),
  /// ThingSpace refused the message itself; the same bytes would fail the same way.
  Refused(String),
  /// Nothing was sent, or the result is unknown: not configured, latched or unreachable.
  Unavailable(String),
}

/// Sends one signed NIDD frame to the modem with this IMEI through the environment's session
/// object, with the standard delivery window. `pigeon_id` is only for its log lines.
pub async fn send(env: &Env, pigeon_id: &str, imei: &str, frame: &[u8]) -> SendOutcome {
  let unavailable = |reason: &str| SendOutcome::Unavailable(reason.to_string());
  let Ok(namespace) = env.durable_object("THINGSPACE") else {
    console_error!("thingspace send: THINGSPACE binding unavailable");
    return unavailable("unbound");
  };
  let Ok(stub) = namespace
    .id_from_name(SESSION_OBJECT_NAME)
    .and_then(|id| id.get_stub())
  else {
    console_error!("thingspace send: session object unreachable");
    return unavailable("unreachable");
  };

  let Ok(body) = serde_json::to_string(&SendRequest {
    pigeon_id: pigeon_id.to_string(),
    imei: imei.to_string(),
    frame_b64: STANDARD.encode(frame),
    max_delivery_secs: NIDD_MT_DELIVERY_SECS as i32,
  }) else {
    return unavailable("unreachable");
  };

  let mut init = RequestInit::default();
  init.with_method(Method::Post);
  init.body = Some(body.into());
  let Ok(request) = Request::new_with_init("https://internal/send", &init) else {
    return unavailable("unreachable");
  };

  let mut response = match stub.fetch_with_request(request).await {
    Ok(response) => response,
    Err(e) => {
      console_error!("thingspace send: session object dispatch failed: {e}");
      return unavailable("unreachable");
    }
  };
  let status = response.status_code();
  let answer = response.json::<SendAnswer>().await.unwrap_or_default();
  match status {
    200 => SendOutcome::Sent(answer.request_id.unwrap_or_default()),
    502 => SendOutcome::Refused(answer.reason.unwrap_or_default()),
    _ => SendOutcome::Unavailable(answer.reason.unwrap_or_else(|| "unreachable".to_string())),
  }
}

/// See the module documentation. `login` is the single flight: whoever holds it is the only
/// request in this object that may log in.
#[durable_object]
pub struct ThingSpaceSession {
  state: State,
  env: Env,
  login: futures::lock::Mutex<()>,
}

impl DurableObject for ThingSpaceSession {
  fn new(state: State, env: Env) -> Self {
    Self {
      state,
      env,
      login: futures::lock::Mutex::new(()),
    }
  }

  async fn fetch(&self, req: Request) -> Result<Response> {
    match req.path().as_str() {
      "/send" => self.send_route(req).await,
      _ => Response::error("Not Found", 404),
    }
  }
}

/// Unix seconds.
fn now_secs() -> i64 {
  (Date::now().as_millis() / 1000) as i64
}

/// A `/send` answer with this status.
fn answer(status: u16, body: &SendAnswer) -> Result<Response> {
  ResponseBuilder::new().with_status(status).from_json(body)
}

/// A `/send` answer carrying only a reason.
fn reason_answer(status: u16, reason: &str) -> Result<Response> {
  answer(
    status,
    &SendAnswer {
      request_id: None,
      reason: Some(reason.to_string()),
    },
  )
}

impl ThingSpaceSession {
  /// `POST /send`: one downlink, answering 200 with ThingSpace's request id, 502 with Verizon's
  /// error code when it refused the message, or 503 with why nothing could be sent.
  async fn send_route(&self, mut req: Request) -> Result<Response> {
    // Only the environment that admits NiddService callbacks may send, or a second one could
    // push shadows to a device whose reports go elsewhere.
    if !crate::helpers::thingspace_callbacks_configured(&self.env) {
      return reason_answer(503, Unavailable::NotConfigured.reason());
    }
    let Some(creds) = Credentials::from_env(&self.env) else {
      return reason_answer(503, Unavailable::NotConfigured.reason());
    };
    let Ok(request) = req.json::<SendRequest>().await else {
      return Response::error("Bad Request", 400);
    };
    let message = NiddMessage {
      account_name: creds.account_name.clone(),
      device_ids: vec![DeviceID {
        id: request.imei.clone(),
        kind: "IMEI".to_string(),
      }],
      maximum_delivery_time: request.max_delivery_secs,
      message: request.frame_b64.clone(),
    };

    let mut retried = false;
    loop {
      let tokens = match self.ensure_tokens(&creds, &request.pigeon_id).await {
        Ok(tokens) => tokens,
        Err(why) => return reason_answer(503, why.reason()),
      };

      let started = Date::now().as_millis();
      let call = async {
        let mut response = send_nidd(&tokens.access, &tokens.session, &message)
          .await
          .map_err(|e| CallFailure::from_sdk(&e))?;
        let status = response.status_code();
        response
          .json::<NiddRequest>()
          .await
          .map_err(|_| CallFailure::NoToken(status))
      };
      let result = within(CALL_TIMEOUT, call)
        .await
        .unwrap_or(Err(CallFailure::Timeout));
      log_call(
        "/devices/nidd/message",
        &result,
        started,
        &request.pigeon_id,
      );

      let failure = match result {
        Ok(sent) => {
          self.touch_session(now_secs()).await;
          return answer(
            200,
            &SendAnswer {
              request_id: Some(sent.request_id),
              reason: None,
            },
          );
        }
        Err(failure) => failure,
      };

      let verdict = match &failure {
        CallFailure::Api { status, code } => classify_send(*status, code.as_deref()),
        CallFailure::PreSend => SendVerdict::Refused,
        _ => SendVerdict::Unreachable,
      };
      match verdict {
        SendVerdict::Retry {
          drop_access,
          drop_session,
        } if !retried => {
          self.drop_tokens(drop_access, drop_session).await;
          retried = true;
        }
        SendVerdict::Retry { .. } | SendVerdict::Unreachable => {
          return reason_answer(503, Unavailable::Unreachable.reason());
        }
        SendVerdict::Refused => {
          return reason_answer(502, failure.code().unwrap_or("refused"));
        }
      }
    }
  }

  /// Fresh tokens, logging in only when a cached one is stale, and never while latched.
  async fn ensure_tokens(
    &self,
    creds: &Credentials,
    pigeon_id: &str,
  ) -> std::result::Result<Tokens, Unavailable> {
    let now = now_secs();
    let tokens = self.read_tokens().await;
    if tokens.access_fresh(now) && tokens.session_fresh(now) {
      return Ok(tokens);
    }

    // A request that waited here usually finds another has just logged in, hence the re-read.
    let _flight = self.login.lock().await;
    let mut tokens = self.read_tokens().await;
    if tokens.access_fresh(now) && tokens.session_fresh(now) {
      return Ok(tokens);
    }

    let failures = self.read_failures(creds).await;
    if failures.as_ref().is_some_and(LoginFailures::latched) {
      return Err(Unavailable::Latched);
    }

    if !tokens.access_fresh(now) {
      match self.fetch_access(creds, pigeon_id).await {
        Ok((access, expires_in)) => {
          tokens.access = access;
          tokens.access_expires_at = now + expires_in;
        }
        Err(failure) => {
          let verdict = classify_login(LoginStep::OAuth, &failure, false);
          return Err(
            self
              .record_failure(creds, failures, verdict, &failure)
              .await,
          );
        }
      }
    }

    if !tokens.session_fresh(now) {
      let mut retried = false;
      loop {
        match self.fetch_session(creds, &tokens.access, pigeon_id).await {
          Ok(session) => {
            tokens.session = session;
            tokens.session_used_at = now;
            if let Err(e) = self.state.storage().delete(LOGIN_FAILURES_KEY).await {
              console_error!("thingspace_login: clearing the failure count failed: {e}");
            }
            break;
          }
          Err(failure) => match classify_login(LoginStep::Session, &failure, retried) {
            LoginVerdict::RetryWithFreshAccess => {
              retried = true;
              match self.fetch_access(creds, pigeon_id).await {
                Ok((access, expires_in)) => {
                  tokens.access = access;
                  tokens.access_expires_at = now + expires_in;
                }
                Err(failure) => {
                  let verdict = classify_login(LoginStep::OAuth, &failure, true);
                  return Err(
                    self
                      .record_failure(creds, failures, verdict, &failure)
                      .await,
                  );
                }
              }
            }
            verdict => {
              return Err(
                self
                  .record_failure(creds, failures, verdict, &failure)
                  .await,
              );
            }
          },
        }
      }
    }

    self.write_tokens(&tokens).await;
    Ok(tokens)
  }

  /// OAuth: the access token and its lifetime in seconds.
  async fn fetch_access(
    &self,
    creds: &Credentials,
    pigeon_id: &str,
  ) -> std::result::Result<(String, i64), CallFailure> {
    let started = Date::now().as_millis();
    let call = async {
      let mut response = get_access_token(&creds.public_key, &creds.private_key)
        .await
        .map_err(|e| CallFailure::from_sdk(&e))?;
      let status = response.status_code();
      match response.json::<LoginResponse>().await {
        Ok(login) if !login.access_token.is_empty() => {
          Ok((login.access_token, i64::from(login.expires_in)))
        }
        _ => Err(CallFailure::NoToken(status)),
      }
    };
    let result = within(CALL_TIMEOUT, call)
      .await
      .unwrap_or(Err(CallFailure::Timeout));
    log_call("/oauth2/token", &result, started, pigeon_id);
    result
  }

  /// The UWS session login: the session token.
  async fn fetch_session(
    &self,
    creds: &Credentials,
    access: &str,
    pigeon_id: &str,
  ) -> std::result::Result<String, CallFailure> {
    let body = SessionRequestBody {
      username: creds.uws_username.clone(),
      password: creds.uws_password.clone(),
    };
    let started = Date::now().as_millis();
    let call = async {
      let mut response = get_session_token(&body, access)
        .await
        .map_err(|e| CallFailure::from_sdk(&e))?;
      let status = response.status_code();
      match response.json::<Session>().await {
        Ok(session) if !session.session_token.is_empty() => Ok(session.session_token),
        _ => Err(CallFailure::NoToken(status)),
      }
    };
    let result = within(CALL_TIMEOUT, call)
      .await
      .unwrap_or(Err(CallFailure::Timeout));
    log_call("/session/login", &result, started, pigeon_id);
    result
  }

  /// Counts a failed login and answers whether the object is now latched. Latching logs once
  /// and sends one ops email.
  async fn record_failure(
    &self,
    creds: &Credentials,
    existing: Option<LoginFailures>,
    verdict: LoginVerdict,
    failure: &CallFailure,
  ) -> Unavailable {
    let Some(count) = count_after_failure(existing.as_ref().map(|f| f.count), &verdict) else {
      return Unavailable::Unreachable;
    };

    let mut failures = existing.unwrap_or_else(|| {
      let mut salt = [0u8; 16];
      // A zero salt still counts the failure, which is what matters here.
      let _ = getrandom::getrandom(&mut salt);
      LoginFailures {
        salt: hex(&salt),
        fingerprint: fingerprint(&salt, creds),
        count: 0,
      }
    });
    failures.count = count;
    if let Err(e) = self
      .state
      .storage()
      .put(LOGIN_FAILURES_KEY, &failures)
      .await
    {
      console_error!("thingspace_login: recording the failure count failed: {e}");
    }

    if !failures.latched() {
      return Unavailable::Unreachable;
    }

    let status = failure
      .status()
      .map_or_else(|| "none".to_string(), |s| s.to_string());
    console_error!("thingspace_login outcome=latched status={status}");
    let mut text =
      String::with_capacity(LATCH_EMAIL_HEAD.len() + status.len() + LATCH_EMAIL_TAIL.len());
    text.push_str(LATCH_EMAIL_HEAD);
    text.push_str(&status);
    text.push_str(LATCH_EMAIL_TAIL);
    crate::helpers::send_ops_email(&self.env, "ThingSpace login latched", &text).await;
    Unavailable::Latched
  }

  /// The stored failure count, when it was recorded against these exact credentials and epoch.
  async fn read_failures(&self, creds: &Credentials) -> Option<LoginFailures> {
    let stored = match self
      .state
      .storage()
      .get::<LoginFailures>(LOGIN_FAILURES_KEY)
      .await
    {
      Ok(stored) => stored?,
      Err(e) => {
        console_error!("thingspace_login: reading the failure count failed: {e}");
        return None;
      }
    };
    held_failures(stored, creds)
  }

  /// The cached tokens; empty ones when none are stored or the read fails.
  async fn read_tokens(&self) -> Tokens {
    match self.state.storage().get::<Tokens>(TOKENS_KEY).await {
      Ok(tokens) => tokens.unwrap_or_default(),
      Err(e) => {
        console_error!("thingspace: reading the cached tokens failed: {e}");
        Tokens::default()
      }
    }
  }

  /// Caches the tokens; a failed write only costs a login later.
  async fn write_tokens(&self, tokens: &Tokens) {
    if let Err(e) = self.state.storage().put(TOKENS_KEY, tokens).await {
      console_error!("thingspace: caching the tokens failed: {e}");
    }
  }

  /// Marks the session used now, which is what keeps it from idling out.
  async fn touch_session(&self, now: i64) {
    let mut tokens = self.read_tokens().await;
    tokens.session_used_at = now;
    self.write_tokens(&tokens).await;
  }

  /// Forgets a token a send found stale, so the next `ensure_tokens` replaces it.
  async fn drop_tokens(&self, drop_access: bool, drop_session: bool) {
    let mut tokens = self.read_tokens().await;
    if drop_access {
      tokens.access.clear();
      tokens.access_expires_at = 0;
    }
    if drop_session {
      tokens.session.clear();
      tokens.session_used_at = 0;
    }
    self.write_tokens(&tokens).await;
  }
}

/// A stored salt's bytes.
fn salt_bytes(hex_salt: &str) -> Option<Vec<u8>> {
  let digits = hex_salt.as_bytes();
  if digits.len() % 2 != 0 {
    return None;
  }
  digits
    .chunks_exact(2)
    .map(|pair| {
      let text = std::str::from_utf8(pair).ok()?;
      u8::from_str_radix(text, 16).ok()
    })
    .collect()
}

/// One log line per ThingSpace call: path, status, Verizon's code, latency and pigeon. Never a
/// token, a body, a header value, a frame or an IMEI.
fn log_call<T>(
  path: &str,
  result: &std::result::Result<T, CallFailure>,
  started_ms: u64,
  pigeon_id: &str,
) {
  let ms = Date::now().as_millis().saturating_sub(started_ms);
  match result {
    Ok(_) => console_log!("thingspace path={path} status=200 ms={ms} pigeon={pigeon_id}"),
    Err(failure) => console_error!(
      "thingspace path={path} status={} code={} failure={} ms={ms} pigeon={pigeon_id}",
      failure.status().unwrap_or(0),
      failure.code().unwrap_or("none"),
      failure.kind(),
    ),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn creds() -> Credentials {
    Credentials {
      public_key: "public".to_string(),
      private_key: "private".to_string(),
      uws_username: "user".to_string(),
      uws_password: "password".to_string(),
      account_name: "0000000000-00001".to_string(),
      login_epoch: "1".to_string(),
    }
  }

  fn api(status: u16, code: Option<&str>) -> CallFailure {
    CallFailure::Api {
      status,
      code: code.map(str::to_string),
    }
  }

  #[test]
  fn the_fingerprint_follows_every_secret_and_the_epoch() {
    let salt = [7u8; 16];
    let base = fingerprint(&salt, &creds());
    assert_eq!(base, fingerprint(&salt, &creds()));
    assert_eq!(base.len(), 64);

    let changed: [fn(&mut Credentials); 5] = [
      |c| c.public_key.push('x'),
      |c| c.private_key.push('x'),
      |c| c.uws_username.push('x'),
      |c| c.uws_password.push('x'),
      |c| c.login_epoch = "2".to_string(),
    ];
    for change in changed {
      let mut other = creds();
      change(&mut other);
      assert_ne!(fingerprint(&salt, &other), base);
    }

    // The account name is not a login credential.
    let mut other = creds();
    other.account_name = "1111111111-00001".to_string();
    assert_eq!(fingerprint(&salt, &other), base);

    // Nor does a field boundary move without changing the digest.
    let mut shifted = creds();
    shifted.public_key = "publicp".to_string();
    shifted.private_key = "rivate".to_string();
    assert_ne!(fingerprint(&salt, &shifted), base);

    assert_ne!(fingerprint(&[8u8; 16], &creds()), base);
  }

  #[test]
  fn a_salt_round_trips_through_hex() {
    let salt = [0u8, 1, 0xab, 0xff];
    assert_eq!(salt_bytes(&hex(&salt)), Some(salt.to_vec()));
    assert_eq!(salt_bytes("abc"), None);
    assert_eq!(salt_bytes("zz"), None);
  }

  #[test]
  fn a_credential_refusal_latches_at_once() {
    let m2m = api(
      400,
      Some("UnifiedWebService.INPUT_INVALID.Password.Invalid"),
    );
    assert_eq!(
      classify_login(LoginStep::Session, &m2m, false),
      LoginVerdict::Latch
    );
    assert_eq!(
      classify_login(LoginStep::Session, &m2m, true),
      LoginVerdict::Latch
    );
    for status in [400, 401] {
      let oauth = api(status, Some("invalid_client"));
      assert_eq!(
        classify_login(LoginStep::OAuth, &oauth, false),
        LoginVerdict::Latch
      );
    }
  }

  #[test]
  fn a_bearer_fault_on_login_is_retried_once() {
    for code in ["900901", "900902"] {
      let fault = api(401, Some(code));
      assert_eq!(
        classify_login(LoginStep::Session, &fault, false),
        LoginVerdict::RetryWithFreshAccess
      );
      assert_eq!(
        classify_login(LoginStep::Session, &fault, true),
        LoginVerdict::Count
      );
    }
  }

  #[test]
  fn every_other_login_failure_counts() {
    for failure in [
      CallFailure::Network,
      CallFailure::Timeout,
      CallFailure::NoToken(200),
      api(429, None),
      api(500, None),
      api(503, Some("900800")),
      api(401, None),
      api(403, Some("900908")),
    ] {
      assert_eq!(
        classify_login(LoginStep::Session, &failure, false),
        LoginVerdict::Count,
        "{failure:?}"
      );
    }
    assert_eq!(
      classify_login(LoginStep::OAuth, &api(429, None), false),
      LoginVerdict::Count
    );
    assert_eq!(
      classify_login(LoginStep::OAuth, &CallFailure::Timeout, false),
      LoginVerdict::Count
    );
    assert_eq!(
      classify_login(LoginStep::Session, &CallFailure::PreSend, false),
      LoginVerdict::Exempt
    );
  }

  /// A count as `record_failure` stores it for these credentials.
  fn stored(creds: &Credentials, count: u32) -> LoginFailures {
    let salt = [7u8; 16];
    LoginFailures {
      salt: hex(&salt),
      fingerprint: fingerprint(&salt, creds),
      count,
    }
  }

  #[test]
  fn the_second_counted_failure_in_a_row_latches() {
    // No count held: the first failure, or the first after a success deleted the count.
    let first = count_after_failure(None, &LoginVerdict::Count);
    assert_eq!(first, Some(1));
    assert!(!stored(&creds(), 1).latched());
    let second = count_after_failure(first, &LoginVerdict::Count);
    assert_eq!(second, Some(LOGIN_FAILURE_LIMIT));
    assert!(stored(&creds(), LOGIN_FAILURE_LIMIT).latched());
  }

  #[test]
  fn a_credential_refusal_latches_whatever_the_count() {
    for held in [None, Some(0), Some(1)] {
      assert_eq!(
        count_after_failure(held, &LoginVerdict::Latch),
        Some(LOGIN_FAILURE_LIMIT)
      );
    }
  }

  #[test]
  fn an_exempt_or_retried_login_is_not_counted() {
    for held in [None, Some(1)] {
      assert_eq!(count_after_failure(held, &LoginVerdict::Exempt), None);
      assert_eq!(
        count_after_failure(held, &LoginVerdict::RetryWithFreshAccess),
        None
      );
    }
  }

  #[test]
  fn a_count_holds_only_for_the_credentials_it_was_recorded_against() {
    let latched = || stored(&creds(), LOGIN_FAILURE_LIMIT);
    assert!(held_failures(latched(), &creds()).is_some_and(|held| held.latched()));

    let mut rotated = creds();
    rotated.uws_password.push('x');
    assert!(held_failures(latched(), &rotated).is_none());
    let mut bumped = creds();
    bumped.login_epoch = "2".to_string();
    assert!(held_failures(latched(), &bumped).is_none());

    // A salt that does not parse holds nothing.
    let mut corrupt = latched();
    corrupt.salt = "zz".to_string();
    assert!(held_failures(corrupt, &creds()).is_none());
  }

  #[test]
  fn a_send_drops_the_session_on_every_session_token_code() {
    for code in [
      "UnifiedWebService.REQUEST_FAILED.SessionToken.Expired",
      "UnifiedWebService.REQUEST_FAILED.SessionToken.Format",
      "UnifiedWebService.INPUT_INVALID.SessionToken.Invalid",
    ] {
      assert_eq!(
        classify_send(400, Some(code)),
        SendVerdict::Retry {
          drop_access: false,
          drop_session: true
        }
      );
    }
    assert_eq!(
      classify_send(401, Some("900901")),
      SendVerdict::Retry {
        drop_access: true,
        drop_session: false
      }
    );
  }

  #[test]
  fn a_send_is_unreachable_on_408_429_and_5xx_and_refused_otherwise() {
    for status in [408, 429, 500, 502, 503, 504] {
      assert_eq!(classify_send(status, None), SendVerdict::Unreachable);
    }
    assert_eq!(
      classify_send(400, Some("INPUT_INVALID.Message.Null")),
      SendVerdict::Refused
    );
    assert_eq!(classify_send(404, None), SendVerdict::Refused);
    assert_eq!(classify_send(401, None), SendVerdict::Refused);
  }

  #[test]
  fn tokens_are_fresh_inside_their_margins() {
    let tokens = Tokens {
      access: "a".to_string(),
      access_expires_at: 10_000,
      session: "s".to_string(),
      session_used_at: 5_000,
    };
    assert!(tokens.access_fresh(9_699));
    assert!(!tokens.access_fresh(9_700));
    assert!(tokens.session_fresh(5_899));
    assert!(!tokens.session_fresh(5_900));
    assert!(!Tokens::default().access_fresh(0));
    assert!(!Tokens::default().session_fresh(0));
  }
}
