'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Module = require('node:module');
const os = require('node:os');
const crypto = require('node:crypto');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH ||
  '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');
const source = path.resolve(__dirname, '../harmonyos/entry/src/ohosTest/ets/support/DeviceHarness.ets');
const nativeSource = path.resolve(__dirname, '../apps/photocraft-ohos/src/device_tests.rs');

function fixture(parameters = {'-s photocraftTestRunId': 'wire-run'}, fileIo = {}, appContext = {}) {
  appContext.createModuleContext = moduleName => {assert.equal(moduleName, 'entry'); return appContext;};
  const submitted = [], polled = [], printed = [], shellCommands = [], capturedScreens = [];
  const monitors = [];
  const ability = {context: appContext.entryContext || appContext};
  const driver = {screenCap: async target => {capturedScreens.push(target); return true;}};
  // Read the actual strict Rust wire declaration so adding a TS-only request field fails here.
  const native = fs.readFileSync(nativeSource, 'utf8');
  const declaration = native.match(/#\[serde\(deny_unknown_fields\)\]\s*struct WireRequest\s*\{([^}]+)\}/);
  assert.ok(declaration, 'Rust request must remain a strict schema');
  const allowed = [...declaration[1].matchAll(/^\s*(\w+)\s*:/gm)].map(match => match[1]).sort();
  const bridge = {
    testSubmit(runId, serialized) {
      const request = JSON.parse(serialized);
      assert.deepEqual(Object.keys(request).sort(), allowed,
        'ArkTS JSON request does not match the deny_unknown_fields Rust wire schema');
      submitted.push({runId, request});
      return `ticket-${submitted.length}`;
    },
    testPoll(runId, ticket) { polled.push({runId, ticket}); return JSON.stringify({ok: true, result: {dialog: 9}}); },
    testSnapshot(runId) {return JSON.stringify({runId, runtime: {storageRoots: {
      files: '/application/base/haps/entry/files/PhotoCraftTestRuns/wire-run/files',
      cache: '/application/base/haps/entry/cache/PhotoCraftTestRuns/wire-run/cache'}}});},
    lastError() { return ''; }
  };
  const loaded = new Module(source, module); loaded.filename = source;
  let publication;
  loaded.testAppStorage = {get: () => publication};
  loaded.require = name => {
    if (name === './DeviceUiDriver') return {DeviceUiDriver: class {constructor() {this.backend = 'uitest-sdk';}},
      DeviceOn: {id: value => ({inWindow: bundle => ({id: value, bundle})})}};
    if (name === '@kit.TestKit') return {abilityDelegatorRegistry: {
      getArguments: () => ({parameters, bundleName: 'moe.kiwi.photocraft'}),
      getAbilityDelegator: () => ({executeShellCommand: async command => {shellCommands.push(command); return {exitCode: 0, stdResult: ''};}, print: async message => printed.push(message), startAbility: async () => {}, getCurrentTopAbility: async () => ability,
        addAbilityMonitor: async monitor => monitors.push(monitor), removeAbilityMonitor: async monitor => {assert.ok(monitors.includes(monitor)); monitors.splice(monitors.indexOf(monitor), 1);},
        getAppContext: () => {throw new Error('Delegator context proxy must not be used for Stage APIs');}})
    }, Driver: {create: () => driver}};
    if (name === '@kit.AbilityKit') return {application: {getApplicationContext: () => appContext, createModuleContext: (context, moduleName) => {
      assert.equal(moduleName, 'entry'); return context;
    }}};
    if (name === '@kit.CoreFileKit') return {fileIo};
    if (name === '@kit.ArkTS') return {util: {TextEncoder: class {encodeInto(text) {return new Uint8Array(Buffer.from(text));}}}};
    if (name === '@kit.CryptoArchitectureKit') return {cryptoFramework: {createMd: () => {
      const hash = crypto.createHash('sha256');
      return {update: async blob => hash.update(blob.data), digest: async () => ({data: new Uint8Array(hash.digest())})};
    }}};
    if (name === '@ohos.process') return {default: {pid: process.pid}};
    if (name === 'libphotocraft.so') return {default: bridge};
    if (name.endsWith('/AppStorageRoots')) return {AppStorageRoots: {validRunId: id => /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(id)}};
    if (name.startsWith('@kit.')) return {};
    throw new Error(`Unexpected import ${name}`);
  };
  loaded._compile('const AppStorage = module.testAppStorage;\n' + ts.transpileModule(fs.readFileSync(source, 'utf8'), {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
  }).outputText, source);
  return {harness: new loaded.exports.DeviceHarness(), submitted, polled, printed, shellCommands, capturedScreens, monitors, ability,
    setPublication: value => {publication = value;}, exports: loaded.exports};
}

test('device harness submits the exact Rust wire schema and correlates replies using native tickets', async () => {
  const f = fixture();
  assert.deepEqual(await f.harness.menu('file.new'), {dialog: 9});
  assert.deepEqual(await f.harness.call('ui.dialog.set', {dialog: 9, field: 'width', value: 16}), {dialog: 9});
  assert.deepEqual(await f.harness.call('ui.type', {text: '合成😀'}), {dialog: 9});
  assert.deepEqual(f.submitted, [
    {runId: 'wire-run', request: {method: 'ui.menu.invoke', params: {id: 'file.new'}}},
    {runId: 'wire-run', request: {method: 'ui.dialog.set', params: {dialog: 9, field: 'width', value: 16}}},
    {runId: 'wire-run', request: {method: 'ui.type', params: {text: '合成😀'}}}
  ]);
  assert.deepEqual(f.polled, [{runId: 'wire-run', ticket: 'ticket-1'}, {runId: 'wire-run', ticket: 'ticket-2'}, {runId: 'wire-run', ticket: 'ticket-3'}]);
});


test('real aa test -s argument names include the SDK prefix and retain strict run ID validation', () => {
  assert.equal(fixture({'-s photocraftTestRunId': 'prefixed-run'}).harness.runId, 'prefixed-run');
  assert.equal(fixture({photocraftTestRunId: 'legacy-run'}).harness.runId, 'legacy-run');
  assert.equal(fixture({'-s photocraftTestRunId': 'real-run', photocraftTestRunId: 'legacy-run'}).harness.runId, 'real-run');
  for (const parameters of [{}, {'-s photocraftTestRunId': ''}, {'-s photocraftTestRunId': '../escape'}]) {
    assert.throws(() => fixture(parameters), /Run with -s photocraftTestRunId/);
  }
});

test('clipboard diagnostics identify the authorization component by stable ID rather than translated title', async () => {
  const f = fixture();
  const selectors = [];
  f.harness.uiFacade = {backend: 'uitest-cli', findComponent: async selector => {selectors.push(selector); return {};}};
  f.harness.snapshot = () => ({runtime: {input: {nativeKeyEvents: 2}}, ui: {tool: 'Type',
    session: {active: 0, documents: [{name: 'fixture'}]}, document: {layers: []}}});
  await f.harness.reportInputDiagnostics('direct-chinese-clipboard');
  assert.deepEqual(selectors, [{id: 'photocraft-paste-authorize', bundle: 'moe.kiwi.photocraft'}]);
  const diagnostic = JSON.parse(f.printed[0].slice('PHOTOCRAFT_INPUT_DIAGNOSTIC '.length));
  assert.equal(diagnostic.stage, 'direct-chinese-clipboard');
  assert.equal(diagnostic.pasteDialogVisible, true);
});

test('the actual Chinese case accepts direct paste, clicks authorization once when needed, and checks arrival before clicking', async t => {
  const deviceSource = path.resolve(__dirname, '../harmonyos/entry/src/ohosTest/ets/test/PhotoCraftDevice.test.ets');
  const ast = ts.createSourceFile(deviceSource, fs.readFileSync(deviceSource, 'utf8'), ts.ScriptTarget.Latest, true);
  let body;
  function inspect(node) {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === 'it' &&
      node.arguments[0].text === 'synthetic_and_actual_chinese_text') body = node.arguments[2].arguments[2];
    ts.forEachChild(node, inspect);
  }
  inspect(ast); assert.ok(body);
  for (const route of ['direct', 'authorized', 'arrived-during-query']) await t.test(route, async () => {
    const diagnosticStages = [], selectors = [];
    let clicks = 0, text = '', nativeKeys = 10;
    const snapshot = () => ({runtime: {input: {nativeKeyEvents: nativeKeys, nativeTextEvents: 7}},
      ui: {document: {layers: [{text: {text}}]}}});
    const harness = {runId: 'wire-run', newDocument: async () => {}, snapshot,
      call: async (method, params) => {if (method === 'ui.type') text = params.text;},
      waitFor: async predicate => {assert.equal(predicate(snapshot()), true); return snapshot();},
      reportInputDiagnostics: async stage => diagnosticStages.push(stage),
      driver: {findComponent: async selector => {
        selectors.push(selector);
        if (route === 'arrived-during-query') text = '合成😀真机中文😀';
        return {click: async () => {clicks++; text = '合成😀真机中文😀';}};
      }, triggerKey: async key => assert.equal(key, 27)}};
    const environment = {harness, delay: async () => {}, KeyCode: {KEYCODE_ESCAPE: 27},
      requireCondition: (condition, message) => assert.ok(condition, message),
      expect: value => ({assertEqual: expected => assert.equal(value, expected), assertTrue: () => assert.equal(value, true)}),
      ON: {id: value => ({inWindow: bundle => ({id: value, bundle})})},
      abilityDelegatorRegistry: {getArguments: () => ({bundleName: 'moe.kiwi.photocraft'}),
        getAbilityDelegator: () => ({executeShellCommand: async command => {
          assert.equal(command, 'uitest uiInput text 真机中文😀'); nativeKeys += 2;
          if (route === 'direct') text = '合成😀真机中文😀';
          return {exitCode: 0, stdResult: ''};
        }})}};
    const loaded = new Module(deviceSource, module); loaded.filename = deviceSource; loaded.testEnvironment = environment;
    loaded._compile('const {harness, delay, KeyCode, requireCondition, expect, ON, abilityDelegatorRegistry} = module.testEnvironment;\n' +
      ts.transpileModule(`module.exports = ${body.getText(ast)};`, {
        compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
      }).outputText, deviceSource);
    await loaded.exports();
    assert.equal(clicks, route === 'authorized' ? 1 : 0);
    assert.equal(selectors.length, route === 'direct' ? 0 : 1);
    assert.ok(selectors.every(selector => selector.id === 'photocraft-paste-authorize' && selector.bundle === 'moe.kiwi.photocraft'));
    assert.deepEqual(diagnosticStages, ['uitest-chinese-clipboard',
      route === 'authorized' ? 'authorized-chinese-clipboard' : 'direct-chinese-clipboard']);
  });
});


