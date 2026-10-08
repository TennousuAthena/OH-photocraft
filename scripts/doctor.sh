#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/ohos-env.sh"

case "${1:-}" in
  ''|--devices) ;;
  *) printf 'Usage: %s [--devices]\n' "$0" >&2; exit 2 ;;
esac

CRAFT_DOCTOR_FAILURES=0
craft_check_tool() {
  if [[ -x "$2" ]]; then
    printf 'OK   %-16s %s\n' "$1" "$2"
  else
    printf 'FAIL %-16s %s\n' "$1" "$2"
    CRAFT_DOCTOR_FAILURES=$((CRAFT_DOCTOR_FAILURES + 1))
  fi
}

printf 'PhotoCraft HarmonyOS environment\n'
printf 'Project: %s\nSDK: %s\n' "${CRAFT_PROJECT_ROOT}" "${OHOS_SDK}"
craft_check_tool 'OHOS clang' "${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER}"
craft_check_tool 'OHOS clang++' "${CXX_aarch64_unknown_linux_ohos}"
craft_check_tool 'llvm-ar' "${AR_aarch64_unknown_linux_ohos}"
craft_check_tool 'Bundled Node' "${CRAFT_NODE}"
craft_check_tool 'Bundled Java' "${JAVA_HOME}/bin/java"
if [[ -f "${CRAFT_HVIGOR}" ]]; then
  printf 'OK   %-16s %s\n' 'Hvigor' "${CRAFT_HVIGOR}"
else
  printf 'FAIL %-16s %s\n' 'Hvigor' "${CRAFT_HVIGOR}"
  CRAFT_DOCTOR_FAILURES=$((CRAFT_DOCTOR_FAILURES + 1))
fi
if [[ -f "${OHOS_SDK}/oh-uni-package.json" ]]; then
  cat "${OHOS_SDK}/oh-uni-package.json"
fi
if command -v rustc >/dev/null && command -v cargo >/dev/null && command -v rustup >/dev/null; then
  rustc --version
  cargo --version
  if rustup target list --installed | grep -qx "${CRAFT_RUST_TARGET}"; then
    printf 'OK   Rust target      %s\n' "${CRAFT_RUST_TARGET}"
  else
    printf 'FAIL Rust target      Install with: rustup target add %s\n' "${CRAFT_RUST_TARGET}"
    CRAFT_DOCTOR_FAILURES=$((CRAFT_DOCTOR_FAILURES + 1))
  fi
  rustc --print cfg --target "${CRAFT_RUST_TARGET}" | grep -E '^target_(arch|os|env)='
else
  printf 'FAIL Rust tools       rustc, cargo, and rustup must be on PATH\n'
  CRAFT_DOCTOR_FAILURES=$((CRAFT_DOCTOR_FAILURES + 1))
fi
printf 'Cargo cache: %s\nHvigor cache: %s\n' "${CARGO_HOME}" "${HVIGOR_USER_HOME}"
CRAFT_HDC="${DEVECO_SDK_HOME}/default/openharmony/toolchains/hdc"
if [[ "${1:-}" == --devices && -x "${CRAFT_HDC}" ]]; then
  printf 'Connected HarmonyOS devices (a device is required for GPU/input acceptance):\n'
  "${CRAFT_HDC}" list targets &
  CRAFT_HDC_PID=$!
  (sleep 5; kill -TERM "${CRAFT_HDC_PID}" 2>/dev/null || true) &
  CRAFT_TIMEOUT_PID=$!
  wait "${CRAFT_HDC_PID}" || printf 'Device query failed or timed out after 5 seconds.\n'
  kill -TERM "${CRAFT_TIMEOUT_PID}" 2>/dev/null || true
elif [[ -x "${CRAFT_HDC}" ]]; then
  printf 'Device query: run %s --devices when a PC or emulator is connected.\n' "$0"
fi
if [[ ${CRAFT_DOCTOR_FAILURES} -gt 0 ]]; then
  printf '%s required environment checks failed.\n' "${CRAFT_DOCTOR_FAILURES}" >&2
  exit 1
fi
