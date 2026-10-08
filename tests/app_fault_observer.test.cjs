'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const syncFs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const Module = require('node:module');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH ||
  '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');
const source = path.resolve(__dirname, '../harmonyos/entry/src/main/ets/platform/AppFaultObserver.ets');
const abilitySource = path.resolve(__dirname, '../harmonyos/entry/src/main/ets/entryability/EntryAbility.ets');

function fixture(fileIo = {}) {
  const added = [], removed = [], written = [];
  let addFailure, removeFailure, holder = {};
  const hiAppEvent = {domain: {OS: 'OS'}, event: {APP_CRASH: 'APP_CRASH', APP_FREEZE: 'APP_FREEZE'}, EventType: {FAULT: 1},
    addWatcher(watcher) {added.push(watcher); if (addFailure) throw addFailure; return holder;},
    removeWatcher(watcher) {removed.push(watcher); if (removeFailure) throw removeFailure;}};
  const loaded = new Module(source, module);
  loaded.filename = source;
  loaded.require = name => {
    if (name === '@kit.CoreFileKit') return {fileIo};
    if (name === '@kit.PerformanceAnalysisKit') return {hiAppEvent};
    throw new Error('Unexpected production import: ' + name);
  };
  loaded._compile(ts.transpileModule(syncFs.readFileSync(source, 'utf8'), {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
  }).outputText, source);
  const {AppFaultObserver} = loaded.exports;
  const storage = {async write(slot, text) {written.push({slot, text, record: JSON.parse(text)});}};
  return {AppFaultObserver, storage, written, added, removed, hiAppEvent,
    create: store => new AppFaultObserver('/private-app/files', store === undefined ? storage : store),
    emit(groups, domain = 'OS', index = added.length - 1) {added[index].onReceive(domain, groups);},
    setAddFailure: error => {addFailure = error;}, setRemoveFailure: error => {removeFailure = error;},
    setHolder: value => {holder = value;}};
}

function info(time = 1, options = {}) {
  return {domain: 'OS', name: 'APP_CRASH', eventType: 1, params: {
    bundle_name: 'moe.kiwi.photocraft', time, crash_type: 'NativeCrash', foreground: false, pid: 37,
    exception: {signal: {signo: 11, code: 1}}, ...options
  }};
}
function group(...infos) {return [{name: infos[0]?.name || 'APP_CRASH', appEventInfos: infos}];}
async function drain(observer) {
  // Real fsync/open/rename work is asynchronous. Event-loop turn counts can
  // expire before the OS I/O queue settles, even in a serial test run.
  const deadline = performance.now() + 10000;
  while (observer.journal.writing && performance.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 5));
  }
  assert.equal(observer.journal.writing, false, 'production bounded writer did not settle within 10 seconds');
}

test('watcher uses only own OS FAULT crash/freeze filters, starts once and removes once', async () => {
  const f = fixture(), observer = f.create(); observer.start(); observer.start();
  assert.equal(f.added.length, 1);
  assert.match(f.added[0].name, /^[A-Za-z][A-Za-z0-9_]{0,30}[A-Za-z0-9]$/);
  assert.deepEqual(f.added[0].appEventFilters, [{domain: 'OS', eventTypes: [1], names: ['APP_CRASH', 'APP_FREEZE']}]);
  observer.foreground(); observer.background(); observer.dispose(); observer.dispose();
  f.emit(group(info())); await drain(observer);
  assert.equal(f.removed.length, 1); assert.equal(f.removed[0], f.added[0]);
  assert.deepEqual(f.written.map(x => x.record.kind), ['created', 'watcherReady', 'foreground', 'background', 'destroyed']);
});

test('subscription failure/null result and removal failure never escape lifecycle', async () => {
  for (const failure of ['throw', 'null', 'remove']) {
    const f = fixture();
    if (failure === 'throw') f.setAddFailure(new Error('unavailable'));
    if (failure === 'null') f.setHolder(null);
    if (failure === 'remove') f.setRemoveFailure(new Error('unavailable'));
    const observer = f.create(); assert.doesNotThrow(() => {observer.start(); observer.dispose();});
    await drain(observer);
    assert.equal(f.written[1].record.kind, failure === 'remove' ? 'watcherReady' : 'watcherUnavailable');
  }
});