test('ordinary-storage evidence identifies exact added, changed and removed files without revealing document names', async t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'photocraft-fingerprint-'));
  t.after(() => fs.rmSync(directory, {recursive: true, force: true}));
  const context = {filesDir: path.join(directory, 'files'), cacheDir: path.join(directory, 'cache')};
  fs.mkdirSync(context.filesDir); fs.mkdirSync(context.cacheDir);
  fs.mkdirSync(path.join(directory, 'haps/entry/files'), {recursive: true});
  fs.mkdirSync(path.join(directory, 'haps/entry/cache'), {recursive: true});
  fs.mkdirSync(path.join(context.filesDir, 'PhotoCraft/Documents'), {recursive: true});
  fs.writeFileSync(path.join(context.filesDir, 'PhotoCraft/Documents/private-project.pcraft'), 'before bytes');
  fs.writeFileSync(path.join(context.cacheDir, 'removed.cache'), 'gone');
  fs.mkdirSync(path.join(context.filesDir, 'Documents'));
  fs.writeFileSync(path.join(context.filesDir, 'Documents/root-private-project.pcraft'), 'root private before');
  fs.mkdirSync(path.join(context.cacheDir, 'Clipboard'));
  fs.writeFileSync(path.join(context.cacheDir, 'Clipboard/root-private-clipboard.png'), 'private cached bytes');
  const io = {OpenMode: {READ_ONLY: fs.constants.O_RDONLY, NOFOLLOW: fs.constants.O_NOFOLLOW},
    listFile: async value => fs.readdirSync(value), lstat: async value => fs.lstatSync(value),
    open: async (value, flags) => ({fd: fs.openSync(value, flags)}), close: async handle => fs.closeSync(handle.fd),
    read: async (fd, buffer, options = {}) => fs.readSync(fd, Buffer.from(buffer), 0, buffer.byteLength, options.offset ?? null)};
  const f = fixture(undefined, io);
  const before = await f.exports.fingerprintOrdinaryStorage(context);
  await f.exports.reportOrdinaryCacheInventory(before, 'wire-run', 'PhotoCraftCore');
  assert.equal(f.printed.join('\n').includes('root-private-clipboard.png'), false);
  fs.writeFileSync(path.join(context.filesDir, 'PhotoCraft/Documents/private-project.pcraft'), 'after bytes');
  fs.writeFileSync(path.join(context.filesDir, 'Documents/root-private-project.pcraft'), 'root private after');
  fs.unlinkSync(path.join(context.cacheDir, 'removed.cache'));
  fs.writeFileSync(path.join(context.cacheDir, 'shader-cache.bin'), Buffer.alloc(16864, 7));
  const after = await f.exports.reportStorageIsolation(context, before, 'wire-run', 'PhotoCraftCore');
  assert.notEqual(after.digest, before.digest);
  const line = f.printed.find(value => value.startsWith('PHOTOCRAFT_ISOLATION '));
  assert.ok(line); assert.equal(line.includes('private-project.pcraft'), false);
  assert.equal(line.includes('root-private-project.pcraft'), false);
  assert.equal(line.includes('before bytes'), false); assert.equal(line.includes('after bytes'), false);
  const evidence = JSON.parse(line.slice('PHOTOCRAFT_ISOLATION '.length));
  assert.equal(evidence.unchanged, false);
  const added = evidence.differences.find(value => value.relativePath === 'shader-cache.bin');
  assert.equal(added.change, 'added'); assert.equal(added.root, 'application.cache'); assert.equal(added.afterSize, 16864);
  assert.equal(added.afterHash, crypto.createHash('sha256').update(Buffer.alloc(16864, 7)).digest('hex'));
  assert.ok(evidence.differences.some(value => value.relativePath === 'removed.cache' && value.change === 'removed'));
  assert.ok(evidence.differences.some(value => value.relativePath === '<private-name-redacted>' && value.change === 'changed' && value.beforeHash !== value.afterHash));
});


