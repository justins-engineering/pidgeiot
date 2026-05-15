use crate::Route;
use crate::components::{Footer, Navbar};
use dioxus::prelude::*;

#[component]
pub fn Wrapper() -> Element {
  rsx! {
    Navbar {}
    main { Outlet::<Route> {} }
    Footer {}
  }
}
