#!/usr/bin/env bash
# Sourced by project scripts. Settings are scoped to their processes; no global
# Cargo, Rustup, Hvigor, or DevEco configuration is changed.
set -euo pipefail

CRAFT_PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRAFT_DEVECO_CONTENTS="${DEVECO_STUDIO_CONTENTS:-/Applications/DevEco-Studio.app/Contents}"
export OHOS_SDK="${OHOS_SDK:-${CRAFT_DEVECO_CONTENTS}/sdk/default/openharmony/native}"
export DEVECO_SDK_HOME="${DEVECO_SDK_HOME:-${CRAFT_DEVECO_CONTENTS}/sdk}"
export JAVA_HOME="${JAVA_HOME:-${CRAFT_DEVECO_CONTENTS}/jbr/Contents/Home}"
export CARGO_HOME="${CARGO_HOME:-${HOME}/.cargo}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${CRAFT_PROJECT_ROOT}/target}"
export HVIGOR_USER_HOME="${HVIGOR_USER_HOME:-${CRAFT_PROJECT_ROOT}/.cache/hvigor}"
CRAFT_OHPM_CACHE="${OHPM_CACHE:-${CRAFT_PROJECT_ROOT}/.cache/ohpm}"
CRAFT_NODE="${CRAFT_DEVECO_CONTENTS}/tools/node/bin/node"
export ARKTS_TYPESCRIPT_PATH="${ARKTS_TYPESCRIPT_PATH:-${DEVECO_SDK_HOME}/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js}"
CRAFT_HVIGOR="${CRAFT_DEVECO_CONTENTS}/tools/hvigor/hvigor/bin/hvigor.js"
CRAFT_OHPM="${CRAFT_DEVECO_CONTENTS}/tools/ohpm/bin/pm-cli.js"
CRAFT_RUST_TARGET=aarch64-unknown-linux-ohos
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="${OHOS_SDK}/llvm/bin/aarch64-unknown-linux-ohos-clang"
export CC_aarch64_unknown_linux_ohos="${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER}"
export CXX_aarch64_unknown_linux_ohos="${OHOS_SDK}/llvm/bin/aarch64-unknown-linux-ohos-clang++"
export AR_aarch64_unknown_linux_ohos="${OHOS_SDK}/llvm/bin/llvm-ar"
export RANLIB_aarch64_unknown_linux_ohos="${OHOS_SDK}/llvm/bin/llvm-ranlib"

craft_require_file() {
  if [[ ! -f "$1" ]]; then
    printf 'Missing required file: %s\n' "$1" >&2
    return 1
  fi
}

craft_prepare_hvigor() {
  mkdir -p "${HVIGOR_USER_HOME}/node_modules/@ohos" "${CRAFT_OHPM_CACHE}"
  local craft_package
  for craft_package in hvigor hvigor-ohos-plugin; do
    local craft_package_link="${HVIGOR_USER_HOME}/node_modules/@ohos/${craft_package}"
    if [[ ! -e "${craft_package_link}" && ! -L "${craft_package_link}" ]]; then
      ln -s "${CRAFT_DEVECO_CONTENTS}/tools/hvigor/${craft_package}" "${craft_package_link}"
    fi
  done
  export NODE_PATH="${HVIGOR_USER_HOME}/node_modules${NODE_PATH:+:${NODE_PATH}}"
}
