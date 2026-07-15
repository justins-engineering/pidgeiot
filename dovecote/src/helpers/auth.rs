use ory_kratos_client_wasm::apis::{configuration::Configuration, frontend_api::to_session};
use worker::{Env, Request, console_debug};

pub async fn authenticate_browser(
  req: &Request,
  env: &Env,
) -> worker::Result<ory_kratos_client_wasm::models::Session> {
  let cookie_header = req.headers().get("Cookie")?;

  match cookie_header {
    None => {
      console_debug!("Request missing Cookie Header");
      Err("Unauthorized".into())
    }
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
        Err(e) => {
          console_debug!("Error: {e:?}");
        }
      }

      Err("Unauthorized".into())
    }
  }
}
