// dovecote's public contact route (docs/api.md "Contact") -- one
// write-only call with no LocalSession cache involvement, same shape as
// `api/feedback.rs`. Uses `fetch_json_any_status` because the form's error
// copy is the server's own rejection sentence, which only a non-2xx
// response body carries.
use crate::api::fetch_json_any_status;
use capsules::ContactRequest;
use dioxus::logger::tracing::error;
use wasm_bindgen_futures::JsFuture;

/// `POST /contact`. `Err` carries the message the form renders verbatim.
///
/// For a `400`/`413` the message is dovecote's own response text, which is
/// `capsules::ContactRejection::message` -- the same sentence this client
/// would have produced from a local `validate()`, so a rule enforced only
/// server-side still reads as a normal field error rather than a generic
/// failure.
pub async fn send(req: &ContactRequest) -> Result<(), String> {
  let Ok(body) = serde_json::to_string(req) else {
    return Err("Could not encode your message. Please try again.".to_string());
  };
  let Ok(body) = serde_wasm_bindgen::to_value(&body) else {
    return Err("Could not encode your message. Please try again.".to_string());
  };

  match fetch_json_any_status("POST", "/contact", Some(&body)).await {
    None => Err("Could not reach the server. Check your connection and try again.".to_string()),
    Some(resp) if resp.ok() => Ok(()),
    Some(resp) => {
      let status = resp.status();
      error!("POST /contact failed with status: {status}");
      let detail = match resp.text().ok() {
        Some(promise) => JsFuture::from(promise)
          .await
          .ok()
          .and_then(|v| v.as_string())
          .map(|t| t.trim().to_string())
          .filter(|t| !t.is_empty()),
        None => None,
      };
      Err(match (status, detail) {
        (429, _) => {
          "That is a few messages in quick succession. Please wait a minute and try again."
            .to_string()
        }
        (400 | 413, Some(detail)) => detail,
        (400 | 413, None) => {
          "The server rejected the message. Please check the form and try again.".to_string()
        }
        _ => format!(
          "Something went wrong on our end (HTTP {status}). Please email us directly and we will pick it up."
        ),
      })
    }
  }
}
