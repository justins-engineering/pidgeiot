// What the one-time reveal has to say about each value it shows. A bare
// box of base64 tells an operator nothing about which of a device's three
// build-time strings it is, and the connectors do not agree on which ones
// a build even needs, so the mapping is per connector and names the
// `~/pigeon` Kconfig symbol each value is destined for.
use capsules::Connector;

/// One row of the reveal: a value, what it is, and where it goes in a
/// device build.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceCredential {
  pub label: &'static str,
  pub value: String,
  /// The Kconfig symbol this value is set through, when there is one.
  /// `None` for the MQTT PSK identity: the device library reads it from
  /// `pigeon_config.device_id` and deliberately has no symbol of its own,
  /// since a second place to write the same string could only disagree
  /// with the first.
  pub target: Option<&'static str>,
  pub note: &'static str,
}

/// Whether this connector carries a write-once secret beyond the token,
/// which is what decides how the reveal warns about retrieval.
pub fn has_psk(connector: &Connector) -> bool {
  connector.psk().is_some()
}

/// Everything a device needs from a create or a token refresh, in the
/// order an operator fills a build in: the credential first, then how the
/// handshake names the device, then the address.
pub fn device_credentials(connector: &Connector) -> Vec<DeviceCredential> {
  let mut fields = Vec::with_capacity(4);

  fields.push(DeviceCredential {
    label: "Device token",
    value: connector.token().to_string(),
    target: Some("CONFIG_PIGEON_TOKEN"),
    note: match connector {
      Connector::Https(_) => {
        "Bearer credential on every device HTTPS request and on the device WebSocket. Store it with no \"Bearer \" prefix; the library adds one."
      }
      Connector::Coap(_) => {
        "A CoAP session is authenticated by the PSK below, so a device build may leave this empty. A build that also fetches firmware over HTTPS still needs it."
      }
      Connector::Mqtt(_) => {
        "The CONNECT password on a certificate session (CONFIG_PIGEON_MQTT_AUTH_CERT). A PSK session may leave it empty, unless the build also fetches firmware over HTTPS."
      }
    },
  });

  if let Some((identity, secret)) = connector.psk() {
    let coap = matches!(connector, Connector::Coap(_));
    fields.push(DeviceCredential {
      label: "TLS PSK identity",
      value: identity.to_string(),
      target: coap.then_some("CONFIG_PIGEON_COAP_TLS_PSK_IDENTITY"),
      note: if coap {
        "Names this pigeon during the DTLS or TLS handshake. It is the pigeon's own id."
      } else {
        "Names this pigeon during the TLS handshake, and is also its CONNECT username and client id. The library takes it from pigeon_config.device_id, so there is no symbol to set."
      },
    });
    fields.push(DeviceCredential {
      label: "TLS PSK secret",
      value: secret.to_string(),
      target: Some(if coap {
        "CONFIG_PIGEON_COAP_TLS_PSK_SECRET"
      } else {
        "CONFIG_PIGEON_MQTT_TLS_PSK_SECRET"
      }),
      note: "The short key that handshake uses, not the device token. Refreshing the token mints a new one and retires this.",
    });
  }

  fields.push(DeviceCredential {
    label: match connector {
      Connector::Mqtt(_) => "Broker endpoint",
      _ => "Device endpoint",
    },
    value: connector.endpoint().to_string(),
    target: Some("CONFIG_PIGEON_ENDPOINT"),
    note: match connector {
      Connector::Https(_) => "This pigeon's own base URL. Every device route hangs off it.",
      Connector::Coap(_) => {
        "The CoAP terminator this pigeon dials. The scheme has to match the transport the firmware was built for: coaps:// over UDP, coaps+tcp:// over TCP."
      }
      Connector::Mqtt(_) => {
        "The broker this pigeon dials. It carries no path, because MQTT names resources with topics rather than URLs."
      }
    },
  });

  fields
}

