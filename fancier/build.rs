use dotenvy::from_path_iter;
use std::env;
use std::path::Path;

fn main() {
  // 1. Determine if we are running `dx serve` (debug) or `dx build --release` (release)
  let profile = env::var("PROFILE").expect("PROFILE should be set by Cargo");
  let env_filename = format!(".env.{}", profile);
  let env_path = Path::new(&env_filename);

  // 2. Tell Cargo to re-run this script if the specific .env file changes
  println!("cargo:rerun-if-changed={}", env_filename);
  println!("cargo:rerun-if-changed=build.rs");

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
