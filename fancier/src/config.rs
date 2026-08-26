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

// Cloudflare Turnstile site key for the contact form's widget
// (views/contact.rs). Public by design, and per-environment because a
// widget is bound to the hostname it renders on. The fallback is
// Cloudflare's published always-pass test key, which pairs with the test
// secret dovecote's dev config accepts, so a build with no .env file still
// gets a working form.
pub const TURNSTILE_SITE_KEY: &str = match option_env!("TURNSTILE_SITE_KEY") {
  Some(key) => key,
  None => "1x00000000000000000000AA",
};
