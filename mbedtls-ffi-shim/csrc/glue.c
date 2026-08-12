/* The only compiled first-party C in the mbedTLS integration, and it stays
 * crypto-free. It exists for exactly two reasons:
 *
 * 1. mbedTLS contexts are caller-allocated structs whose size depends on
 *    compile-time configuration. Rust must never embed those layouts -- a
 *    Debian security update must not be able to skew an allocation -- so
 *    allocation happens here, sized against the same headers the runtime
 *    shared library was built from (build and runtime stages move together
 *    on trixie, and the libmbedtls.so.21 soname enforces ABI at load).
 *
 * 2. Several conf setters are static inline in the headers and do not
 *    exist as linkable symbols; hand-declared externs for them would not
 *    link. They are wrapped as real functions here.
 *
 * Everything else the Rust side needs is a genuine exported symbol of
 * libmbedtls and is hand-declared in src/ffi.rs against opaque pointers.
 */

#include <stdlib.h>

#include <mbedtls/build_info.h>

#if !defined(MBEDTLS_SSL_DTLS_CONNECTION_ID) || !defined(MBEDTLS_KEY_EXCHANGE_PSK_ENABLED) \
  || !defined(MBEDTLS_SSL_DTLS_HELLO_VERIFY) || !defined(MBEDTLS_SSL_PROTO_DTLS)
#error "system mbedTLS lacks a feature loft's DTLS listener requires"
#endif
#if !defined(MBEDTLS_SSL_SRV_C) || !defined(MBEDTLS_SSL_CLI_C) || !defined(MBEDTLS_SSL_COOKIE_C)
#error "system mbedTLS lacks the SSL server/client/cookie modules"
#endif
#if !defined(MBEDTLS_SSL_DTLS_ANTI_REPLAY)
#error "system mbedTLS lacks DTLS anti-replay"
#endif
#if !defined(MBEDTLS_CCM_C) || !defined(MBEDTLS_GCM_C) || !defined(MBEDTLS_CIPHER_MODE_CBC)
#error "system mbedTLS lacks a cipher mode loft's suite list pins"
#endif

#include <mbedtls/net_sockets.h>
#include <mbedtls/ssl.h>
#include <mbedtls/ssl_ciphersuites.h>
#include <mbedtls/ssl_cookie.h>

#if MBEDTLS_SSL_CID_OUT_LEN_MAX < 8 || MBEDTLS_SSL_CID_IN_LEN_MAX < 8
#error "system mbedTLS CID length caps are below the 8-byte CID this shim mints"
#endif
/* The Rust side hands mbedtls_ssl_get_peer_cid a 32-byte out-buffer whose
 * size the library does not take as a parameter -- it writes up to its own
 * compile-time cap (the peer's CID cap, MBEDTLS_SSL_CID_OUT_LEN_MAX; the
 * IN cap rides along for symmetry). Gate the caps so a rebuilt library
 * can never outgrow that buffer silently. */
#if MBEDTLS_SSL_CID_OUT_LEN_MAX > 32 || MBEDTLS_SSL_CID_IN_LEN_MAX > 32
#error "system mbedTLS CID length caps exceed the shim's peer_cid out-buffer"
#endif

mbedtls_ssl_context *shim_ssl_new(void) {
  mbedtls_ssl_context *p = calloc(1, sizeof *p);
  if (p != NULL) mbedtls_ssl_init(p);
  return p;
}

void shim_ssl_free(mbedtls_ssl_context *p) {
  if (p == NULL) return;
  mbedtls_ssl_free(p);
  free(p);
}

mbedtls_ssl_config *shim_ssl_config_new(void) {
  mbedtls_ssl_config *p = calloc(1, sizeof *p);
  if (p != NULL) mbedtls_ssl_config_init(p);
  return p;
}

void shim_ssl_config_free(mbedtls_ssl_config *p) {
  if (p == NULL) return;
  mbedtls_ssl_config_free(p);
  free(p);
}

mbedtls_ssl_cookie_ctx *shim_cookie_new(void) {
  mbedtls_ssl_cookie_ctx *p = calloc(1, sizeof *p);
  if (p != NULL) mbedtls_ssl_cookie_init(p);
  return p;
}

void shim_cookie_free(mbedtls_ssl_cookie_ctx *p) {
  if (p == NULL) return;
  mbedtls_ssl_cookie_free(p);
  free(p);
}

void shim_ssl_conf_tls12_only(mbedtls_ssl_config *conf) {
  mbedtls_ssl_conf_min_tls_version(conf, MBEDTLS_SSL_VERSION_TLS1_2);
  mbedtls_ssl_conf_max_tls_version(conf, MBEDTLS_SSL_VERSION_TLS1_2);
}

void shim_ssl_set_user_data(mbedtls_ssl_context *ssl, void *p) {
  mbedtls_ssl_set_user_data_p(ssl, p);
}

void *shim_ssl_get_user_data(mbedtls_ssl_context *ssl) {
  return mbedtls_ssl_get_user_data_p(ssl);
}

/* Mirrors of the header constants the Rust side hardcodes, so a unit test
 * can prove those values against the real headers instead of trusting
 * them. Indexed lookup rather than exported globals keeps the Rust side to
 * one trivially-safe extern. */
int shim_const(int which) {
  switch (which) {
    case 0: return MBEDTLS_SSL_IS_SERVER;
    case 1: return MBEDTLS_SSL_IS_CLIENT;
    case 2: return MBEDTLS_SSL_TRANSPORT_DATAGRAM;
    case 3: return MBEDTLS_SSL_PRESET_DEFAULT;
    case 4: return MBEDTLS_SSL_CID_ENABLED;
    case 5: return MBEDTLS_SSL_CID_DISABLED;
    case 6: return MBEDTLS_SSL_UNEXPECTED_CID_IGNORE;
    case 7: return MBEDTLS_ERR_SSL_WANT_READ;
    case 8: return MBEDTLS_ERR_SSL_WANT_WRITE;
    case 9: return MBEDTLS_ERR_SSL_TIMEOUT;
    case 10: return MBEDTLS_ERR_SSL_HELLO_VERIFY_REQUIRED;
    case 11: return MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY;
    case 12: return MBEDTLS_ERR_SSL_CONN_EOF;
    case 13: return MBEDTLS_ERR_NET_SEND_FAILED;
    case 14: return MBEDTLS_ERR_NET_RECV_FAILED;
    case 15: return MBEDTLS_TLS_PSK_WITH_AES_128_CCM_8;
    case 16: return MBEDTLS_TLS_PSK_WITH_AES_128_GCM_SHA256;
    case 17: return MBEDTLS_TLS_PSK_WITH_AES_128_CBC_SHA256;
    default: return 0x7FFFFFFF;
  }
}
