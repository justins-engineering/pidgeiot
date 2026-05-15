#![allow(unreachable_code)]
#![allow(unused_imports)]

//! This module implements WebSocket support for Dioxus Fullstack applications.
//!
//! WebSockets provide a full-duplex communication channel over a single, long-lived connection.
//!
//! This makes them ideal for real-time applications where the server and the client need to communicate
//! frequently and with low latency. Unlike Server-Sent Events (SSE), WebSockets allow the direct
//! transport of binary data, enabling things like video and audio streaming as well as more efficient
//! zero-copy serialization formats.
//!
//! This module implements a variety of types:
//! - `Websocket<In, Out, E>`: Represents a WebSocket connection that can send messages of type `In` and receive messages of type `Out`, using the encoding `E`.
//! - `UseWebsocket<In, Out, E>`: A hook that provides a reactive interface to a WebSocket connection.
//! - `WebSocketOptions`: Configuration options for establishing a WebSocket connection.
//! - `TypedWebsocket<In, Out, E>`: A typed wrapper around an Axum WebSocket connection for server-side use.
//! - `WebsocketState`: An enum representing the state of the WebSocket connection.
//! - plus a variety of error types and traits for encoding/decoding messages.
//!
//! Dioxus Fullstack websockets are typed in both directions, letting the happy path (`.send()` and `.recv()`)
//! automatically serialize and deserialize messages for you.

use crate::encoding::{Encoding, JsonEncoding};
use bytes::Bytes;
use dioxus::core::{CapturedError, Result, use_hook};
use dioxus::hooks::{Resource, UseWaker, use_resource};
use dioxus::hooks::{use_signal, use_waker};
use dioxus::logger::tracing::info;
use dioxus::signals::{ReadSignal, ReadableExt, ReadableOptionExt, Signal, WritableExt};
use futures::StreamExt;
use futures::{
  SinkExt, TryFutureExt,
  stream::{SplitSink, SplitStream},
};
use serde::{Serialize, de::DeserializeOwned};
use std::prelude::rust_2024::Future;
use {
  futures_util::lock::Mutex,
  gloo_net::websocket::{Message as WsMessage, futures::WebSocket as WsWebsocket},
};

/// A hook that provides a reactive interface to a WebSocket connection.
///
/// WebSockets provide a full-duplex communication channel over a single, long-lived connection.
///
/// This makes them ideal for real-time applications where the server and the client need to communicate
/// frequently and with low latency. Unlike Server-Sent Events (SSE), WebSockets allow the direct
/// transport of binary data, enabling things like video and audio streaming as well as more efficient
/// zero-copy serialization formats.
///
/// This hook takes a function that returns a future which resolves to a `Websocket<In, Out, E>` -
/// usually a server function.
pub fn use_websocket<
  In: 'static,
  Out: 'static,
  E: Into<CapturedError> + 'static,
  F: Future<Output = Result<Websocket<In, Out, Enc>, E>> + 'static,
  Enc: Encoding,
>(
  mut connect_to_websocket: impl FnMut() -> F + 'static,
) -> UseWebsocket<In, Out, Enc> {
  let mut waker = use_waker();
  let mut status = use_signal(|| WebsocketState::Connecting);
  let status_read = use_hook(|| ReadSignal::new(status));

  let connection = use_resource(move || {
    let connection = connect_to_websocket().map_err(|e| e.into());
    async move {
      let connection = connection.await;

      // Update the status based on the result of the connection attempt
      match connection.as_ref() {
        Ok(_) => status.set(WebsocketState::Open),
        Err(_) => status.set(WebsocketState::FailedToConnect),
      }

      // Wake up the `.recv()` calls waiting for the connection to be established
      waker.wake(());

      connection
    }
  });

  UseWebsocket {
    connection,
    waker,
    status,
    status_read,
  }
}

// pub fn use_websocket<
//   In: 'static,
//   Out: 'static,
//   E: Into<CapturedError> + 'static,
//   F: Future<Output = Result<Websocket<In, Out, Enc>, E>> + 'static,
//   Enc: Encoding,
// >(
//   url: String,
//   websocket_options: WebSocketOptions,
// ) -> UseWebsocket<In, Out, Enc> {
//   let mut waker = use_waker();
//   let mut status = use_signal(|| WebsocketState::Connecting);
//   let status_read: ReadSignal<WebsocketState> = use_hook(|| ReadSignal::new(status));

