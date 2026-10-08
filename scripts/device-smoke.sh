#!/usr/bin/env bash
# Default is read-only. Installation requires the explicit --install-and-run flag.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/ohos-env.sh"

craft_require_file "${CRAFT_NODE}"
export CRAFT_PROJECT_ROOT CRAFT_DEVECO_CONTENTS
export CRAFT_HDC="${HDC:-${DEVECO_SDK_HOME}/default/openharmony/toolchains/hdc}"
exec "${CRAFT_NODE}" - "$@" <<'NODE'
const fs = require('fs');
const path = require('path');
const cp = require('child_process');
const root = process.env.CRAFT_PROJECT_ROOT;
const args = process.argv.slice(2);
if (args.includes('--help') || args.includes('-h')) {
  console.log('Usage: scripts/device-smoke.sh [--check|--install-and-run]');
  console.log('Default --check reads device IDs/API and signed HAP metadata.');
  console.log('--install-and-run installs this bundle, starts its main ability, and checks its first rendered frame.');
  console.log('Overrides: HDC_DEVICE, HDC, HAP_PATH. Reports: logs/device-smoke/<timestamp>.');
  console.log('Logs are restricted to this app PID and the PhotoCraft tag. Screenshots are not taken.');
  process.exit(0);
}
if (args.length > 1 || (args.length === 1 && !['--check', '--install-and-run'].includes(args[0]))) {
  console.error('Choose --check or --install-and-run.');
  process.exit(2);
}
const install = args[0] === '--install-and-run';
const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const report = path.join(root, 'logs/device-smoke', stamp);
fs.mkdirSync(report, { recursive: true });
function save(name, text) { fs.writeFileSync(path.join(report, name), text); }
function command(executable, argv, timeout = 5000) {
  const result = cp.spawnSync(executable, argv, {
    encoding: 'utf8', timeout, killSignal: 'SIGTERM', maxBuffer: 4 * 1024 * 1024
  });
  if (result.error) throw new Error(`${path.basename(executable)}: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`${path.basename(executable)} exited ${result.status}: ${(result.stderr || result.stdout).trim()}`);
  return result.stdout;
}
function fail(message) { console.error(message); console.error(`Report: ${report}`); process.exit(1); }

(async () => {
  const json5 = require(path.join(process.env.CRAFT_DEVECO_CONTENTS, 'tools/hvigor/hvigor-ohos-plugin/node_modules/json5'));
  const app = json5.parse(fs.readFileSync(path.join(root, 'harmonyos/AppScope/app.json5'), 'utf8')).app;
  const module = json5.parse(fs.readFileSync(path.join(root, 'harmonyos/entry/src/main/module.json5'), 'utf8')).module;
  const identity = { bundle: app.bundleName, module: module.name, ability: module.mainElement };
  for (const value of Object.values(identity)) {
    if (typeof value !== 'string' || !/^[A-Za-z][A-Za-z0-9_.]*$/.test(value)) throw new Error('Invalid app, module, or ability identifier.');
  }
  save('identity.json', JSON.stringify(identity, null, 2) + '\n');
  console.log(`Bundle: ${identity.bundle}; ability: ${identity.ability}`);

  const hap = process.env.HAP_PATH || path.join(root, 'harmonyos/entry/build/default/outputs/default/entry-default-signed.hap');
  let hapError;
  let pack;
  try {
    if (!fs.existsSync(hap)) throw new Error(`Signed HAP is missing: ${hap}. Run scripts/sign-local.sh after the native build.`);
    pack = JSON.parse(command('unzip', ['-p', hap, 'pack.info']));
    const entries = command('unzip', ['-Z1', hap]).split(/\r?\n/);
    const packagedBundle = pack.summary?.app?.bundleName;
    if (packagedBundle !== identity.bundle) throw new Error(`HAP bundle ${packagedBundle} does not match AppScope ${identity.bundle}; rebuild the signed HAP.`);
    if (!entries.includes('libs/arm64-v8a/libphotocraft.so')) throw new Error('Signed HAP does not contain libs/arm64-v8a/libphotocraft.so.');
    const info = { path: hap, bytes: fs.statSync(hap).size, bundle: packagedBundle,
      nativeLibrary: 'libs/arm64-v8a/libphotocraft.so',
      api: pack.summary.modules.find(m => m.distro?.moduleName === identity.module)?.apiVersion };
    save('hap.json', JSON.stringify(info, null, 2) + '\n');
    console.log(`HAP: ${hap} (${info.bytes} bytes; native library present)`);
  } catch (error) { hapError = error.message; save('hap-error.txt', hapError + '\n'); console.error(hapError); }

  const hdc = process.env.CRAFT_HDC;
  const output = command(hdc, ['list', 'targets']);
  save('targets.txt', output);
  const targets = output.split(/\r?\n/).map(x => x.trim()).filter(x => /^[A-Za-z0-9_.:-]+$/.test(x));
  const selected = process.env.HDC_DEVICE;
  let device;
  if (selected) {
    if (!targets.includes(selected)) throw new Error('HDC_DEVICE is not a connected device listed by hdc.');
    device = selected;
  } else if (targets.length === 1) { device = targets[0]; }
  else if (targets.length === 0) throw new Error('No connected device. Enable USB debugging and authorize this computer.');
  else throw new Error('Multiple devices connected; set HDC_DEVICE to one of the IDs in targets.txt.');
  const deviceCommand = (argv, timeout) => command(hdc, ['-t', device, ...argv], timeout);
  const api = deviceCommand(['shell', 'param', 'get', 'const.ohos.apiversion']).trim();
  save('api.txt', api + '\n');
  console.log(`Device: ${device}; API: ${api}`);
  if (!/^\d+$/.test(api)) throw new Error('Unable to read a numeric device API version.');
  if (hapError) throw new Error(hapError);
  const compatible = pack.summary.modules.find(m => m.distro?.moduleName === identity.module)?.apiVersion?.compatible;
  if (compatible && Number(api) < Number(compatible)) throw new Error(`Device API ${api} is below HAP compatible API ${compatible}.`);
  if (!install) { console.log(`Read-only checks passed. Report: ${report}`); return; }

  const installed = deviceCommand(['install', hap], 60000);
  save('install.txt', installed);
  if (!/successfully|success/i.test(installed)) throw new Error('hdc install did not report success; see install.txt.');
  console.log(`Installed ${identity.bundle}. Starting ${identity.ability}.`);
  const started = deviceCommand(['shell', 'aa', 'start', '-a', identity.ability, '-b', identity.bundle, '-m', identity.module], 15000);
  save('start.txt', started);
  if (!/successfully|success/i.test(started)) throw new Error('aa start did not report success; see start.txt.');
  let firstFrame = false;
  let pid;
  for (let attempt = 0; attempt < 12; attempt++) {
    await new Promise(resolve => setTimeout(resolve, 1000));
    try { pid = deviceCommand(['shell', 'pidof', identity.bundle]).trim(); }
    catch { pid = ''; }
    if (!/^[1-9]\d*$/.test(pid)) continue;
    save('pid.txt', pid + '\n');
    // Never issue an unfiltered hilog query: PID and tag must both match.
    const log = deviceCommand(['shell', 'hilog', '-x', '-P', pid, '-T', 'PhotoCraft']);
    save('hilog.txt', log);
    if (log.includes('PhotoCraft UI first frame presented')) { firstFrame = true; break; }
  }
  if (!firstFrame) throw new Error(pid
    ? 'App is running, but no rendered PhotoCraft UI frame was reported within the smoke window. Inspect hilog.txt.'
    : 'The app process was not found after launch. No unrelated app logs were collected.');
  console.log(`PhotoCraft UI first frame presented. Report: ${report}`);
})().catch(error => fail(error.message));
NODE