test('fingerprinting covers distinct application and entry module roots, detects module writes, and ignores only run namespaces', async t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'photocraft-module-fingerprint-'));
  t.after(() => fs.rmSync(directory, {recursive: true, force: true}));
  const entry = {filesDir: path.join(directory, 'haps/entry/files'), cacheDir: path.join(directory, 'haps/entry/cache')};
  const context = {filesDir: path.join(directory, 'files'), cacheDir: path.join(directory, 'cache'), entryContext: entry};
  for (const root of [context.filesDir, context.cacheDir, entry.filesDir, entry.cacheDir]) fs.mkdirSync(root, {recursive: true});
  fs.writeFileSync(path.join(context.filesDir, 'normal.json'), 'app');
  fs.writeFileSync(path.join(entry.filesDir, 'normal.pcraft'), 'before');
  fs.writeFileSync(path.join(entry.cacheDir, 'normal.cache'), 'cache');
  const io = {OpenMode: {READ_ONLY: fs.constants.O_RDONLY, NOFOLLOW: fs.constants.O_NOFOLLOW},
    listFile: async value => fs.readdirSync(value), lstat: async value => fs.lstatSync(value),
    open: async (value, flags) => ({fd: fs.openSync(value, flags)}), close: async handle => fs.closeSync(handle.fd),
    read: async (fd, buffer, options = {}) => fs.readSync(fd, Buffer.from(buffer), 0, buffer.byteLength, options.offset ?? null)};
  const f = fixture(undefined, io, context);
  const before = await f.exports.fingerprintOrdinaryStorage(context);
  assert.equal(before.roots.length, 4);
  assert.deepEqual(before.roots.flatMap(root => root.labels).sort(), ['application.cache', 'application.files', 'entry.cache', 'entry.files']);
  assert.equal(before.entries, before.roots.reduce((count, root) => count + root.entries, 0));
  assert.equal(before.bytes, before.roots.reduce((count, root) => count + root.bytes, 0));
  for (const root of [context.filesDir, context.cacheDir, entry.filesDir, entry.cacheDir]) {
    fs.mkdirSync(path.join(root, 'PhotoCraftTestRuns/run/files'), {recursive: true});
    fs.writeFileSync(path.join(root, 'PhotoCraftTestRuns/run/files/fixture.pcraft'), 'isolated test bytes');
  }
  assert.equal((await f.exports.fingerprintOrdinaryStorage(context)).digest, before.digest);
  fs.writeFileSync(path.join(entry.filesDir, 'normal.pcraft'), 'after!');
  const after = await f.exports.reportStorageIsolation(context, before, 'wire-run', 'PhotoCraftCore');
  assert.notEqual(after.digest, before.digest);
  for (const original of before.roots) {
    const current = after.roots.find(root => root.labels[0] === original.labels[0]);
    assert.equal(current.pathDigest, original.pathDigest);
    assert.equal(current.digest === original.digest, original.labels[0] !== 'entry.files');
  }
  const evidence = JSON.parse(f.printed.find(line => line.startsWith('PHOTOCRAFT_ISOLATION ')).slice('PHOTOCRAFT_ISOLATION '.length));
  assert.ok(evidence.differences.some(item => item.root === 'entry.files' && item.change === 'changed'));
});