//   let connection = use_resource(move || {
//     let connection = connect_to_websocket::<In, Out, E, F, Enc>(
//       url.to_owned(),
//       websocket_options.protocols.to_owned(),
//     )
//     .map_err(|e| e.into());
//     async move {
//       let connection = connection.await;

//       // Update the status based on the result of the connection attempt
//       match connection.as_ref() {
//         Ok(_) => status.set(WebsocketState::Open),
//         Err(_) => status.set(WebsocketState::FailedToConnect),
//       }

//       // Wake up the `.recv()` calls waiting for the connection to be established
//       waker.wake(());

//       connection
//     }
//   });

//   UseWebsocket {
//     connection,
//     waker,
//     status,
//     status_read,
//   }
// }

// async fn connect_to_websocket<
//   In: 'static,
//   Out: 'static,
//   E: Into<CapturedError> + 'static,
//   F: Future<Output = Result<Websocket<In, Out, Enc>, E>> + 'static,
//   Enc: Encoding,
// >(
//   url: String,
//   protocols: Vec<String>,
// ) -> Result<Websocket<In, Out, Enc>, E> {
//   let (sender, receiver) =
//     gloo_net::websocket::futures::WebSocket::open_with_protocols(&url, &protocols)
//       .unwrap()
//       .split();

//   let socket: WebsysSocket = WebsysSocket {
//     sender: Mutex::new(sender),
//     receiver: Mutex::new(receiver),
//   };

//   Ok(Websocket::<In, Out, Enc> {
//     protocol: None,
//     _in: std::marker::PhantomData,
//     web: Some(socket),
//     response: None,
//   })
// }

/// The return type of the `use_websocket` hook.
///
/// See the `use_websocket` documentation for more details.
///
/// This handle provides methods to send and receive messages, check the connection status,
/// and wait for the connection to be established.
pub struct UseWebsocket<In, Out, Enc = JsonEncoding>
where
  In: 'static,
  Out: 'static,
  Enc: 'static,
{
  connection: Resource<Result<Websocket<In, Out, Enc>, CapturedError>>,
  waker: UseWaker<()>,
  status: Signal<WebsocketState>,
  status_read: ReadSignal<WebsocketState>,
}

impl<In, Out, E> UseWebsocket<In, Out, E> {
  /// Wait for the connection to be established. This guarantees that subsequent calls to methods like
  /// `.try_recv()` will not fail due to the connection not being ready.
  pub async fn connect(&self) -> WebsocketState {
    // Wait for the connection to be established
    while !self.connection.finished() {
      _ = self.waker.wait().await;
    }

    self.status.cloned()
  }

  /// Returns true if the WebSocket is currently connecting.
  ///
  /// This can be useful to present a loading state to the user while the connection is being established.
  pub fn connecting(&self) -> bool {
    matches!(self.status.cloned(), WebsocketState::Connecting)
  }

  /// Returns true if the Websocket is closed due to an error.
  pub fn is_err(&self) -> bool {
    matches!(self.status.cloned(), WebsocketState::FailedToConnect)
  }

  /// Returns true if the WebSocket is currently shut down and cannot be used to send or receive messages.
  pub fn is_closed(&self) -> bool {
    matches!(
      self.status.cloned(),
      WebsocketState::Closed | WebsocketState::FailedToConnect
    )
  }

  /// Get the current status of the WebSocket connection.
  pub fn status(&self) -> ReadSignal<WebsocketState> {
    self.status_read
  }

  /// Send a raw message over the WebSocket connection
  ///
  /// To send a message with a particular type, see the `.send()` method instead.
  pub async fn send_raw(&self, msg: Message) -> Result<(), WebsocketError> {
    self.connect().await;

    self
      .connection
      .as_ref()
      .as_deref()
      .ok_or_else(WebsocketError::closed_away)?
      .as_ref()
      .map_err(|_| WebsocketError::AlreadyClosed)?
      .send_raw(msg)
      .await
  }

