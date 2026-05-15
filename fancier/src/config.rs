// Ory Kratos public endpoint for browser flows
pub const KRATOS_BROWSER_URL: &str = match option_env!("KRATOS_BROWSER_URL") {
  Some(url) => url,
  None => "http://127.0.0.1:4433", // A safe local fallback
};

// Path for dashboard (dovecote), a seperate app on the same domain
pub const ROOT_URL: &str = match option_env!("ROOT_URL") {
  Some(url) => url,
  None => "http://127.0.0.1:4455",
};

// Cookie name for cookie defining session state
pub const SESSION_COOKIE_NAME: &str = match option_env!("SESSION_COOKIE_NAME") {
  Some(name) => name,
  None => "session_expiry",
};

pub const API_HOST: &str = match option_env!("API_HOST") {
  Some(name) => name,
  None => "http://127.0.0.1:8787",
};