test('foreign domains, names, bundles, event types and malformed groups do not enter records', async () => {
  const f = fixture(), observer = f.create(); observer.start();
  f.emit(group(info()), 'OTHER');
  f.emit(group({...info(), domain: 'OTHER'}));
  f.emit(group({...info(), eventType: 4}));
  f.emit([{name: 'APP_LAUNCH', appEventInfos: [info()]}]);
  f.emit(group(info(2, {bundle_name: 'foreign.bundle'})));
  f.emit(group({...info(), params: null})); f.emit(null);
  f.emit([{name: 'APP_CRASH', appEventInfos: null}]);
  f.emit([{name: 'APP_CRASH', appEventInfos: [null, {...info(), name: 'APP_FREEZE'}]}]);
  // A malicious getter is outside the SDK contract but must still not escape the callback.
  assert.doesNotThrow(() => f.emit([{get name() {throw new Error('invalid');}}]));
  await drain(observer); assert.equal(f.written.filter(x => ['crash', 'freeze'].includes(x.record.kind)).length, 0);
});

test('crash/freeze preserve only documented bounded primitives, never raw names, URI, stacks or messages', async () => {
  const f = fixture(), observer = f.create(); observer.start();
  const secret = 'content://private-provider/My Private Document.psd';
  f.emit(group(info(121, {process_life_time: 33, memory: {rss: 2048}, log_over_limit: true,
    exception: {message: secret, signal: {signo: 11, code: -6}, frames: [{file: secret}]},
    hilog: [secret], external_log: [secret], uuid: secret, process_name: secret})));
  f.emit(group({...info(122), name: 'APP_FREEZE', params: {time: 122, foreground: true, exception: {name: secret, message: secret}}}));
  await drain(observer);
  const faults = f.written.filter(x => ['crash', 'freeze'].includes(x.record.kind));
  assert.equal(faults.length, 2);
  assert.equal(faults[0].record.signalNumber, 11); assert.equal(faults[0].record.signalCode, -6);
  assert.equal(faults[0].record.rssKiB, 2048); assert.equal(faults[0].record.eventAtMs, 121);
  assert.equal(faults[1].record.foreground, true); assert.equal(faults[1].record.jsErrorType, '');
  assert.equal(JSON.stringify(f.written).includes(secret), false);
  for (const item of f.written) assert.ok(Buffer.byteLength(item.text) <= 2048);
});

test('invalid numeric/object metadata is omitted rather than coerced or serialized', async () => {
  const f = fixture(), observer = f.create(); observer.start();
  f.emit(group(info(1, {time: Infinity, pid: -1, foreground: 'yes', process_life_time: Number.MAX_SAFE_INTEGER + 1,
    memory: {rss: '2048'}, exception: {signal: {signo: 129, code: NaN}}, log_over_limit: 'true'})));
  f.emit(group(info(2, {crash_type: 'JsError', exception: {name: 'TypeError', message: 'private'}})));
  f.emit(group(info(3, {crash_type: 'JsError', exception: {name: 'private-file.psd'}})));
  await drain(observer);
  const faults = f.written.filter(x => x.record.kind === 'crash').map(x => x.record);
  for (const key of ['eventAtMs', 'pid', 'foreground', 'processLifeSeconds', 'rssKiB', 'signalNumber', 'signalCode', 'logOverLimit'])
    assert.equal(faults[0][key], null, key);
  assert.equal(faults[1].jsErrorType, 'TypeError'); assert.equal(faults[2].jsErrorType, '');
});

test('overlapping callbacks share one writer with at most 32 queued plus one active record', async () => {
  const f = fixture(); let release, active = 0, maximum = 0, writes = 0;
  const gate = new Promise(resolve => {release = resolve;});
  const observer = f.create({async write() {active++; maximum = Math.max(maximum, active); writes++; await gate; active--;}});
  observer.start();
  for (let batch = 0; batch < 100; batch++) f.emit(group(...Array.from({length: 100}, (_, n) => info(batch * 100 + n + 1))));
  assert.equal(observer.journal.queue.length, 32); assert.equal(active, 1);
  release(); await drain(observer);
  assert.equal(maximum, 1); assert.equal(writes, 33);
  assert.ok(observer.journal.dropped > 0);
});