  /// Receive a raw message from the WebSocket connection
  ///
  /// To receive a message with a particular type, see the `.recv()` method instead.
  pub async fn recv_raw(&mut self) -> Result<Message, WebsocketError> {
    self.connect().await;

    let result = self
      .connection
      .as_ref()
      .as_deref()
      .ok_or_else(WebsocketError::closed_away)?
      .as_ref()
      .map_err(|_| WebsocketError::AlreadyClosed)?
      .recv_raw()
      .await;

    if let Err(WebsocketError::ConnectionClosed { .. }) = result.as_ref() {
      self.received_shutdown();
    }

    result
  }

  pub async fn send(&self, msg: In) -> Result<(), WebsocketError>
  where
    In: Serialize,
    E: Encoding,
  {
    self
      .send_raw(Message::Binary(
        E::to_bytes(&msg).ok_or_else(WebsocketError::serialization)?,
      ))
      .await
  }

  /// Receive the next message from the WebSocket connection, deserialized into the `Out` type.
  ///
  /// If the connection is still opening, this will wait until the connection is established.
  /// If the connection fails to open or is killed while waiting, an error will be returned.
  ///
  /// This method returns an error if the connection is closed since we assume closed connections
  /// are a "failure".
  pub async fn recv(&mut self) -> Result<Out, WebsocketError>
  where
    Out: DeserializeOwned,
    E: Encoding,
  {
    self.connect().await;

    let result = self
      .connection
      .as_ref()
      .as_deref()
      .ok_or_else(WebsocketError::closed_away)?
      .as_ref()
      .map_err(|_| WebsocketError::AlreadyClosed)?
      .recv()
      .await;

    if let Err(WebsocketError::ConnectionClosed { .. }) = result.as_ref() {
      self.received_shutdown();
    }

    result
  }

  /// Set the WebSocket connection.
  ///
  /// This method takes a `Result<Websocket<In, Out, E>, Err>`, allowing you to drive the connection
  /// into an errored state manually.
  pub fn set<Err: Into<CapturedError>>(&mut self, socket: Result<Websocket<In, Out, E>, Err>) {
    match socket {
      Ok(_) => self.status.set(WebsocketState::Open),
      Err(_) => self.status.set(WebsocketState::FailedToConnect),
    }

    self.connection.set(Some(socket.map_err(|e| e.into())));
    self.waker.wake(());
  }

  /// Mark the WebSocket as closed. This is called internally when the connection is closed.
  fn received_shutdown(&self) {
    let mut _self = *self;
    _self.status.set(WebsocketState::Closed);
    _self.waker.wake(());
  }
}

impl<In, Out, E> Copy for UseWebsocket<In, Out, E> {}
impl<In, Out, E> Clone for UseWebsocket<In, Out, E> {
  fn clone(&self) -> Self {
    *self
  }
}

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum WebsocketState {
  /// The WebSocket is connecting.
  Connecting,

  /// The WebSocket is open and ready to send and receive messages.
  Open,

  /// The WebSocket is closing.
  Closing,

  /// The WebSocket is closed and cannot be used to send or receive messages.
  Closed,

  /// The WebSocket failed to connect
  FailedToConnect,
}

/// A WebSocket connection that can send and receive messages of type `In` and `Out`.
pub struct Websocket<In = String, Out = String, E = JsonEncoding> {
  pub protocol: Option<String>,

  #[allow(clippy::type_complexity)]
  pub _in: std::marker::PhantomData<fn() -> (In, Out, E)>,

  #[cfg(feature = "web")]
  pub web: Option<WebsysSocket>,

  pub response: Option<gloo_net::http::Response>,
}

impl<I, O, E> Websocket<I, O, E> {
  pub async fn recv(&self) -> Result<O, WebsocketError>
  where
    O: DeserializeOwned,
    E: Encoding,
  {
    loop {
      let msg = self.recv_raw().await?;
      info!("{:?}", msg);
      match msg {
        Message::Text(text) => {
          info!("{}", text);
          let e: O = E::decode(text.into()).ok_or_else(WebsocketError::deserialization)?;
          return Ok(e);
        }
        Message::Binary(bytes) => {
          let e: O = E::decode(bytes).ok_or_else(WebsocketError::deserialization)?;
          return Ok(e);
        }
        Message::Close { code, reason } => {
          return Err(WebsocketError::ConnectionClosed {
            code,
            description: reason,
          });
        }

        // todo - are we supposed to response to pings?
        Message::Ping(_bytes) => continue,
        Message::Pong(_bytes) => continue,
      }
    }
  }

