#!/usr/bin/env bash
set -euo pipefail
craft_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
craft_pin=4337a6227a823a28728e68aed844feab62b3314d
craft_source="${craft_root}/upstream"
if [[ -e "${craft_source}/.git" ]]; then
  if [[ "$(git -C "${craft_source}" rev-parse HEAD)" != "${craft_pin}" ]]; then
    printf 'Upstream is at a different commit. Preserve local work and align it with %s before building.\n' "${craft_pin}" >&2
    exit 1
  fi
  printf 'PhotoCraft upstream is already pinned at %s\n' "${craft_pin}"
  exit 0
fi
if [[ -f "${craft_root}/.gitmodules" ]] && git -C "${craft_root}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git -C "${craft_root}" submodule update --init --recursive upstream
  if [[ "$(git -C "${craft_source}" rev-parse HEAD)" != "${craft_pin}" ]]; then
    printf 'The upstream submodule must be pinned at %s before building.\n' "${craft_pin}" >&2
    exit 1
  fi
  printf 'PhotoCraft upstream is pinned at %s\n' "${craft_pin}"
  exit 0
fi
if [[ -e "${craft_source}" ]]; then
  printf 'Existing upstream directory has no Git checkout; preserve it before fetching.\n' >&2
  exit 1
fi
git init "${craft_source}"
git -C "${craft_source}" remote add origin https://github.com/storytold/photocraft.git
git -C "${craft_source}" fetch --depth 1 origin "${craft_pin}"
git -C "${craft_source}" checkout --detach FETCH_HEAD