test('callback scan cap, deduplication and fixed ring slots keep storage and bookkeeping bounded', async () => {
  const f = fixture(), observer = f.create(); observer.start(); await drain(observer);
  f.emit(group(...Array.from({length: 200}, (_, n) => info(n + 1)))); await drain(observer);
  // Inspect limit is 64; queue bound can drop some of those independently.
  assert.ok(f.written.filter(x => x.record.kind === 'crash').every(x => x.record.eventAtMs <= 64));
  const before = f.written.length; f.emit(group(info(1))); await drain(observer); assert.equal(f.written.length, before);
  for (let n = 0; n < 100; n++) {f.emit(group(info(n + 1000))); observer.foreground(); await drain(observer);}
  assert.ok(new Set(f.written.map(x => x.slot)).size <= 24);
  assert.ok(observer.journal.recentFaultKeys.length <= 64);
});

test('new ability replaces old watcher without old onDestroy removing the new owner; storage stays serialized', async () => {
  const f = fixture(), first = f.create(); first.start();
  const second = f.create(); second.start(); first.dispose();
  assert.equal(f.added.length, 2); assert.equal(f.removed.length, 1);
  f.emit(group(info(1)), 'OS', 0); f.emit(group(info(2)), 'OS', 1);
  await drain(second);
  const faults = f.written.filter(x => x.record.kind === 'crash');
  assert.equal(faults.length, 1); assert.equal(faults[0].record.eventAtMs, 2);
  assert.equal(first.journal, second.journal);
  second.dispose(); assert.equal(f.removed.length, 2);
});

test('failed asynchronous storage drops diagnostics without stalling following records or throwing', async () => {
  const f = fixture(); let calls = 0;
  const observer = f.create({async write() {calls++; throw new Error('disk full with private path');}});
  observer.start(); f.emit(group(info())); observer.foreground(); observer.dispose();
  await drain(observer); assert.equal(calls, 5); assert.equal(observer.journal.queue.length, 0);
  assert.equal(observer.journal.dropped, 5); assert.equal(observer.journal.writing, false);
});

async function diskFixture(t, fail = '') {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'photocraft-fault-contract-'));
  t.after(() => fs.rm(directory, {recursive: true, force: true}));
  const handles = new Map(), trace = [];
  let serial = 0;
  const mode = {WRITE_ONLY: 1, CREATE: 64, TRUNC: 512, NOFOLLOW: 131072};
  const sdk = {OpenMode: mode,
    async lstat(p) {trace.push(['lstat', p]); return fs.lstat(p);},
    async access(p) {try {await fs.access(p); return true;} catch {return false;}},
    async mkdir(p) {if (fail === 'mkdir') throw new Error('injected'); await fs.mkdir(p);},
    async open(p, flags) {trace.push(['open', p]); assert.ok(flags & mode.NOFOLLOW);
      const handle = await fs.open(p, syncFs.constants.O_WRONLY | syncFs.constants.O_CREAT | syncFs.constants.O_TRUNC | syncFs.constants.O_NOFOLLOW, 0o600);
      const fd = ++serial; handles.set(fd, handle); return {fd};},
    async stat(fd) {return handles.get(fd).stat();},
    async write(fd, text) {if (fail === 'write') throw new Error('injected'); return (await handles.get(fd).write(text)).bytesWritten;},
    async fsync(fd) {if (fail === 'fsync') throw new Error('injected'); await handles.get(fd).sync();},
    async close(file) {await handles.get(file.fd).close(); handles.delete(file.fd);},
    async rename(a, b) {if (fail === 'rename') throw new Error('injected'); await fs.rename(a, b);}};
  return {directory, handles, trace, ...fixture(sdk)};
}

