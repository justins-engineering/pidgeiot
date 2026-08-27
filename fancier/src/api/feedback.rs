// dovecote's feedback route (docs/api.md "Feedback" section) -- one
// write-only call, no LocalSession cache involvement (nothing is
// persisted or ever read back). Uses `fetch_json_any_status` rather than
// `fetch_json` for the same reason the shell route does: the modal's
// error copy distinguishes *which* rejection happened (too long vs.
// rejected vs. unreachable), not just "it failed".
use crate::api::fetch_json_any_status;
use capsules::FeedbackRequest;
use dioxus::logger::tracing::error;

/// `POST /feedback`. `Err` carries the user-facing message the modal
/// renders verbatim -- copy lives here, next to the status mapping that
/// selects it.
pub async fn send(req: &FeedbackRequest) -> Result<(), String> {
  let Ok(body) = serde_json::to_string(req) else {
    return Err("Could not encode your feedback. Please try again.".to_string());
  };
  let Ok(body) = serde_wasm_bindgen::to_value(&body) else {
    return Err("Could not encode your feedback. Please try again.".to_string());
  };

  match fetch_json_any_status("POST", "/feedback", Some(&body)).await {
    None => Err("Could not reach the server. Check your connection and try again.".to_string()),
    Some(resp) if resp.ok() => Ok(()),
    Some(resp) => {
      let status = resp.status();
      error!("POST /feedback failed with status: {status}");
      Err(match status {
        413 => "Your message is too long. Please shorten it and try again.".to_string(),
        400 => {
          "The server rejected the submission. Make sure the message isn't empty and try again."
            .to_string()
        }
        _ => format!(
          "Something went wrong on our end (HTTP {status}). Please try again in a few minutes."
        ),
      })
    }
  }
}