  /// Send a typed message over the WebSocket connection.
  ///
  /// This method serializes the message using the specified encoding `E` before sending it.
  /// The message will always be sent as a binary message, even if the encoding is valid UTF-8
  /// like JSON.
  pub async fn send(&self, msg: I) -> Result<(), WebsocketError>
  where
    I: Serialize,
    E: Encoding,
  {
    let bytes = E::to_bytes(&msg).ok_or_else(WebsocketError::serialization)?;
    self.send_raw(Message::Binary(bytes)).await
  }

  /// Send a raw message over the WebSocket connection.
  ///
  /// This method allows sending text, binary, ping, pong, and close messages directly.
  pub async fn send_raw(&self, message: Message) -> Result<(), WebsocketError> {
    #[cfg(feature = "web")]
    let mut sender = self
      .web
      .as_ref()
      .ok_or_else(|| WebsocketError::Uninitialized)?
      .sender
      .lock()
      .await;

    match message {
      Message::Text(s) => {
        sender.send(gloo_net::websocket::Message::Text(s)).await?;
      }
      Message::Binary(bytes) => {
        sender
          .send(gloo_net::websocket::Message::Bytes(bytes.into()))
          .await?;
      }
      Message::Close { .. } => {
        sender.close().await?;
      }
      Message::Ping(_bytes) => return Ok(()),
      Message::Pong(_bytes) => return Ok(()),
    }

    Ok(())
  }

  /// Receive a raw message from the WebSocket connection.
  pub async fn recv_raw(&self) -> Result<Message, WebsocketError> {
    #[cfg(feature = "web")]
    let mut conn = self.web.as_ref().unwrap().receiver.lock().await;
    return match conn.next().await {
      Some(Ok(WsMessage::Text(text))) => Ok(Message::Text(text)),
      Some(Ok(WsMessage::Bytes(items))) => Ok(Message::Binary(items.into())),
      Some(Err(e)) => Err(WebsocketError::from(e)),
      None => Err(WebsocketError::closed_away()),
    };

    unimplemented!("Non web wasm32 clients are not supported yet")
  }

  pub fn protocol(&self) -> Option<&str> {
    self.protocol.as_deref()
  }
}

// no two websockets are ever equal
impl<I, O, E> PartialEq for Websocket<I, O, E> {
  fn eq(&self, _other: &Self) -> bool {
    false
  }
}

pub struct WebSocketOptions {
  protocols: Vec<String>,
  automatic_reconnect: bool,
}

impl WebSocketOptions {
  pub fn new() -> Self {
    Self {
      protocols: Vec::new(),
      automatic_reconnect: false,
    }
  }

  /// Automatically reconnect if the connection is lost. This uses an exponential backoff strategy.
  pub fn with_automatic_reconnect(mut self) -> Self {
    self.automatic_reconnect = true;
    self
  }
}

impl Default for WebSocketOptions {
  fn default() -> Self {
    Self::new()
  }
}

#[doc(hidden)]
pub struct UpgradingWebsocket {
  protocol: Option<String>,

  #[cfg(feature = "web")]
  web: Option<gloo_net::websocket::futures::WebSocket>,
}

unsafe impl Send for UpgradingWebsocket {}
unsafe impl Sync for UpgradingWebsocket {}

#[derive(thiserror::Error, Debug)]
pub enum WebsocketError {
  #[error("Connection closed")]
  ConnectionClosed {
    code: CloseCode,
    description: String,
  },

  #[error("WebSocket already closed")]
  AlreadyClosed,

  #[error("WebSocket capacity reached")]
  Capacity,

  #[error("An unexpected internal error occurred")]
  Unexpected,

  #[error("WebSocket is not initialized on this platform")]
  Uninitialized,

  /// Error during serialization/deserialization.
  #[error("error during serialization/deserialization")]
  Deserialization(Box<dyn std::error::Error + Send + Sync>),

  /// Error during serialization/deserialization.
  #[error("error during serialization/deserialization")]
  Serialization(Box<dyn std::error::Error + Send + Sync>),

