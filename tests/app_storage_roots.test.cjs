'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const Module = require('node:module');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH ||
  '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');
const source = path.resolve(__dirname, '../harmonyos/entry/src/main/ets/platform/AppStorageRoots.ets');

function fixture(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'photocraft-test-roots-'));
  t.after(() => fs.rmSync(directory, {recursive: true, force: true}));
  const context = {filesDir: path.join(directory, 'files'), cacheDir: path.join(directory, 'cache')};
  fs.mkdirSync(context.filesDir); fs.mkdirSync(context.cacheDir);
  const loaded = new Module(source, module); loaded.filename = source;
  loaded.require = name => {
    if (name === '@kit.AbilityKit') return {};
    if (name === '@kit.CoreFileKit') return {fileIo: {
      accessSync: fs.existsSync, mkdirSync: fs.mkdirSync, lstatSync: fs.lstatSync
    }};
    throw new Error(`Unexpected import ${name}`);
  };
  loaded._compile(ts.transpileModule(fs.readFileSync(source, 'utf8'), {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
  }).outputText, source);
  return {directory, context, Roots: loaded.exports.AppStorageRoots};
}

test('disabled test build keeps real directories even when a Want supplies an escaping ID', t => {
  const {context, Roots} = fixture(t);
  const roots = Roots.activate(context, false, '../../ordinary');
  assert.equal(roots.filesDir, context.filesDir); assert.equal(roots.cacheDir, context.cacheDir);
  assert.equal(roots.runId, ''); assert.deepEqual(fs.readdirSync(context.filesDir), []);
});

test('enabled run creates only its files/cache namespace and leaves ordinary bytes untouched', t => {
  const {context, Roots} = fixture(t);
  const normal = path.join(context.filesDir, 'preferences.json');
  fs.writeFileSync(normal, 'ordinary preferences');
  const roots = Roots.activate(context, true, 'run_20261008-1');
  assert.equal(roots.filesDir, path.join(context.filesDir, 'PhotoCraftTestRuns/run_20261008-1/files'));
  assert.equal(roots.cacheDir, path.join(context.cacheDir, 'PhotoCraftTestRuns/run_20261008-1/cache'));
  fs.writeFileSync(path.join(roots.filesDir, 'preferences.json'), 'isolated preferences');
  assert.equal(fs.readFileSync(normal, 'utf8'), 'ordinary preferences');
  assert.equal(Roots.current(context), roots);
  assert.doesNotThrow(() => Roots.activate(context, true, 'run_20261008-1'));
  assert.throws(() => Roots.activate(context, true, 'other-run'), /cannot change/);
  assert.throws(() => Roots.activate(context, false), /cannot change/);
});

test('invalid and escaping IDs are rejected before any directory is created', t => {
  const {context, Roots} = fixture(t);
  for (const id of ['', '..', '../escape', '/absolute', 'dot.name', 'x/y', 'x\\y', 'x\0y', 'x'.repeat(65), '中文']) {
    assert.throws(() => Roots.activate(context, true, id), /invalid/);
    if (id.length) assert.throws(() => new Roots(context, id), /invalid/);
  }
  assert.deepEqual(fs.readdirSync(context.filesDir), []);
  assert.deepEqual(fs.readdirSync(context.cacheDir), []);
});

test('symlinks in the run ancestry are refused without writing through them', t => {
  const {directory, context, Roots} = fixture(t);
  const outside = path.join(directory, 'outside'); fs.mkdirSync(outside);
  fs.writeFileSync(path.join(outside, 'marker'), 'unchanged');
  fs.symlinkSync(outside, path.join(context.filesDir, 'PhotoCraftTestRuns'));
  assert.throws(() => Roots.activate(context, true, 'fresh-run'), /real directories/);
  assert.deepEqual(fs.readdirSync(outside), ['marker']);
  assert.equal(fs.readFileSync(path.join(outside, 'marker'), 'utf8'), 'unchanged');
});
