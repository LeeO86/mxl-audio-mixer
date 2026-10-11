#!/usr/bin/env bash
# Build libmxl at the platform pin (dmf-mxl/mxl release/v1.1, 218ddaa) with the
# fabrics library off, and install it (default /opt/mxl).
#
#   scripts/build-mxl.sh [prefix]
#
# Same recipe as the sibling repos (mxl-replay CI): vcpkg for stduuid, spdlog,
# fmt and picojson; no tests, tools, utils or docs.
set -euo pipefail

MXL_REF=218ddaa0a08c12ffe75fc475ae65aa3d9eef16d7
PREFIX="${1:-/opt/mxl}"
WORK="${MXL_BUILD_DIR:-/tmp/mxl-build}"
VCPKG="${VCPKG_ROOT:-/opt/vcpkg}"

if [[ ! -x "$VCPKG/vcpkg" ]]; then
    if [[ ! -d "$VCPKG/.git" ]]; then
        sudo mkdir -p "$VCPKG"
        sudo chown "$(id -u):$(id -g)" "$VCPKG"
        git clone -q --depth 1 https://github.com/microsoft/vcpkg "$VCPKG"
    fi
    "$VCPKG/bootstrap-vcpkg.sh" -disableMetrics
fi

mkdir -p "$WORK"
if [[ ! -d "$WORK/src/.git" ]]; then
    git init -q "$WORK/src"
    git -C "$WORK/src" remote add origin https://github.com/dmf-mxl/mxl.git
fi
git -C "$WORK/src" fetch -q --depth 1 origin "$MXL_REF"
git -C "$WORK/src" checkout -q FETCH_HEAD

baseline=$(git -C "$VCPKG" rev-parse HEAD)
mkdir -p "$WORK/manifest"
cat >"$WORK/manifest/vcpkg.json" <<EOF
{"dependencies": [{"name": "stduuid", "features": ["system-gen", "gsl-span"]}, "spdlog", "fmt", "picojson"],
 "builtin-baseline": "$baseline"}
EOF

cmake -S "$WORK/src" -B "$WORK/build" -G Ninja \
    -DCMAKE_TOOLCHAIN_FILE="$VCPKG/scripts/buildsystems/vcpkg.cmake" \
    -DVCPKG_MANIFEST_DIR="$WORK/manifest" \
    -DCMAKE_BUILD_TYPE=Release \
    -DBUILD_TESTS=OFF -DBUILD_TOOLS=OFF -DBUILD_UTILS=OFF -DBUILD_DOCS=OFF \
    -DMXL_ENABLE_FABRICS_OFI=OFF \
    -DCMAKE_INSTALL_PREFIX="$PREFIX"
cmake --build "$WORK/build" -j"$(nproc)"
if [[ -w "$(dirname "$PREFIX")" ]]; then
    cmake --install "$WORK/build"
else
    sudo cmake --install "$WORK/build"
fi
echo "libmxl $MXL_REF installed to $PREFIX/lib"
