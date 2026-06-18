use crate::components::ory_error::{ErrorContentJs, ErrorContentRsx};
use dioxus::prelude::*;
use ory_kratos_client_wasm::apis::ResponseContent;
use ory_kratos_client_wasm::apis::frontend_api::{
  CreateBrowserLoginFlowError, CreateBrowserLogoutFlowError, CreateBrowserRecoveryFlowError,
  CreateBrowserRegistrationFlowError, CreateBrowserSettingsFlowError,
  CreateBrowserVerificationFlowError, GetLoginFlowError, GetRecoveryFlowError,
  GetRegistrationFlowError, GetSettingsFlowError, GetVerificationFlowError, ToSessionError,
};

pub trait DisplayError {
  fn view_response_content(self) -> Element;
}

impl DisplayError for ResponseContent<CreateBrowserRegistrationFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        CreateBrowserRegistrationFlowError::DefaultResponse(err) => rsx! {
          ErrorContentRsx { err }
        },
        CreateBrowserRegistrationFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<GetRegistrationFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        GetRegistrationFlowError::DefaultResponse(err)
        | GetRegistrationFlowError::Status403(err)
        | GetRegistrationFlowError::Status404(err)
        | GetRegistrationFlowError::Status410(err) => rsx! {
          ErrorContentRsx { err }
        },
        GetRegistrationFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<CreateBrowserLoginFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        CreateBrowserLoginFlowError::DefaultResponse(err)
        | CreateBrowserLoginFlowError::Status400(err) => rsx! {
          ErrorContentRsx { err }
        },
        CreateBrowserLoginFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<GetLoginFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        GetLoginFlowError::DefaultResponse(err)
        | GetLoginFlowError::Status403(err)
        | GetLoginFlowError::Status404(err)
        | GetLoginFlowError::Status410(err) => rsx! {
          ErrorContentRsx { err }
        },
        GetLoginFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<CreateBrowserLogoutFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        CreateBrowserLogoutFlowError::Status400(err)
        | CreateBrowserLogoutFlowError::Status401(err)
        | CreateBrowserLogoutFlowError::Status500(err) => rsx! {
          ErrorContentRsx { err }
        },
        CreateBrowserLogoutFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<ToSessionError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        ToSessionError::DefaultResponse(err)
        | ToSessionError::Status401(err)
        | ToSessionError::Status403(err) => rsx! {
          ErrorContentRsx { err }
        },
        ToSessionError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<CreateBrowserRecoveryFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        CreateBrowserRecoveryFlowError::DefaultResponse(err)
        | CreateBrowserRecoveryFlowError::Status400(err) => rsx! {
          ErrorContentRsx { err }
        },
        CreateBrowserRecoveryFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<GetRecoveryFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        GetRecoveryFlowError::DefaultResponse(err)
        | GetRecoveryFlowError::Status404(err)
        | GetRecoveryFlowError::Status410(err) => rsx! {
          ErrorContentRsx { err }
        },
        GetRecoveryFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<CreateBrowserSettingsFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        CreateBrowserSettingsFlowError::DefaultResponse(err)
        | CreateBrowserSettingsFlowError::Status400(err)
        | CreateBrowserSettingsFlowError::Status401(err)
        | CreateBrowserSettingsFlowError::Status403(err) => rsx! {
          ErrorContentRsx { err }
        },
        CreateBrowserSettingsFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<GetSettingsFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        GetSettingsFlowError::DefaultResponse(err)
        | GetSettingsFlowError::Status401(err)
        | GetSettingsFlowError::Status403(err)
        | GetSettingsFlowError::Status404(err)
        | GetSettingsFlowError::Status410(err) => rsx! {
          ErrorContentRsx { err }
        },
        GetSettingsFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<CreateBrowserVerificationFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        CreateBrowserVerificationFlowError::DefaultResponse(err) => rsx! {
          ErrorContentRsx { err }
        },
        CreateBrowserVerificationFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}

impl DisplayError for ResponseContent<GetVerificationFlowError> {
  fn view_response_content(self) -> Element {
    if let Some(ent) = self.entity {
      match ent {
        GetVerificationFlowError::DefaultResponse(err)
        | GetVerificationFlowError::Status403(err)
        | GetVerificationFlowError::Status404(err) => rsx! {
          ErrorContentRsx { err }
        },
        GetVerificationFlowError::UnknownValue(err) => rsx! {
          ErrorContentJs { err }
        },
      }
    } else {
      rsx! {
        p { "{self.content}" }
      }
    }
  }
}
