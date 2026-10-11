# syntax=docker/dockerfile:1
# MXL pin: dmf-mxl/mxl release/v1.1 218ddaa0a08c12ffe75fc475ae65aa3d9eef16d7, fabrics off.

FROM ubuntu:24.04 AS mxl
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential cmake ninja-build git curl ca-certificates zip unzip tar pkg-config uuid-dev python3 \
    && rm -rf /var/lib/apt/lists/*
COPY scripts/build-mxl.sh /usr/local/bin/build-mxl.sh
RUN VCPKG_ROOT=/opt/vcpkg MXL_BUILD_DIR=/tmp/mxl /usr/local/bin/build-mxl.sh /opt/mxl \
    && rm -rf /tmp/mxl /opt/vcpkg/buildtrees /opt/vcpkg/downloads

FROM ubuntu:24.04 AS build
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential curl ca-certificates git pkg-config clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.97.1
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
RUN cargo build --release --locked --bins \
    && install -m 0755 target/release/mxl-audio-mixer target/release/mxl-mixer-tone /usr/local/bin/

FROM ubuntu:24.04 AS runtime
ARG MXL_REF=218ddaa0a08c12ffe75fc475ae65aa3d9eef16d7
ARG VERSION=dev
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /config && chown 1000:1000 /config
COPY --from=mxl /opt/mxl/lib/libmxl.so* /usr/local/lib/
COPY --from=build /usr/local/bin/mxl-audio-mixer /usr/local/bin/mxl-mixer-tone /usr/local/bin/
RUN ldconfig && ldd /usr/local/lib/libmxl.so.1 | (! grep -q "not found")
USER 1000
EXPOSE 8160 3312
LABEL org.opencontainers.image.source="https://github.com/LeeO86/mxl-audio-mixer" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}" \
      io.dmf.mxl.revision="${MXL_REF}"
ENTRYPOINT ["/usr/local/bin/mxl-audio-mixer"]
