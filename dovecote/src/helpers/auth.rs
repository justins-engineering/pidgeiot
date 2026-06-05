use base64::{Engine as _, engine::general_purpose::STANDARD};
use jwt_simple::prelude::*;
use ory_kratos_client_wasm::apis::{configuration::Configuration, frontend_api::to_session};
use std::collections::HashSet;
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

fn load_public_key(env: &Env) -> worker::Result<Ed25519PublicKey> {
  let der_b64 = env.secret("DEVICE_PUBLIC_KEY")?.to_string();
  let der = STANDARD
    .decode(der_b64)
    .map_err(|e| worker::Error::RustError(format!("Base64 decode error: {e}")))?;

  // Parse SubjectPublicKeyInfo DER
  Ed25519PublicKey::from_der(&der)
    .map_err(|e| worker::Error::RustError(format!("Public key parse error: {e}")))
}

pub fn require_device_auth(req: &Request, env: &Env, pigeon_id: &str) -> worker::Result<()> {
  let auth_header = req.headers().get("Authorization")?;

  let auth_header = auth_header
    .ok_or_else(|| worker::Error::RustError("Unauthorized: Missing Authorization header".into()))?;

  let token = auth_header
    .strip_prefix("Bearer ")
    .ok_or_else(|| worker::Error::RustError("Unauthorized: Missing Bearer token".into()))?;

  let pubkey = load_public_key(env)?;

  let options = VerificationOptions {
    allowed_audiences: Some(HashSet::from([pigeon_id.to_string()])),
    ..Default::default()
  };

  pubkey
    .verify_token::<NoCustomClaims>(token, Some(options))
    .map_err(|e| worker::Error::RustError(format!("Unauthorized: Invalid token ({e})")))?;

  Ok(())
}
