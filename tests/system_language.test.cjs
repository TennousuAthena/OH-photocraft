'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Module = require('node:module');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH || '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');

function abilityHarness() {
  const calls = [], foreground = [], warnings = [];
  let language = 'zh-Hans-CN', resolver;
  const native = { setSystemLanguage: tag => { calls.push(tag); return true; }, lastError: () => '' };
  class UIAbility { constructor() { this.context = { resourceManager: { getStringByNameSync: key => `resource:${key}` } }; } }
  const source = path.resolve(__dirname, '../harmonyos/entry/src/main/ets/entryability/EntryAbility.ets');
  const loaded = new Module(source, module);
  loaded.filename = source;
  loaded.paths = module.paths;
  const mocks = {
    '@kit.AbilityKit': { UIAbility }, '@kit.ArkUI': {}, '@kit.LocalizationKit': { i18n: { System: { getSystemLanguage: () => language } } },
    '@kit.BasicServicesKit': {}, '@kit.PerformanceAnalysisKit': { hiAppEvent: { configure() {} }, hilog: { warn: (...args) => warnings.push(args) } },
    '../platform/AppFaultObserver': { AppFaultObserver: class { start() {} foreground() {} background() {} dispose() {} } },
    '../platform/AppStorageRoots': { AppStorageRoots: { activate: () => ({filesDir:'/sandbox/files',runId:''}) } },
    '../platform/ShellLocalization': { configureShellLocalization: value => { resolver = value; } },
    'libphotocraft.so': {default:native}, 'BuildProfile': {default:{ DEVICE_TESTS: false }}
  };
  loaded.require = name => Object.hasOwn(mocks, name) ? mocks[name] : require(name);
  global.AppStorage = { setOrCreate: (...args) => foreground.push(args) };
  loaded._compile(ts.transpileModule(fs.readFileSync(source, 'utf8'), {
    compilerOptions: {module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}
  }).outputText, source);
  return { ability: new loaded.exports.default(), calls, foreground, warnings, native,
    setLanguage: value => { language = value; }, resource: key => resolver(key) };
}

test('cold start supplies system locale before the editor and binds resource lookup', () => {
  const h = abilityHarness();
  h.ability.onCreate({parameters:{}});
  assert.deepEqual(h.calls, ['zh-Hans-CN']);
  assert.equal(h.resource('shell_index_cancel'), 'resource:shell_index_cancel');
});

test('configuration changes and resume follow the new system language', () => {
  const h = abilityHarness();
  h.ability.onCreate({parameters:{}});
  h.setLanguage('zh-Hant-TW');
  h.ability.onConfigurationUpdate({language:'zh-Hant-TW'});
  h.ability.onBackground();
  h.setLanguage('en-US');
  h.ability.onForeground();
  assert.deepEqual(h.calls, ['zh-Hans-CN', 'zh-Hant-TW', 'en-US']);
  assert.deepEqual(h.foreground, [['photocraft.foreground',false],['photocraft.foreground',true]]);
});

test('unrelated configuration changes query the current system locale', () => {
  const h = abilityHarness();
  h.setLanguage('zh-Hant-HK');
  h.ability.onConfigurationUpdate({colorMode:0});
  assert.deepEqual(h.calls, ['zh-Hant-HK']);
});

test('native language errors are reported without stopping the editor lifecycle', () => {
  const h = abilityHarness();
  h.native.setSystemLanguage = () => false;
  h.native.lastError = () => 'render thread stopped';
  h.ability.onCreate({parameters:{}});
  assert.equal(h.warnings.length,1);
  assert.equal(h.warnings[0].at(-1),'render thread stopped');
});