test('production SDK adapter writes bounded fixed-name private files over real sandbox fixtures', async t => {
  const f = await diskFixture(t), observer = new f.AppFaultObserver(f.directory);
  observer.start(); await drain(observer);
  for (let n = 0; n < 40; n++) {f.emit(group(info(n + 1))); observer.foreground(); await drain(observer);}
  observer.dispose(); await drain(observer);
  const root = path.join(f.directory, 'PhotoCraft', 'Diagnostics'), names = await fs.readdir(root);
  assert.ok(names.length <= 24); assert.equal(f.handles.size, 0);
  assert.ok(names.every(name => /^(fault-(0[0-9]|1[0-5])|lifecycle-0[0-7])\.json$/.test(name)));
  for (const name of names) assert.ok((await fs.stat(path.join(root, name))).size <= 2048);
  assert.ok(f.trace.every(([, p]) => p === f.directory || p.startsWith(f.directory + '/PhotoCraft')));
});

test('isolated device policy skips shared OS watcher IO while journaling lifecycle only beneath the run root', async t => {
  const f = await diskFixture(t);
  const filesRoot = path.join(f.directory, 'PhotoCraftTestRuns/policy-run/files');
  await fs.mkdir(filesRoot, {recursive: true});
  const observer = new f.AppFaultObserver(filesRoot, undefined, false);
  observer.start(); observer.start(); observer.foreground(); observer.background();
  observer.receive('OS', group(info())); observer.dispose(); observer.dispose();
  await drain(observer);
  assert.deepEqual(f.added, []); assert.deepEqual(f.removed, []);
  const root = path.join(filesRoot, 'PhotoCraft/Diagnostics');
  const records = await Promise.all((await fs.readdir(root)).map(async name => JSON.parse(await fs.readFile(path.join(root, name), 'utf8'))));
  assert.deepEqual(records.map(record => record.kind), ['created', 'watcherSkippedForDeviceTest', 'foreground', 'background', 'destroyed']);
  assert.ok(f.trace.every(([, value]) => value === filesRoot || value.startsWith(filesRoot + '/PhotoCraft')));
  assert.deepEqual(await fs.readdir(f.directory), ['PhotoCraftTestRuns']);
  assert.equal(f.handles.size, 0);
});

test('SDK storage mkdir/write/fsync/rename failures never break launch and always close opened descriptors', async t => {
  for (const failure of ['mkdir', 'write', 'fsync', 'rename']) {
    const f = await diskFixture(t, failure), observer = new f.AppFaultObserver(f.directory);
    assert.doesNotThrow(() => observer.start()); f.emit(group(info())); observer.dispose(); await drain(observer);
    assert.equal(f.handles.size, 0); assert.ok(observer.journal.dropped > 0);
    const root = path.join(f.directory, 'PhotoCraft', 'Diagnostics');
    if (await fs.access(root).then(() => true, () => false)) {
      const names = await fs.readdir(root); assert.ok(names.length <= 25);
      for (const name of names) assert.ok((await fs.lstat(path.join(root, name))).size <= 2048);
    }
  }
});

test('private directory and destination symlinks are rejected without touching their outside targets', async t => {
  for (const variant of ['directory', 'destination', 'temporary']) {
    const f = await diskFixture(t), outside = path.join(f.directory, 'outside'); await fs.mkdir(outside);
    const marker = path.join(outside, 'marker'); await fs.writeFile(marker, 'unchanged');
    if (variant === 'directory') await fs.symlink(outside, path.join(f.directory, 'PhotoCraft'));
    else {
      const root = path.join(f.directory, 'PhotoCraft', 'Diagnostics'); await fs.mkdir(root, {recursive: true});
      await fs.symlink(marker, path.join(root, variant === 'destination' ? 'lifecycle-00.json' : 'pending.json'));
    }
    const observer = new f.AppFaultObserver(f.directory); observer.start(); await drain(observer);
    assert.equal(await fs.readFile(marker, 'utf8'), 'unchanged'); assert.equal(f.handles.size, 0);
    assert.ok(observer.journal.dropped >= 1);
  }
});

