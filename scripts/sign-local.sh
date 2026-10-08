#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/ohos-env.sh"

if [[ $# -ne 0 ]]; then
  printf 'Usage: %s\nSigns the existing arm64 PhotoCraft build using .signing/build-profile.json5.\n' "$0" >&2
  exit 2
fi

craft_require_file "${CRAFT_PROJECT_ROOT}/.signing/build-profile.json5"
craft_require_file "${CRAFT_PROJECT_ROOT}/harmonyos/entry/libs/arm64-v8a/libphotocraft_ohos.a"
craft_require_file "${CRAFT_NODE}"
craft_require_file "${CRAFT_HVIGOR}"
craft_require_file "${CRAFT_OHPM}"
craft_prepare_hvigor

cd "${CRAFT_PROJECT_ROOT}/harmonyos"
"${CRAFT_NODE}" "${CRAFT_OHPM}" install --all --cache "${CRAFT_OHPM_CACHE}" \
  --no-experimental-concurrently-safe

python3 - "${CRAFT_PROJECT_ROOT}" "${CRAFT_NODE}" "${CRAFT_HVIGOR}" <<'PY'
import copy
import fcntl
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys

root = Path(sys.argv[1])
public_path = root / 'harmonyos/build-profile.json5'
private_path = root / '.signing/build-profile.json5'
lock_path = root / '.signing/sign-local.lock'
backup_path = root / '.signing/build-profile.public-backup.json5'

# Only one signing invocation may temporarily replace the public build profile.
with lock_path.open('a') as lock_file:
    try:
        fcntl.flock(lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit('Another local signing build is already running.')

    original = public_path.read_bytes()
    original_mode = stat.S_IMODE(public_path.stat().st_mode)
    public_config = json.loads(original)
    private_config = json.loads(private_path.read_text())
    configurations = private_config.get('app', {}).get('signingConfigs', [])
    if not configurations:
        sys.exit('The private profile contains no signingConfigs. Generate a debug signature in DevEco first.')
    if public_config.get('app', {}).get('signingConfigs'):
        sys.exit('The public profile already contains signing material. Move it to .signing before running this script.')

    signed_config = copy.deepcopy(public_config)
    signed_config['app']['signingConfigs'] = configurations
    private_products = {item['name']: item for item in private_config['app'].get('products', [])}
    names = {item['name'] for item in configurations}
    for product in signed_config['app'].get('products', []):
        selected = private_products.get(product['name'], {}).get('signingConfig')
        if not selected and len(names) == 1:
            selected = next(iter(names))
        if selected not in names:
            sys.exit('A product has no unambiguous private signingConfig. Configure it in DevEco first.')
        product['signingConfig'] = selected

    current_bundle = json.loads((root / 'harmonyos/AppScope/app.json5').read_text())['app']['bundleName']
    for configuration in configurations:
        profile_path = Path(configuration['material']['profile'])
        if not profile_path.is_absolute():
            profile_path = root / 'harmonyos' / profile_path
        decoded = subprocess.run([
            'openssl', 'cms', '-verify', '-inform', 'DER', '-in', str(profile_path), '-noverify'
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if decoded.returncode:
            sys.exit('The private provisioning profile could not be decoded.')
        allowed_bundle = json.loads(decoded.stdout).get('bundle-info', {}).get('bundle-name')
        if allowed_bundle not in ('*', current_bundle):
            sys.exit('The private provisioning profile does not match the current application bundle.')

    def interrupted(signum, frame):
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, interrupted)
    backup_path.write_bytes(original)
    backup_path.chmod(0o600)
    return_code = 1
    try:
        public_path.write_text(json.dumps(signed_config, indent=2) + '\n')
        os.chmod(public_path, 0o600)
        result = subprocess.run([
            sys.argv[2], sys.argv[3], '--mode', 'module', '-p', 'product=default',
            '-p', 'module=entry@default', '-p', 'buildMode=debug', '-p', 'enableSignTask=true',
            'assembleHap', '--no-daemon', '--no-parallel', '--no-stacktrace', '--no-analyze'
        ], cwd=root / 'harmonyos', stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        return_code = result.returncode
        # Keep credential-bearing signer command details out of the terminal.
        for line in result.stdout.splitlines():
            if any(marker in line for marker in ('Finished :', 'UP-TO-DATE :', 'BUILD SUCCESSFUL', 'BUILD FAILED')):
                print(line)
        if return_code:
            print('Signing build failed. Inspect the local .hvigor/outputs/build-logs directory.', file=sys.stderr)
    finally:
        public_path.write_bytes(original)
        os.chmod(public_path, original_mode)
        print('Restored the public build profile without signing material.')

    if return_code == 0:
        signed_hap = root / 'harmonyos/entry/build/default/outputs/default/entry-default-signed.hap'
        if not signed_hap.is_file():
            sys.exit('The build did not produce entry-default-signed.hap. Verify the private signing configuration.')
        print(f'Signed HAP: {signed_hap}')
    sys.exit(return_code)
PY