  /// Error during serialization/deserialization.
  #[error("serde_json error")]
  #[cfg_attr(docsrs, doc(cfg(feature = "json")))]
  Json(#[from] serde_json::Error),

  /// Error during serialization/deserialization.
  #[cfg(feature = "cbor")]
  #[error("ciborium error")]
  Cbor(#[from] ciborium::de::Error<std::io::Error>),
}

#[cfg(feature = "web")]
impl From<gloo_net::websocket::WebSocketError> for WebsocketError {
  fn from(value: gloo_net::websocket::WebSocketError) -> Self {
    use gloo_net::websocket::WebSocketError;
    match value {
      WebSocketError::ConnectionError => WebsocketError::AlreadyClosed,
      WebSocketError::ConnectionClose(close_event) => WebsocketError::ConnectionClosed {
        code: close_event.code.into(),
        description: close_event.reason,
      },
      WebSocketError::MessageSendError(_js_error) => WebsocketError::Unexpected,
      _ => WebsocketError::Unexpected,
    }
  }
}

impl WebsocketError {
  pub fn closed_away() -> Self {
    Self::ConnectionClosed {
      code: CloseCode::Normal,
      description: "Connection closed normally".into(),
    }
  }

  pub fn deserialization() -> Self {
    Self::Deserialization(anyhow::anyhow!("Failed to deserialize message").into())
  }

  pub fn serialization() -> Self {
    Self::Serialization(anyhow::anyhow!("Failed to serialize message").into())
  }
}

#[cfg(feature = "web")]
pub struct WebsysSocket {
  pub sender: Mutex<SplitSink<WsWebsocket, WsMessage>>,
  pub receiver: Mutex<SplitStream<WsWebsocket>>,
}

/// A `WebSocket` message, which can be a text string or binary data.
#[derive(Clone, Debug)]
pub enum Message {
  /// A text `WebSocket` message.
  // note: we can't use `tungstenite::Utf8String` here, since we don't have tungstenite on wasm.
  Text(String),

  /// A binary `WebSocket` message.
  Binary(Bytes),

  /// A ping message with the specified payload.
  ///
  /// The payload here must have a length less than 125 bytes.
  ///
  /// # WASM
  ///
  /// This variant is ignored for WASM targets.
  Ping(Bytes),

  /// A pong message with the specified payload.
  ///
  /// The payload here must have a length less than 125 bytes.
  ///
  /// # WASM
  ///
  /// This variant is ignored for WASM targets.
  Pong(Bytes),

  /// A close message.
  ///
  /// Sending this will not close the connection, though the remote peer will likely close the connection after receiving this.
  Close { code: CloseCode, reason: String },
}

impl From<String> for Message {
  #[inline]
  fn from(value: String) -> Self {
    Self::Text(value)
  }
}

impl From<&str> for Message {
  #[inline]
  fn from(value: &str) -> Self {
    Self::from(value.to_owned())
  }
}

impl From<Bytes> for Message {
  #[inline]
  fn from(value: Bytes) -> Self {
    Self::Binary(value)
  }
}

impl From<Vec<u8>> for Message {
  #[inline]
  fn from(value: Vec<u8>) -> Self {
    Self::from(Bytes::from(value))
  }
}

impl From<&[u8]> for Message {
  #[inline]
  fn from(value: &[u8]) -> Self {
    Self::from(Bytes::copy_from_slice(value))
  }
}

/// Status code used to indicate why an endpoint is closing the `WebSocket`
/// connection.[1]
///
/// [1]: https://datatracker.ietf.org/doc/html/rfc6455
#[derive(Debug, Default, Eq, PartialEq, Clone, Copy)]
#[non_exhaustive]
pub enum CloseCode {
  /// Indicates a normal closure, meaning that the purpose for
  /// which the connection was established has been fulfilled.
  #[default]
  Normal,

  /// Indicates that an endpoint is "going away", such as a server
  /// going down or a browser having navigated away from a page.
  Away,

  /// Indicates that an endpoint is terminating the connection due
  /// to a protocol error.
  Protocol,

  /// Indicates that an endpoint is terminating the connection
  /// because it has received a type of data it cannot accept (e.g., an
  /// endpoint that understands only text data MAY send this if it
  /// receives a binary message).
  Unsupported,

  /// Indicates that no status code was included in a closing frame. This
  /// close code makes it possible to use a single method, `on_close` to
  /// handle even cases where no close code was provided.
  Status,

  /// Indicates an abnormal closure. If the abnormal closure was due to an
  /// error, this close code will not be used. Instead, the `on_error` method
  /// of the handler will be called with the error. However, if the connection
  /// is simply dropped, without an error, this close code will be sent to the
  /// handler.
  Abnormal,

  /// Indicates that an endpoint is terminating the connection
  /// because it has received data within a message that was not
  /// consistent with the type of the message (e.g., non-UTF-8 \[RFC3629\]
  /// data within a text message).
  Invalid,

  /// Indicates that an endpoint is terminating the connection
  /// because it has received a message that violates its policy.  This
  /// is a generic status code that can be returned when there is no
  /// other more suitable status code (e.g., Unsupported or Size) or if there
  /// is a need to hide specific details about the policy.
  Policy,

  /// Indicates that an endpoint is terminating the connection
  /// because it has received a message that is too big for it to
  /// process.
  Size,

  /// Indicates that an endpoint (client) is terminating the
  /// connection because it has expected the server to negotiate one or
  /// more extension, but the server didn't return them in the response
  /// message of the `WebSocket` handshake.  The list of extensions that
  /// are needed should be given as the reason for closing.
  /// Note that this status code is not used by the server, because it
  /// can fail the `WebSocket` handshake instead.
  Extension,

  /// Indicates that a server is terminating the connection because
  /// it encountered an unexpected condition that prevented it from
  /// fulfilling the request.
  Error,

  /// Indicates that the server is restarting. A client may choose to
  /// reconnect, and if it does, it should use a randomized delay of 5-30
  /// seconds between attempts.
  Restart,

  /// Indicates that the server is overloaded and the client should either
  /// connect to a different IP (when multiple targets exist), or
  /// reconnect to the same IP when a user has performed an action.
  Again,

  /// Indicates that the connection was closed due to a failure to perform a
  /// TLS handshake (e.g., the server certificate can't be verified). This
  /// is a reserved value and MUST NOT be set as a status code in a Close
  /// control frame by an endpoint.
  Tls,

  /// Reserved status codes.
  Reserved(u16),

  /// Reserved for use by libraries, frameworks, and applications. These
  /// status codes are registered directly with IANA. The interpretation of
  /// these codes is undefined by the `WebSocket` protocol.
  Iana(u16),

  /// Reserved for private use. These can't be registered and can be used by
  /// prior agreements between `WebSocket` applications. The interpretation of
  /// these codes is undefined by the `WebSocket` protocol.
  Library(u16),

  /// Unused / invalid status codes.
  Bad(u16),
}

impl CloseCode {
  /// Check if this `CloseCode` is allowed.
  #[must_use]
  pub const fn is_allowed(self) -> bool {
    !matches!(
      self,
      Self::Bad(_) | Self::Reserved(_) | Self::Status | Self::Abnormal | Self::Tls
    )
  }
}

impl std::fmt::Display for CloseCode {
  fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
    let code: u16 = (*self).into();
    write!(f, "{code}")
  }
}

impl From<CloseCode> for u16 {
  fn from(code: CloseCode) -> Self {
    match code {
      CloseCode::Normal => 1000,
      CloseCode::Away => 1001,
      CloseCode::Protocol => 1002,
      CloseCode::Unsupported => 1003,
      CloseCode::Status => 1005,
      CloseCode::Abnormal => 1006,
      CloseCode::Invalid => 1007,
      CloseCode::Policy => 1008,
      CloseCode::Size => 1009,
      CloseCode::Extension => 1010,
      CloseCode::Error => 1011,
      CloseCode::Restart => 1012,
      CloseCode::Again => 1013,
      CloseCode::Tls => 1015,
      CloseCode::Reserved(code)
      | CloseCode::Iana(code)
      | CloseCode::Library(code)
      | CloseCode::Bad(code) => code,
    }
  }
}

impl From<u16> for CloseCode {
  fn from(code: u16) -> Self {
    match code {
      1000 => Self::Normal,
      1001 => Self::Away,
      1002 => Self::Protocol,
      1003 => Self::Unsupported,
      1005 => Self::Status,
      1006 => Self::Abnormal,
      1007 => Self::Invalid,
      1008 => Self::Policy,
      1009 => Self::Size,
      1010 => Self::Extension,
      1011 => Self::Error,
      1012 => Self::Restart,
      1013 => Self::Again,
      1015 => Self::Tls,
      1016..=2999 => Self::Reserved(code),
      3000..=3999 => Self::Iana(code),
      4000..=4999 => Self::Library(code),
      _ => Self::Bad(code),
    }
  }
}
