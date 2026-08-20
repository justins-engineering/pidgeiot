// Ory Kratos public endpoint for browser flows
pub const KRATOS_BROWSER_URL: &str = match option_env!("KRATOS_BROWSER_URL") {
  Some(url) => url,
  None => "http://localhost:4433", // A safe local fallback
};

// Cookie name for cookie defining session state
pub const SESSION_COOKIE_NAME: &str = match option_env!("SESSION_COOKIE_NAME") {
  Some(name) => name,
  None => "session_expiry",
};

pub const API_HOST: &str = match option_env!("API_HOST") {
  Some(name) => name,
  None => "http://localhost:8787",
};

// Pigeon id backing the public /demo page (views/demo.rs, api/demo.rs) --
// must match whatever dovecote's own DEMO_PIGEON_IDS allowlists for this
// environment (dovecote/wrangler.toml). Empty means "no demo pigeon here"
// (dev's default -- dovecote's dev DEMO_PIGEON_IDS is empty too), which
// api::demo short-circuits on rather than firing a request that would just
// 404.
pub const DEMO_PIGEON_ID: &str = match option_env!("DEMO_PIGEON_ID") {
  Some(id) => id,
  None => "",
};