test('native startup accepts only actual entry module test roots and rejects application-level roots', async () => {
  const context = {filesDir: '/application/base/files', cacheDir: '/application/base/cache',
    entryContext: {filesDir: '/application/base/haps/entry/files', cacheDir: '/application/base/haps/entry/cache'}};
  const io = {lstat: async () => ({isDirectory: () => true, isSymbolicLink: () => false})};
  const f = fixture(undefined, io, context);
  const runtime = {frames: 2, uiPresented: true, storageRoots: {
    files: '/application/base/haps/entry/files/PhotoCraftTestRuns/wire-run/files',
    cache: '/application/base/haps/entry/cache/PhotoCraftTestRuns/wire-run/cache'}};
  f.harness.snapshot = () => ({runtime});
  await f.harness.start();
  runtime.storageRoots.files = '/application/base/files/PhotoCraftTestRuns/wire-run/files';
  await assert.rejects(f.harness.start(), /Actual native storage roots/);
  context.entryContext.filesDir = '/application/base/files';
  await assert.rejects(f.harness.start(), /Real EntryAbility context/);
});


test('prelaunch root discovery refuses unsafe layouts, absent directories and symbolic links without creating them', async t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'photocraft-root-preflight-'));
  t.after(() => fs.rmSync(directory, {recursive: true, force: true}));
  const context = {filesDir: path.join(directory, 'files'), cacheDir: path.join(directory, 'cache')};
  for (const root of [context.filesDir, context.cacheDir, path.join(directory, 'haps/entry/files')]) {
    fs.mkdirSync(root, {recursive: true});
  }
  const f = fixture(undefined, {lstat: async value => fs.lstatSync(value)}, context);
  await assert.rejects(f.exports.entryStorageDirectories(context), {code: 'ENOENT'});
  assert.equal(fs.existsSync(path.join(directory, 'haps/entry/cache')), false);
  fs.symlinkSync(context.cacheDir, path.join(directory, 'haps/entry/cache'));
  await assert.rejects(f.exports.entryStorageDirectories(context), /real sandbox directories/);
  await assert.rejects(f.exports.entryStorageDirectories({...context, cacheDir: '/another/cache'}), /safe sandbox base/);
  await assert.rejects(f.exports.entryStorageDirectories({...context, filesDir: `${directory}/../files`}), /safe sandbox base/);
});