test('EntryAbility lifecycle integrates production observer before editor loading, retains foreground storage and destroys watcher', async () => {
  const calls = [], appStorage = [], exports = {};
  const loaded = new Module(abilitySource, module); loaded.filename = abilitySource;
  loaded.require = name => {
    if (name === '@kit.AbilityKit') return {UIAbility: class {context = {filesDir: '/private-app/files',
      resourceManager: {getStringByNameSync: key => `resource:${key}`}};}};
    if (name === '../platform/ShellLocalization') return {configureShellLocalization: resolver => {
      assert.equal(resolver('fixture-key'), 'resource:fixture-key');
    }};
    if (name === '@kit.LocalizationKit') return {i18n: {System: {getSystemLanguage: () => 'zh-Hans-CN'}}};
    if (name === 'libphotocraft.so') return {default: {setSystemLanguage: language => {
      assert.equal(language, 'zh-Hans-CN'); return true;
    }}};
    if (name === 'BuildProfile') return {default: {DEVICE_TESTS: false}};
    if (name === '../platform/AppStorageRoots') {
      const rootsPath = path.join(path.dirname(source), 'AppStorageRoots.ets');
      const rootsModule = new Module(rootsPath, module); rootsModule.filename = rootsPath;
      rootsModule.require = () => ({});
      rootsModule._compile(ts.transpileModule(syncFs.readFileSync(rootsPath, 'utf8'), {
        compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
      }).outputText, rootsPath);
      return rootsModule.exports;
    }
    if (name === '../platform/AppFaultObserver') return {AppFaultObserver: class {
      constructor(directory, storage, subscribeToOsFaults = true) {calls.push(['constructor', directory, subscribeToOsFaults]);}
      start() {calls.push('start');} foreground() {calls.push('foreground');}
      background() {calls.push('background');} dispose() {calls.push('dispose');}
    }};
    if (name === '@kit.PerformanceAnalysisKit') return {hilog: {error() {}, warn() {}}};
    return {};
  };
  const prior = global.AppStorage;
  global.AppStorage = {setOrCreate: (key, value) => appStorage.push([key, value])};
  try {
    loaded._compile(ts.transpileModule(syncFs.readFileSync(abilitySource, 'utf8'), {
      compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
    }).outputText, abilitySource);
    const ability = new loaded.exports.default();
    ability.onCreate(); ability.onWindowStageCreate({loadContent(page, callback) {calls.push(['load', page]); callback({code: 0});}});
    ability.onForeground(); ability.onBackground(); ability.onDestroy(); ability.onDestroy();
    assert.deepEqual(calls, [['constructor', '/private-app/files', true], 'start', ['load', 'pages/Index'], 'foreground', 'background', 'dispose']);
    assert.deepEqual(appStorage, [['photocraft.foreground', true], ['photocraft.foreground', false]]);
  } finally {global.AppStorage = prior;}
});