#[cfg(test)]
mod tests {
  use super::*;
  use capsules::{CoapConfig, HttpsConfig, MqttConfig};

  fn labels(connector: &Connector) -> Vec<&'static str> {
    device_credentials(connector)
      .into_iter()
      .map(|f| f.label)
      .collect()
  }

  fn targets(connector: &Connector) -> Vec<Option<&'static str>> {
    device_credentials(connector)
      .into_iter()
      .map(|f| f.target)
      .collect()
  }

  fn https() -> Connector {
    Connector::Https(HttpsConfig {
      endpoint: "https://api.pidgeiot.com/device/pigeons/abc".to_string(),
      token: "tok".to_string(),
    })
  }

  fn coap() -> Connector {
    Connector::Coap(CoapConfig {
      endpoint: "coaps://coap.pidgeiot.com:5684/device/pigeons/abc".to_string(),
      token: "tok".to_string(),
      tls_psk_identity: Some("abc".to_string()),
      tls_psk_secret: Some("hex".to_string()),
    })
  }

  fn mqtt() -> Connector {
    Connector::Mqtt(MqttConfig {
      endpoint: "mqtts://mqtt.pidgeiot.com:8883".to_string(),
      token: "tok".to_string(),
      tls_psk_identity: Some("abc".to_string()),
      tls_psk_secret: Some("hex".to_string()),
    })
  }

  #[test]
  fn an_https_pigeon_reveals_a_token_and_an_endpoint() {
    assert_eq!(labels(&https()), vec!["Device token", "Device endpoint"]);
    assert_eq!(
      targets(&https()),
      vec![Some("CONFIG_PIGEON_TOKEN"), Some("CONFIG_PIGEON_ENDPOINT")]
    );
    assert!(!has_psk(&https()));
  }

  #[test]
  fn a_coap_pigeon_reveals_both_psk_halves_under_the_coap_symbols() {
    assert_eq!(
      labels(&coap()),
      vec![
        "Device token",
        "TLS PSK identity",
        "TLS PSK secret",
        "Device endpoint"
      ]
    );
    assert_eq!(
      targets(&coap()),
      vec![
        Some("CONFIG_PIGEON_TOKEN"),
        Some("CONFIG_PIGEON_COAP_TLS_PSK_IDENTITY"),
        Some("CONFIG_PIGEON_COAP_TLS_PSK_SECRET"),
        Some("CONFIG_PIGEON_ENDPOINT")
      ]
    );
    assert!(has_psk(&coap()));
  }

  #[test]
  fn an_mqtt_pigeon_names_its_broker_and_has_no_identity_symbol() {
    assert_eq!(
      labels(&mqtt()),
      vec![
        "Device token",
        "TLS PSK identity",
        "TLS PSK secret",
        "Broker endpoint"
      ]
    );
    assert_eq!(
      targets(&mqtt()),
      vec![
        Some("CONFIG_PIGEON_TOKEN"),
        None,
        Some("CONFIG_PIGEON_MQTT_TLS_PSK_SECRET"),
        Some("CONFIG_PIGEON_ENDPOINT")
      ]
    );
  }

  #[test]
  fn every_revealed_value_comes_from_the_connector() {
    let fields = device_credentials(&mqtt());
    assert_eq!(fields[0].value, "tok");
    assert_eq!(fields[1].value, "abc");
    assert_eq!(fields[2].value, "hex");
    assert_eq!(fields[3].value, "mqtts://mqtt.pidgeiot.com:8883");
  }

  #[test]
  fn a_connector_read_back_with_its_secrets_stripped_reveals_no_psk_rows() {
    let stripped = Connector::Mqtt(MqttConfig {
      endpoint: "mqtts://mqtt.pidgeiot.com:8883".to_string(),
      token: String::new(),
      tls_psk_identity: Some("abc".to_string()),
      tls_psk_secret: None,
    });
    assert_eq!(labels(&stripped), vec!["Device token", "Broker endpoint"]);
    assert!(!has_psk(&stripped));
  }
}