test('failure evidence is captured during the failure in a fresh owned path and scopes layout to the tested bundle', async () => {
  const f = fixture();
  await f.harness.initializeDriver();
  await f.harness.captureFailureEvidence('unicode callback failed');
  await f.harness.captureFailureEvidence('duplicate immediate catch');
  assert.equal(f.shellCommands.length, 1);
  assert.equal(f.capturedScreens.length, 1);
  const marker = f.printed.find(value => value.startsWith('PHOTOCRAFT_FAILURE_EVIDENCE '));
  const evidence = JSON.parse(marker.slice('PHOTOCRAFT_FAILURE_EVIDENCE '.length));
  assert.equal(evidence.runId, 'wire-run'); assert.equal(evidence.suite, 'PhotoCraftCore');
  assert.equal(evidence.label, 'unicode callback failed');
  assert.match(evidence.screenPath, /^\/data\/local\/tmp\/PhotoCraftTest-wire-run-[0-9]+-failure\.png$/);
  assert.equal(evidence.layoutPath, evidence.screenPath.replace(/\.png$/, '.json'));
  assert.equal(f.shellCommands[0], `uitest dumpLayout -b moe.kiwi.photocraft -p ${evidence.layoutPath}`);
  assert.equal(f.capturedScreens[0], evidence.screenPath);
});

