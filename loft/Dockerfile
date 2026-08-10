# loft -- PidgeIoT CoAP terminator. Build from the REPO ROOT context
# (cargo needs the whole workspace's manifests to resolve `-p loft`):
#
#   docker build -f loft/Dockerfile -t pidgeiot-loft .
#
# See docs/infra/coap-terminator.md for deployment.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build -p loft --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends libssl3 ca-certificates \
  && rm -rf /var/lib/apt/lists/* \
  # Sanity-check at image build time that this OpenSSL actually provides
  # the constrained-device PSK suites loft is configured for -- a silently
  # PSK-less build would reject every device at handshake instead.
  && openssl ciphers 'PSK-AES128-CCM8:PSK-AES128-GCM-SHA256' | grep -q PSK-AES128-CCM8 \
  && useradd --system --no-create-home loft
COPY --from=build /src/target/release/loft /usr/local/bin/loft
USER loft
# coaps (DTLS/UDP) + coaps+tcp (TLS/TCP), one registered port.
EXPOSE 5684/udp 5684/tcp
ENTRYPOINT ["/usr/local/bin/loft"]
