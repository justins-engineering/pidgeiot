# loft -- PidgeIoT CoAP terminator. Build from the REPO ROOT context
# (cargo needs the whole workspace's manifests to resolve `-p loft`):
#
#   docker build -f loft/Dockerfile -t pidgeiot-loft .
#
# See docs/infra/coap-terminator.md for deployment.

FROM rust:1-trixie AS build
# libmbedtls-dev: the mbedTLS-backed DTLS listener (RFC 9146 CID) links the
# system library; the shim's compile-time #error probes gate the build
# headers' feature set here.
RUN apt-get update \
  && apt-get install -y --no-install-recommends libmbedtls-dev \
  && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# Every shipped binary is dual-stack (LOFT_DTLS_STACK selects at runtime);
# the mbedtls cargo feature exists only so dev machines without the
# library can still build the OpenSSL-only default.
RUN cargo build -p loft --release --features mbedtls

FROM debian:trixie-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends libssl3t64 libmbedtls21 ca-certificates binutils \
  && rm -rf /var/lib/apt/lists/* \
  # Sanity-check at image build time that this OpenSSL actually provides
  # the constrained-device PSK suites loft is configured for -- a silently
  # PSK-less build would reject every device at handshake instead.
  && openssl ciphers 'PSK-AES128-CCM8:PSK-AES128-GCM-SHA256' | grep -q PSK-AES128-CCM8 \
  # Same idea for mbedTLS's Connection ID support, but against the runtime
  # shared library: the shim's #error probes only cover the build stage's
  # headers, and a CID-less runtime .so would fail every rebind silently.
  # binutils is here only for this nm and leaves in the same layer.
  && nm -D /usr/lib/*/libmbedtls.so.21 | grep -qw mbedtls_ssl_conf_cid \
  && apt-get purge -y --auto-remove binutils \
  && useradd --system --no-create-home loft
COPY --from=build /src/target/release/loft /usr/local/bin/loft
USER loft
# coaps (DTLS/UDP) + coaps+tcp (TLS/TCP), one registered port.
EXPOSE 5684/udp 5684/tcp
ENTRYPOINT ["/usr/local/bin/loft"]
