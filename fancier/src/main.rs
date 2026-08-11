use dioxus::prelude::*;

// `server_only!` compiles to nothing on the `web`/wasm build, so this only
// runs for the native "server" target that drives SSG prerendering.
fn main() {
  dioxus::LaunchBuilder::new()
    .with_cfg(server_only! {
      ServeConfig::builder().incremental(
        dioxus_server::IncrementalRendererConfig::new()
          // Write prerendered route HTML next to the wasm/js/assets output
          // (the executable's sibling `public/` dir) so it lands in the same
          // directory wrangler serves as static assets -- no separate copy
          // step needed for the prerendered files themselves.
          .static_dir(
            std::env::current_exe()
              .unwrap()
              .parent()
              .unwrap()
              .join("public"),
          )
          .clear_cache(false),
      )
    })
    .launch(fancier::App);
}
