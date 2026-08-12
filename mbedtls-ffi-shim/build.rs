use std::path::PathBuf;

fn main() {
  println!("cargo:rerun-if-changed=csrc/glue.c");
  println!("cargo:rerun-if-env-changed=MBEDTLS_INCLUDE_DIR");

  // Fail with a message that names the fix, rather than a wall of cc
  // include errors: this crate links Debian trixie's system mbedTLS 3.6
  // and is normally built inside loft's Docker build stage.
  let include_dir = std::env::var_os("MBEDTLS_INCLUDE_DIR").map(PathBuf::from);
  let probe = include_dir
    .clone()
    .unwrap_or_else(|| PathBuf::from("/usr/include"))
    .join("mbedtls/ssl.h");
  if !probe.exists() {
    panic!(
      "mbedTLS 3.6 development headers not found ({}). Install libmbedtls-dev \
       (Debian trixie) or the distro equivalent (Arch: mbedtls 3.6), or point \
       MBEDTLS_INCLUDE_DIR at the headers. The canonical build environment is \
       loft/Dockerfile's rust:1-trixie stage.",
      probe.display()
    );
  }

  let mut build = cc::Build::new();
  if let Some(dir) = include_dir {
    build.include(dir);
  }
  build
    .file("csrc/glue.c")
    .warnings(true)
    .flag_if_supported("-Werror")
    .compile("mbedtls_glue");

  println!("cargo:rustc-link-lib=dylib=mbedtls");
  println!("cargo:rustc-link-lib=dylib=mbedx509");
  println!("cargo:rustc-link-lib=dylib=mbedcrypto");
}
