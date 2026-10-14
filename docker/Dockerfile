# syntax=docker/dockerfile:1
# MXL pin: dmf-mxl/mxl release/v1.1 218ddaa0a08c12ffe75fc475ae65aa3d9eef16d7, fabrics off.

FROM ubuntu:24.04 AS mxl
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential cmake ninja-build git curl ca-certificates zip unzip tar pkg-config uuid-dev python3 \
    && rm -rf /var/lib/apt/lists/*
COPY scripts/build-mxl.sh /usr/local/bin/build-mxl.sh
RUN VCPKG_ROOT=/opt/vcpkg MXL_BUILD_DIR=/tmp/mxl /usr/local/bin/build-mxl.sh /opt/mxl \
    && rm -rf /tmp/mxl /opt/vcpkg/buildtrees /opt/vcpkg/downloads

# MediaMTX (MIT) for the built-in preview server (Appendix B.1), the release tarball by checksum.
FROM ubuntu:24.04 AS mediamtx
ARG TARGETARCH=amd64
ARG MEDIAMTX_VERSION=v1.20.1
ARG MEDIAMTX_SHA256_AMD64=81b143f55a5d23d4a8c028d52869c14ea4a59919900528698fcc97a747fd69c6
ARG MEDIAMTX_SHA256_ARM64=d1689f0bfefb1864e5ed3dcc8495eb2d7ec0a654f90bf3cd48980cb3bd08718a
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN set -eu; \
    case "$TARGETARCH" in amd64) sum=$MEDIAMTX_SHA256_AMD64 ;; arm64) sum=$MEDIAMTX_SHA256_ARM64 ;; *) exit 1 ;; esac; \
    curl -fsSL -o /tmp/m.tgz "https://github.com/bluenviron/mediamtx/releases/download/${MEDIAMTX_VERSION}/mediamtx_${MEDIAMTX_VERSION}_linux_${TARGETARCH}.tar.gz"; \
    echo "$sum  /tmp/m.tgz" | sha256sum -c; \
    mkdir /opt/mediamtx && tar xzf /tmp/m.tgz -C /opt/mediamtx mediamtx LICENSE && rm /tmp/m.tgz

FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/index.html web/vite.config.js ./
COPY web/src ./src
RUN npm run build

FROM ubuntu:24.04 AS build
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential cmake curl ca-certificates git pkg-config clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.97.1
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml build.rs ./
COPY src ./src
COPY assets ./assets
COPY --from=web /web/dist/index.html ./web/dist/index.html
RUN cargo build --release --locked --bins \
    && install -m 0755 target/release/mxl-audio-mixer target/release/mxl-mixer-tone \
        target/release/mxl-mixer-probe /usr/local/bin/ \
    && install -D -m 0644 "$(ls -d /opt/cargo/registry/src/*/opusic-sys-*/opus | head -1)/COPYING" \
        /usr/local/share/doc/libopus/COPYING

FROM ubuntu:24.04 AS runtime
ARG MXL_REF=218ddaa0a08c12ffe75fc475ae65aa3d9eef16d7
ARG VERSION=dev
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /config && chown 1000:1000 /config
COPY --from=mxl /opt/mxl/lib/libmxl.so* /usr/local/lib/
COPY --from=build /usr/local/bin/mxl-audio-mixer /usr/local/bin/mxl-mixer-tone \
    /usr/local/bin/mxl-mixer-probe /usr/local/bin/
COPY --from=mediamtx /opt/mediamtx/mediamtx /usr/local/bin/mediamtx
COPY --from=mediamtx /opt/mediamtx/LICENSE /usr/share/doc/mediamtx/LICENSE
# libopus (BSD-3-Clause) and ONNX Runtime (MIT, with its third-party notices)
# are linked statically into the mixer; the Silero VAD model (MIT) and
# Com-IS-Assist's control core (MIT) are in it.
COPY --from=build /usr/local/share/doc/libopus/COPYING /usr/share/doc/libopus/COPYING
COPY assets/onnxruntime.LICENSE /usr/share/doc/onnxruntime/LICENSE
COPY assets/onnxruntime.ThirdPartyNotices.txt /usr/share/doc/onnxruntime/ThirdPartyNotices.txt
COPY assets/silero_vad.LICENSE /usr/share/doc/silero-vad/LICENSE
COPY src/auto/comis/LICENSE /usr/share/doc/com-is-assist/LICENSE
RUN ldconfig && ldd /usr/local/lib/libmxl.so.1 | (! grep -q "not found") \
    && ldd /usr/local/bin/mxl-audio-mixer | (! grep -q "not found")
USER 1000
# Web/API, NMOS, the built-in MediaMTX's WHEP (TCP) and ICE (UDP).
EXPOSE 8190 3342 8599 8199/udp
LABEL org.opencontainers.image.source="https://github.com/LeeO86/mxl-audio-mixer" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}" \
      io.dmf.mxl.revision="${MXL_REF}"
ENTRYPOINT ["/usr/local/bin/mxl-audio-mixer"]
