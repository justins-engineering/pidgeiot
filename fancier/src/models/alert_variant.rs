#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AlertVariant {
  Info,
  Success,
  #[default]
  Warning,
  Error,
}

impl AlertVariant {
  pub fn theme_classes(&self) -> &'static str {
    match self {
      AlertVariant::Info => "alert-info not-dark:text-info-content",
      AlertVariant::Success => "alert-success not-dark:text-success-content",
      AlertVariant::Warning => "alert-warning not-dark:text-warning-content",
      AlertVariant::Error => "alert-error not-dark:text-error-content",
    }
  }

  pub fn btn_classes(&self) -> &'static str {
    match self {
      AlertVariant::Info => "btn-info not-dark:text-info-content",
      AlertVariant::Success => "btn-success not-dark:text-success-content",
      AlertVariant::Warning => "btn-warning not-dark:text-warning-content",
      AlertVariant::Error => "btn-error not-dark:text-error-content",
    }
  }
}
