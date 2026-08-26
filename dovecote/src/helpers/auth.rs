use ory_kratos_client_wasm::apis::{Error, configuration::Configuration, frontend_api::to_session};
use worker::{Env, Request, console_debug};

/// Kratos answers whoami with this when the cookie jar carries no session
/// for us.
const UNAUTHORIZED: u16 = 401;

pub async fn authenticate_browser(
  req: &Request,
  env: &Env,
) -> worker::Result<ory_kratos_client_wasm::models::Session> {
  let cookie_header = req.headers().get("Cookie")?;

  match cookie_header {
    // The public routes (contact, feedback, error reports) resolve a
    // session only if one happens to be there, so an anonymous caller is
    // the ordinary case on this path, not a fault. Logging it buries the
    // failures that do mean something under a line per visitor.
    None => Err("Unauthorized".into()),
    Some(ch) => {
      let conf = Configuration {
        base_path: env.var("KRATOS_BROWSER_URL")?.to_string(),
        user_agent: None,
        basic_auth: None,
        oauth_access_token: None,
        bearer_access_token: None,
        api_key: None,
      };

      match to_session(&conf, None, Some(&ch), None).await {
        Ok(session) => {
          if let Some(active) = session.active
            && active
          {
            return Ok(session);
          }
        }
        // Same ordinary case, one layer further in: cookies were sent,
        // none of them a session Kratos still recognises.
        Err(Error::ResponseError(response)) if response.status == UNAUTHORIZED => {}
        // Everything else is real -- Kratos unreachable, answering 5xx, or
        // sending something we cannot parse -- and reads as an outage
        // rather than as a signed-out visitor.
        Err(e) => console_debug!("Kratos session check failed: {e}"),
      }

      Err("Unauthorized".into())
    }
  }
}