test('the case guard captures failures, preserves the original error and leaves successful cases untouched', async () => {
  const f = fixture();
  await f.harness.initializeDriver();
  await f.exports.guardedCase(() => f.harness, 'success', async () => {})();
  assert.equal(f.capturedScreens.length, 0);
  const failure = new Error('actual provider failure');
  await assert.rejects(f.exports.guardedCase(() => f.harness, 'save failed', async () => {throw failure;})(),
    error => error === failure);
  assert.equal(f.capturedScreens.length, 1);
  f.harness.captureFailureEvidence = async () => {throw new Error('capture unavailable');};
  await assert.rejects(f.exports.guardedCase(() => f.harness, 'close failed', async () => {throw failure;})(),
    error => error === failure);
  const files = ['PhotoCraftDevice.test.ets', 'PhotoCraftPhases.test.ets'];
  let cases = 0;
  for (const file of files) {
    const sourcePath = path.resolve(__dirname, '../harmonyos/entry/src/ohosTest/ets/test', file);
    const ast = ts.createSourceFile(sourcePath, fs.readFileSync(sourcePath, 'utf8'), ts.ScriptTarget.Latest, true);
    function inspect(node) {
      if (ts.isCallExpression(node) && node.expression.getText(ast) === 'it') {
        cases++;
        assert.ok(ts.isCallExpression(node.arguments[2]));
        assert.equal(node.arguments[2].expression.getText(ast), 'guardedCase',
          `${file} ${node.arguments[0].getText(ast)} lacks capture during failure`);
      }
      ts.forEachChild(node, inspect);
    }
    inspect(ast);
  }
  assert.equal(cases, 15);
});


test('independent external replacement writes only two distinct genuine owned picker URIs and fsyncs exact fixture bytes', async t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'photocraft-external-replacement-'));
  t.after(() => fs.rmSync(directory, {recursive: true, force: true}));
  const folderUri = 'file://docs/storage/Users/currentUser/Desktop/PhotoCraftTest-wire-run/';
  const sourceUri = `${folderUri}replacement-wire-run.pcraft`, targetUri = `${folderUri}recent-wire-run.pcraft`;
  const sourcePath = path.join(directory, 'source.pcraft'), targetPath = path.join(directory, 'target.pcraft');
  const replacement = Buffer.concat([Buffer.from([80, 75, 3, 4]), Buffer.from('black external fixture')]);
  fs.writeFileSync(sourcePath, replacement); fs.writeFileSync(targetPath, 'previous white bytes longer than fixture');
  const opened = [], synced = [];
  const paths = new Map([[sourceUri, sourcePath], [targetUri, targetPath]]);
  const io = {OpenMode: {READ_ONLY: fs.constants.O_RDONLY, WRITE_ONLY: fs.constants.O_WRONLY, TRUNC: fs.constants.O_TRUNC},
    open: async (uri, flags) => {assert.ok(paths.has(uri), 'URI was never genuinely granted'); opened.push({uri, flags}); return {fd: fs.openSync(paths.get(uri), flags)};},
    close: async handle => fs.closeSync(handle.fd), stat: async fd => fs.fstatSync(fd),
    read: async (fd, bytes, options = {}) => fs.readSync(fd, Buffer.from(bytes), 0, bytes.byteLength, options.offset ?? null),
    write: async (fd, bytes) => fs.writeSync(fd, Buffer.from(bytes)), fsync: async fd => {synced.push(fd); fs.fsyncSync(fd);}};
  const f = fixture(undefined, io);
  f.setPublication({runId: 'wire-run', filename: 'replacement-wire-run.pcraft', uri: sourceUri});
  await f.harness.publishedDigest('replacement-wire-run.pcraft');
  f.setPublication({runId: 'wire-run', filename: 'recent-wire-run.pcraft', uri: targetUri});
  await f.harness.publishedDigest('recent-wire-run.pcraft');
  opened.length = 0;
  await f.harness.replaceOwnedPublishedFile('replacement-wire-run.pcraft', 'recent-wire-run.pcraft');
  assert.deepEqual(fs.readFileSync(targetPath), replacement); assert.deepEqual(fs.readFileSync(sourcePath), replacement);
  assert.deepEqual(opened.map(value => value.uri), [sourceUri, targetUri]);
  assert.equal(opened[0].flags, fs.constants.O_RDONLY); assert.equal(opened[1].flags, fs.constants.O_WRONLY | fs.constants.O_TRUNC);
  assert.equal(synced.length, 1);
  opened.length = 0;
  await assert.rejects(f.harness.replaceOwnedPublishedFile('recent-wire-run.pcraft', 'recent-wire-run.pcraft'), /distinct owned/);
  await assert.rejects(f.harness.replaceOwnedPublishedFile('replacement-wire-run.pcraft', '../escape.pcraft'), /distinct owned/);
  await assert.rejects(f.harness.replaceOwnedPublishedFile('replacement-wire-run.pcraft', 'ungranted.pcraft'), /Missing genuine/);
  f.setPublication({runId: 'wire-run', filename: 'foreign.pcraft', uri: 'file://docs/storage/Users/currentUser/Desktop/foreign.pcraft'});
  await assert.rejects(f.harness.replaceOwnedPublishedFile('replacement-wire-run.pcraft', 'foreign.pcraft'), /verified owned test folder/);
  assert.equal(opened.length, 0, 'invalid authority must fail before any file is opened');
});

