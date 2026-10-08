#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/ohos-env.sh"

CRAFT_BUILD_RUST=true
CRAFT_BUILD_HAP=true
CRAFT_SIGN_HAP=false
CRAFT_SELECTED_BUILD_MODE=all
for CRAFT_ARGUMENT in "$@"; do
case "${CRAFT_ARGUMENT}" in
  --rust-only)
    if [[ ${CRAFT_SELECTED_BUILD_MODE} == hap ]]; then
      printf '%s\n' '--rust-only and --hap-only cannot be combined.' >&2; exit 2
    fi
    CRAFT_BUILD_HAP=false
    CRAFT_SELECTED_BUILD_MODE=rust ;;
  --hap-only)
    if [[ ${CRAFT_SELECTED_BUILD_MODE} == rust ]]; then
      printf '%s\n' '--rust-only and --hap-only cannot be combined.' >&2; exit 2
    fi
    CRAFT_BUILD_RUST=false
    CRAFT_SELECTED_BUILD_MODE=hap ;;
  --signed) CRAFT_SIGN_HAP=true ;;
  -h|--help)
    printf 'Usage: %s [--rust-only|--hap-only] [--signed]\n' "$0"
    printf 'Builds arm64 Rust release archive and unsigned HarmonyOS debug HAP.\n'
    printf 'With --signed, calls scripts/sign-local.sh after verifying the unsigned native HAP.\n'
    printf 'Overrides: OHOS_SDK, DEVECO_STUDIO_CONTENTS, DEVECO_SDK_HOME, CARGO_HOME, CARGO_TARGET_DIR, HVIGOR_USER_HOME.\n'
    exit 0 ;;
  *) printf 'Unknown argument: %s\n' "${CRAFT_ARGUMENT}" >&2; exit 2 ;;
esac
done

craft_require_file "${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER}"
if ${CRAFT_BUILD_RUST}; then
  if ! rustup target list --installed | grep -qx "${CRAFT_RUST_TARGET}"; then
    printf 'Install the target before building: rustup target add %s\n' "${CRAFT_RUST_TARGET}" >&2
    exit 1
  fi
  mkdir -p "${CARGO_HOME}" "${CARGO_TARGET_DIR}"
  printf 'Building PhotoCraft Rust archive for %s\n' "${CRAFT_RUST_TARGET}"
  cargo build --locked --manifest-path "${CRAFT_PROJECT_ROOT}/Cargo.toml" \
    --target "${CRAFT_RUST_TARGET}" -p photocraft-ohos --release
  mkdir -p "${CRAFT_PROJECT_ROOT}/harmonyos/entry/libs/arm64-v8a"
  cp "${CARGO_TARGET_DIR}/${CRAFT_RUST_TARGET}/release/libphotocraft_ohos.a" \
    "${CRAFT_PROJECT_ROOT}/harmonyos/entry/libs/arm64-v8a/libphotocraft_ohos.a"
fi

if ${CRAFT_BUILD_HAP}; then
  craft_require_file "${CRAFT_PROJECT_ROOT}/harmonyos/entry/libs/arm64-v8a/libphotocraft_ohos.a"
  craft_require_file "${CRAFT_NODE}"
  craft_require_file "${CRAFT_HVIGOR}"
  craft_require_file "${CRAFT_OHPM}"
  craft_prepare_hvigor
  cd "${CRAFT_PROJECT_ROOT}/harmonyos"
  "${CRAFT_NODE}" "${CRAFT_OHPM}" install --all --cache "${CRAFT_OHPM_CACHE}" \
    --no-experimental-concurrently-safe
  "${CRAFT_NODE}" "${CRAFT_HVIGOR}" --mode module -p product=default \
    -p module=entry@default -p buildMode=debug -p enableSignTask=false \
    assembleHap --no-daemon --no-parallel
  CRAFT_UNSIGNED_HAP="${CRAFT_PROJECT_ROOT}/harmonyos/entry/build/default/outputs/default/entry-default-unsigned.hap"
  craft_require_file "${CRAFT_UNSIGNED_HAP}"
  if ! unzip -Z1 "${CRAFT_UNSIGNED_HAP}" | grep -x 'libs/arm64-v8a/libphotocraft.so' >/dev/null; then
    printf '%s\n' 'Unsigned HAP is missing the PhotoCraft native library.' >&2
    exit 1
  fi
  printf 'Unsigned HAP: %s\n' "${CRAFT_UNSIGNED_HAP}"
  if ${CRAFT_SIGN_HAP}; then
    craft_require_file "${CRAFT_PROJECT_ROOT}/scripts/sign-local.sh"
    bash "${CRAFT_PROJECT_ROOT}/scripts/sign-local.sh"
    printf 'Signed HAP: %s\n' "${CRAFT_PROJECT_ROOT}/harmonyos/entry/build/default/outputs/default/entry-default-signed.hap"
  fi
fi