test('production diagnostics have no network, clipboard, picker, log output, raw payload serialization or data export entry point', () => {
  const text = syncFs.readFileSync(source, 'utf8');
  assert.equal(/\b(console|hilog|http|request|picker|fileShare|pasteboard)\s*[.(]/.test(text), false);
  assert.equal(/JSON\.stringify\((params|info|groups|domain)\)/.test(text), false);
  assert.deepEqual([...text.matchAll(/^export\s+(?:class|function|interface)\s+(\w+)/gm)].map(m => m[1]), ['AppFaultObserver']);
  assert.equal(/(getRecords|exportRecords|readText|copyFile|setEventConfig|configEventPolicy|setEventParam)\s*\(/.test(text), false);
});


test('EntryAbility disables OS subscription only for a validated isolated test-enabled Want', t => {
  const directory = syncFs.mkdtempSync(path.join(os.tmpdir(), 'photocraft-ability-policy-'));
  t.after(() => syncFs.rmSync(directory, {recursive: true, force: true}));
  for (const [enabled, requested, expected] of [[false, 'policy-run', true], [true, undefined, true], [true, 'policy-run', false]]) {
    const context = {filesDir: path.join(directory, `files-${enabled}-${requested}`), cacheDir: path.join(directory, `cache-${enabled}-${requested}`),
      resourceManager: {getStringByNameSync: key => `resource:${key}`}};
    syncFs.mkdirSync(context.filesDir); syncFs.mkdirSync(context.cacheDir);
    const calls = [], eventConfigurations = [], order = [];
    const loaded = new Module(abilitySource, module); loaded.filename = abilitySource;
    loaded.require = name => {
      if (name === '@kit.AbilityKit') return {UIAbility: class {context = context;}};
      if (name === '../platform/ShellLocalization') return {configureShellLocalization: resolver => {
        assert.equal(resolver('fixture-key'), 'resource:fixture-key');
      }};
      if (name === '@kit.LocalizationKit') return {i18n: {System: {getSystemLanguage: () => 'zh-Hans-CN'}}};
      if (name === 'libphotocraft.so') return {default: {setSystemLanguage: language => {
        assert.equal(language, 'zh-Hans-CN'); return true;
      }}};
      if (name === 'BuildProfile') return {default: {DEVICE_TESTS: enabled}};
      if (name === '@kit.PerformanceAnalysisKit') return {hilog: {warn() {}}, hiAppEvent: {configure: config => {
        eventConfigurations.push(config); order.push('configure');
      }}};
      if (name === '../platform/AppStorageRoots') {
        const rootsPath = path.join(path.dirname(source), 'AppStorageRoots.ets');
        const rootsModule = new Module(rootsPath, module); rootsModule.filename = rootsPath;
        rootsModule.require = name => name === '@kit.CoreFileKit' ? {fileIo: {
          accessSync: syncFs.existsSync, mkdirSync: syncFs.mkdirSync, lstatSync: syncFs.lstatSync
        }} : {};
        rootsModule._compile(ts.transpileModule(syncFs.readFileSync(rootsPath, 'utf8'), {
          compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
        }).outputText, rootsPath);
        return rootsModule.exports;
      }
      if (name === '../platform/AppFaultObserver') return {AppFaultObserver: class {
        constructor(...args) {calls.push(args); order.push('observer');} start() {order.push('start');}
      }};
      return {};
    };
    loaded._compile(ts.transpileModule(syncFs.readFileSync(abilitySource, 'utf8'), {
      compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
    }).outputText, abilitySource);
    new loaded.exports.default().onCreate({parameters: requested === undefined ? {} : {'photocraft.testRunId': requested}});
    assert.equal(calls[0][2], expected);
    assert.equal(calls[0][0], expected ? context.filesDir : path.join(context.filesDir, 'PhotoCraftTestRuns/policy-run/files'));
    assert.deepEqual(eventConfigurations, expected ? [] : [{disable: true}]);
    assert.deepEqual(order, expected ? ['observer', 'start'] : ['configure', 'observer', 'start']);
  }
});

test('isolated HiAppEvent configuration errors preserve startup failure and no observer or UI is initialized', () => {
  const failure = new Error('official SDK configure failed');
  const calls = [];
  const loaded = new Module(abilitySource, module); loaded.filename = abilitySource;
  loaded.require = name => {
    if (name === '@kit.AbilityKit') return {UIAbility: class {context = {};}};
    if (name === 'BuildProfile') return {default: {DEVICE_TESTS: true}};
    if (name === '../platform/AppStorageRoots') return {AppStorageRoots: {activate: () => {
      calls.push('roots'); return {filesDir: '/isolated/files', runId: 'valid-run'};
    }}};
    if (name === '@kit.PerformanceAnalysisKit') return {hiAppEvent: {configure: config => {
      assert.deepEqual(config, {disable: true}); calls.push('configure'); throw failure;
    }}};
    if (name === '../platform/AppFaultObserver') return {AppFaultObserver: class {constructor() {calls.push('observer');}}};
    return {};
  };
  loaded._compile(ts.transpileModule(syncFs.readFileSync(abilitySource, 'utf8'), {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
  }).outputText, abilitySource);
  assert.throws(() => new loaded.exports.default().onCreate({parameters: {'photocraft.testRunId': 'valid-run'}}), error => error === failure);
  assert.deepEqual(calls, ['roots', 'configure']);
});
