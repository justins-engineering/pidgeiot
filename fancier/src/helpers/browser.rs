macro_rules! window {
  () => {
    web_sys::window().expect("Could not access window")
  };
}

macro_rules! html_document {
  ($window:expr) => {
    web_sys::wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlDocument>(
      $window
        .document()
        .expect("Could not access window document"),
    )
    .expect("Could not access HTMLDocument")
  };
}

macro_rules! get_cookies {
  ($html_document:expr) => {
    $html_document
      .cookie()
      .expect("Could not access HTMLDocument cookies")
  };
}

pub(crate) use get_cookies;
pub(crate) use html_document;
pub(crate) use window;