test('runtime snapshot and predicate failures are preserved; initialization tolerance is limited to snapshot readiness', async () => {
  const f = fixture();
  const snapshotError = new Error('native transport rejected this run');
  f.harness.snapshot = () => {throw snapshotError;};
  await assert.rejects(f.harness.waitFor(() => true, 'wrong timeout', 10000), error => error === snapshotError);
  let attempts = 0;
  const snapshot = {runtime: {frames: 2}};
  f.harness.snapshot = () => {if (++attempts === 1) throw snapshotError; return snapshot;};
  assert.equal(await f.harness.waitFor(value => value.runtime.frames === 2, 'start deadline', 1000, true), snapshot);
  const predicateError = new Error('original production status failure');
  for (const allowInitialization of [false, true]) {
    await assert.rejects(f.harness.waitFor(() => {throw predicateError;}, 'wrong timeout', 10000, allowInitialization),
      error => error === predicateError);
  }
});

test('a new failed or cancelled production file completion fails immediately with its original status error', async () => {
  const f = fixture();
  for (const [result, accepted] of [['error', true], ['cancel', true], ['success', false]]) {
    f.harness.snapshot = () => ({runtime: {lastFileCompletion: {id: 8, result, accepted, displayName: 'owned.pcraft'},
      fileOperationPending: false}, ui: {statusError: 'Provider denied the original write'}});
    await assert.rejects(f.harness.awaitFileSuccess('owned.pcraft', 7),
      new RegExp(`Original file request 8 failed \\(${result}.*Provider denied`));
  }
  let attempts = 0;
  f.harness.snapshot = () => ({runtime: {lastFileCompletion: {id: ++attempts === 1 ? 7 : 8,
    result: attempts === 1 ? 'cancel' : 'success', accepted: true, displayName: 'owned.pcraft'},
    fileOperationPending: attempts === 1}, ui: {statusError: null}});
  assert.equal((await f.harness.awaitFileSuccess('owned.pcraft', 7)).runtime.lastFileCompletion.id, 8);
});

test('close proof accepts destroy events only for the exact real ability and context; background or foreign destruction cannot pass', async () => {
  const appContext = {filesDir: '/application/base/files', cacheDir: '/application/base/cache',
    entryContext: {filesDir: '/application/base/haps/entry/files', cacheDir: '/application/base/haps/entry/cache'}};
  const f = fixture(undefined, {lstat: async () => ({isDirectory: () => true, isSymbolicLink: () => false})}, appContext);
  await f.harness.armCloseMonitor();
  assert.equal(f.monitors.length, 1);
  const monitor = f.monitors[0];
  assert.equal(monitor.abilityName, 'EntryAbility'); assert.equal(monitor.moduleName, 'entry');
  const foreign = {context: appContext.entryContext};
  monitor.onWindowStageDestroy(foreign); monitor.onAbilityDestroy(foreign);
  f.harness.assertCloseWasCancelled();
  monitor.onWindowStageDestroy(f.ability);
  assert.throws(() => f.harness.assertCloseWasCancelled(), /destroyed/);
  monitor.onAbilityDestroy(f.ability);
  assert.deepEqual(await f.harness.awaitCloseDestruction(), {windowStageDestroyed: true, abilityDestroyed: true});
  await f.harness.disarmCloseMonitor(); assert.equal(f.monitors.length, 0);
  await f.harness.armCloseMonitor();
  const original = new Error('actual close failure');
  f.harness.captureFailureEvidence = async () => {};
  await assert.rejects(f.harness.guard('close failure', async () => {throw original;}), error => error === original);
  assert.equal(f.monitors.length, 0, 'failure must release the exact registered monitor');
});
