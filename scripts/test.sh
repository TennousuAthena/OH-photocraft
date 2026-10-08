#!/usr/bin/env bash
set -euo pipefail
CRAFT_TEST_APP="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${CRAFT_TEST_APP}/scripts/ohos-env.sh"
export CRAFT_NODE CRAFT_HVIGOR CRAFT_OHPM CRAFT_OHPM_CACHE CRAFT_DEVECO_CONTENTS CRAFT_RUST_TARGET CRAFT_PROJECT_ROOT
export CRAFT_HDC="${HDC:-${DEVECO_SDK_HOME}/default/openharmony/toolchains/hdc}"
exec python3 "${CRAFT_TEST_APP}/scripts/test_runner.py" --project "${CRAFT_PROJECT_ROOT}" "$@"
