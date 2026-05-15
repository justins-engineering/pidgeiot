use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn set_lang() {
  let window = web_sys::window().expect("Could not access window");
  let document = window.document().expect("Could not access window document");
  // let navigator = window.navigator();
  let document_element = document
    .document_element()
    .expect("Expecting an element on document");

  let data_lang = "en".to_string();
  // let mut data_lang = "en".to_string();
  // let browser_lang = web_sys::Navigator::language(&navigator);
  // if let Some(lang) = browser_lang {
  //   data_lang = lang;
  // }

  // debug!("lang set to {data_lang}");

  document_element
    .set_attribute("lang", &data_lang.to_owned())
    .expect("Failed to set data-theme");
}
