#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AuthState {
  Pending,
  Authenticated,
  Unauthenticated,
}

impl AuthState {
  #[inline]
  pub fn is_authenticated(&self) -> bool {
    matches!(self, AuthState::Authenticated)
  }
}
