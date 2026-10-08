#!/usr/bin/env bash
# Exercise the production UTF-16 decoder without the HarmonyOS SDK or a device.
set -euo pipefail
CRAFT_IME_APP="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRAFT_IME_CXX="${CXX:-clang++}"
if ! command -v "${CRAFT_IME_CXX}" >/dev/null 2>&1; then
  printf '%s\n' 'Bail out! No C++ compiler found; set CXX or install the system clang++.'
  exit 1
fi
CRAFT_IME_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/photocraft-ime-tests.XXXXXX")"
trap 'rm -rf -- "${CRAFT_IME_TEST_DIR}"' EXIT
if ! "${CRAFT_IME_CXX}" -std=c++17 -Wall -Wextra -Werror -pedantic \
  -I"${CRAFT_IME_APP}/harmonyos/entry/src/main/cpp" \
  "${CRAFT_IME_APP}/tests/native-ime.test.cpp" -o "${CRAFT_IME_TEST_DIR}/native-ime-tests"; then
  printf '%s\n' 'Bail out! Cannot compile the production UTF-16 decoder tests.'
  exit 1
fi
"${CRAFT_IME_TEST_DIR}/native-ime-tests"
