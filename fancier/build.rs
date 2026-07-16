use dotenvy::from_path_iter;
use std::env;
use std::path::Path;

fn main() {
  // 1. Determine which .env file to bake in. Cargo's own PROFILE is only ever
  // "debug" or "release" (even for custom profiles), so it can't distinguish
  // a staging release build from a production one. FANCIER_ENV, when set,
  // overrides that profile-based pick — e.g. `FANCIER_ENV=staging` picks
  // .env.staging instead of .env.release for an otherwise-identical release
  // build. Unset (the normal `dx serve`/`dx build --release` invocations),
  // behavior is unchanged: debug -> .env.dev, release -> .env.release, so
  // this is safe to land ahead of the staging deploy itself actually landing
  // (currently paused — see .env.staging and worker/access-gate.mjs) since
  // it never changes without FANCIER_ENV being set by something new.
  let profile = env::var("PROFILE").expect("PROFILE should be set by Cargo");
  let env_filename = match env::var("FANCIER_ENV") {
    Ok(name) if !name.is_empty() => format!(".env.{}", name),
    _ => format!(".env.{}", profile),
  };
  let env_path = Path::new(&env_filename);

  // 2. Tell Cargo to re-run this script if the specific .env file changes,
  // or if FANCIER_ENV itself changes (e.g. switching a cached release build
  // between a plain release and a staging one without touching any .env file).
  println!("cargo:rerun-if-changed={}", env_filename);
  println!("cargo:rerun-if-changed=build.rs");
  println!("cargo:rerun-if-env-changed=FANCIER_ENV");

  // 3. Read the file and export the variables
  if env_path.exists() {
    let iter = from_path_iter(env_path).expect("Failed to parse env file");

    for item in iter {
      let (key, val) = item.expect("Invalid key-value pair in env file");

      // This instructs Cargo to set the variable at compile time for your Dioxus code
      println!("cargo:rustc-env={}={}", key, val);
    }
  } else {
    // Emits a warning in the terminal if the file is missing
    println!("cargo:warning=Environment file {} not found.", env_filename);
  }
}
