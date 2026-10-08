'use strict';

// Run the production DocumentBridge and SDK storage adapter over real temporary files.
// The SDK shim enforces its documented mkdtemp template and FD contracts; this is
// not a device/provider substitute. The libc probe independently checks the template ABI.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const syncFs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const Module = require('node:module');
const { spawnSync } = require('node:child_process');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH ||
  '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');
const platformRoot = path.resolve(__dirname, '../harmonyos/entry/src/main/ets/platform');
const URI = 'content://fixture/opaque-granted-file';
const ORIGINAL = Buffer.from('previous saved bytes with a long tail');
const ENCODED = Buffer.from('8BPS new encoded document');

function loadProduction(entry, kits) {
  const modules = new Map();
  function load(filename) {
    if (modules.has(filename)) return modules.get(filename).exports;
    const loaded = new Module(filename, module);
    modules.set(filename, loaded);
    loaded.filename = filename;
    loaded.require = specifier => {
      if (kits[specifier]) return kits[specifier];
      if (specifier.startsWith('./')) return load(path.resolve(path.dirname(filename), specifier + '.ets'));
      throw new Error('Unexpected production import ' + specifier);
    };
    loaded._compile(ts.transpileModule(syncFs.readFileSync(filename, 'utf8'), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 }
    }).outputText, filename);
    return loaded.exports;
  }
  return load(path.join(platformRoot, entry + '.ets'));
}

async function fixture(t, fault = '', original = ORIGINAL) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'photocraft-sdk-publication-'));
  const filesDir = path.join(directory, 'files');
  await fs.mkdir(filesDir);
  const target = path.join(directory, 'opaque-provider-target');
  const stage = path.join(filesDir, 'capture.psd');
  await fs.writeFile(target, original);
  await fs.writeFile(stage, ENCODED);
  const handles = new Map();
  const logs = [];
  const trace = [];
  const templates = [];
  const permission = { granted: false, persisted: 0, activated: 0 };
  let pendingFault = fault;
  const error = () => Object.assign(new Error('Invalid argument'), { code: 13900020 });
  const hit = name => {
    trace.push(name);
    if (pendingFault === name) { pendingFault = ''; throw error(); }
  };
  const modes = { READ_ONLY: 0, WRITE_ONLY: 1, CREATE: 64, TRUNC: 512, NOFOLLOW: 0x100000 };
  const fileIo = {
    OpenMode: modes,
    async open(location, mode) {
      const actual = location === URI ? target : location;
      const write = (mode & modes.WRITE_ONLY) !== 0;
      trace.push(write && location === URI ? 'destination-open-write' : 'open');
      let flags = write ? syncFs.constants.O_WRONLY : syncFs.constants.O_RDONLY;
      if (mode & modes.CREATE) flags |= syncFs.constants.O_CREAT;
      if (mode & modes.TRUNC) flags |= syncFs.constants.O_TRUNC;
      if (mode & modes.NOFOLLOW) flags |= syncFs.constants.O_NOFOLLOW;
      const handle = await fs.open(actual, flags);
      handles.set(handle.fd, { handle, destination: location === URI });
      return { fd: handle.fd };
    },
    async close(file) {
      const fd = typeof file === 'number' ? file : file.fd;
      await handles.get(fd).handle.close();
      handles.delete(fd);
    },
    async copyFile(source, destination) {
      const src = handles.get(source).handle;
      const dst = handles.get(destination);
      await dst.handle.truncate(0);
      const bytes = Buffer.alloc(Number((await src.stat()).size));
      await src.read(bytes, 0, bytes.length, 0);
      if (dst.destination && pendingFault === 'destination-copy') {
        await dst.handle.write(bytes.subarray(0, 3), 0, Math.min(bytes.length, 3), 0);
        hit('destination-copy');
      }
      hit(dst.destination ? 'destination-copy' : 'backup-copy');
      await dst.handle.write(bytes, 0, bytes.length, 0);
    },
    async fsync(fd) {
      const handle = handles.get(fd).handle;
      hit((await handle.stat()).isDirectory() ? 'directory-fsync' : 'file-fsync');
      await handle.sync();
    },
    lseek(fd, offset) { assert.equal(offset, 0); assert(handles.has(fd)); hit('seek-preflight'); return 0; },
    async truncate(fd, len) { assert.equal(len, 0); await handles.get(fd).handle.truncate(0); },
    async write(fd, bytes) { return (await handles.get(fd).handle.write(Buffer.from(bytes))).bytesWritten; },
    async mkdir(location, recursive) { await fs.mkdir(location, { recursive }); },
    async lstat(location) { return fs.lstat(location); },
    async mkdtemp(template) {
      templates.push(template);
      if (!template.endsWith('XXXXXX')) throw error();
      // Node appends the six random characters itself; CoreFileKit/libuv replaces them.
      return fs.mkdtemp(template.slice(0, -6));
    },
    async rename(source, destination) { await fs.rename(source, destination); },
    async access(location) { try { await fs.access(location); return true; } catch { return false; } },
    async unlink(location) { await fs.unlink(location); },
    async listFile(location) { return fs.readdir(location); },
    async readText(location) { return fs.readFile(location, 'utf8'); },
    async stat(location) { return fs.stat(location); },
    async rmdir(location) { await fs.rmdir(location); }
  };
  class Encoder { encodeInto(text) { return new TextEncoder().encode(text); } }
  class FileUri { constructor(uri) { assert.equal(uri, URI); this.name = 'selected-name.psd'; } }
  const kits = {
    '@kit.AbilityKit': {},
    '@kit.BasicServicesKit': {},
    '@kit.ArkTS': { util: { TextEncoder: Encoder } },
    '@kit.CoreFileKit': { fileIo, fileUri: { FileUri }, fileShare: {
      OperationMode: { READ_MODE: 1, WRITE_MODE: 2 },
      checkPersistentPermission: async () => [permission.granted],
      persistPermission: async () => { permission.persisted++; },
      activatePermission: async () => { permission.activated++; }
    }, picker: {} },
    '@kit.PerformanceAnalysisKit': { hilog: {
      error: (...args) => logs.push(args), warn: (...args) => logs.push(args)
    } }
  };
  const previousCapability = global.canIUse;
  global.canIUse = () => true;
  const { DocumentBridge } = loadProduction('DocumentBridge', kits);
  const bridge = new DocumentBridge({ filesDir, cacheDir: path.join(directory, 'cache') });
  t.after(async () => {
    global.canIUse = previousCapability;
    await Promise.allSettled([...handles.values()].map(record => record.handle.close()));
    await fs.rm(directory, { recursive: true, force: true });
  });
  return { directory, filesDir, target, stage, bridge, handles, logs, trace, templates, permission, DocumentBridge };
}

test('Recent refreshes the saved external source and preserves its old cache', async t => {
  const f = await fixture(t);
  const previous = path.join(f.filesDir,'PhotoCraft/Documents/staging/41/Old.psd');
  await fs.mkdir(path.dirname(previous),{recursive:true});
  await fs.writeFile(previous,ENCODED);
  await f.bridge.bindSource(previous,URI);
  f.permission.granted = true;
  const latest = Buffer.from('external file edited after PhotoCraft closed');
  await fs.writeFile(f.target,latest);
  const reopened = await f.bridge.openRequestedDocument({id:51,kind:'open',intent:'file.openRecent',chooseDestination:false,previousPath:previous});
  assert.deepEqual(await fs.readFile(reopened.path),latest);
  assert.deepEqual(await fs.readFile(previous),ENCODED);
  assert.notEqual(reopened.path,previous);
  assert.equal(reopened.originalName,'selected-name.psd');
  assert.equal(f.permission.persisted,0,'Recent must reactivate existing permission without requesting persistence again');
  assert.equal(f.permission.activated,1);
  await assert.rejects(f.bridge.openRequestedDocument({id:51,kind:'open',intent:'file.openRecent',chooseDestination:true,previousPath:previous}),/missing its original source/);
  const rebuilt = new f.DocumentBridge({filesDir:f.filesDir});
  await rebuilt.restoreBindings();
  assert.deepEqual(await fs.readFile((await rebuilt.reopenSource(previous,52)).path),latest);
});

test('Recent revoked or missing source never falls back to retained cached bytes', async t => {
  const f = await fixture(t);
  const previous = path.join(f.filesDir,'PhotoCraft/Documents/imports/0-1-0/Old.psd');
  await fs.mkdir(path.dirname(previous),{recursive:true});await fs.writeFile(previous,ENCODED);
  await f.bridge.bindSource(previous,URI);
  await assert.rejects(f.bridge.reopenSource(previous,53),/Permission.*has expired/);
  await assert.rejects(f.bridge.reopenSource(previous+'-missing',54),/source record is unavailable/);
  assert.deepEqual(await fs.readFile(previous),ENCODED);
  assert.equal(f.permission.activated,0);
});

test('Recent restore rejects unsafe records and does not follow a linked import ancestor', async t => {
  const f = await fixture(t);
  const documents = path.join(f.filesDir,'PhotoCraft/Documents');
  await fs.mkdir(documents,{recursive:true});
  const record = path.join(documents,'source-uris.json');
  await fs.writeFile(record,JSON.stringify([{path:path.join(f.directory,'outside.psd'),uri:URI}]));
  await assert.rejects(f.bridge.restoreBindings(),/outside/);
  await fs.symlink(f.directory,path.join(documents,'imports'));
  await fs.writeFile(record,JSON.stringify([{path:path.join(documents,'imports/1-2-3/Test.psd'),uri:URI}]));
  await assert.rejects(f.bridge.restoreBindings(),/link/);
  assert.equal(f.bridge.destinationFor(path.join(f.directory,'outside.psd')),undefined);
});

test('CoreFileKit-compatible mkdtemp template creates unique real native directories', () => {
  const probe = spawnSync('python3', ['-c', `
import ctypes, os, tempfile
with tempfile.TemporaryDirectory(prefix='photocraft-template-') as root:
    libc = ctypes.CDLL(None, use_errno=True)
    libc.mkdtemp.argtypes = [ctypes.c_char_p]
    libc.mkdtemp.restype = ctypes.c_char_p
    results = []
    for _ in range(2):
        valid = ctypes.create_string_buffer(os.fsencode(root + '/publication-XXXXXX'))
        assert libc.mkdtemp(valid)
        actual = os.fsdecode(valid.value)
        assert os.path.isdir(actual)
        assert len(actual.rsplit('publication-', 1)[1]) == 6
        results.append(actual)
    assert results[0] != results[1]
`], { encoding: 'utf8' });
  assert.equal(probe.status, 0, probe.stderr);
});

test('production publisher backs up a new empty target and commits real bytes', async t => {
  const f = await fixture(t, '', Buffer.alloc(0));
  assert.equal(await f.bridge.publishDocument(f.stage, URI), 'selected-name.psd');
  assert.deepEqual(await fs.readFile(f.target), ENCODED);
  assert.equal(f.templates.length, 1);
  assert(f.templates[0].endsWith('publication-XXXXXX'));
  assert.deepEqual(await fs.readdir(path.join(f.directory, 'files/PhotoCraft/Documents/PublicationRecovery')), []);
  assert.equal(f.handles.size, 0);
});

test('existing recovery hierarchy permits repeated publication without mkdir EEXIST', async t => {
  const f = await fixture(t);
  await fs.mkdir(path.join(f.directory, 'files/PhotoCraft/Documents/PublicationRecovery'), { recursive: true });
  await f.bridge.publishDocument(f.stage, URI);
  await f.bridge.publishDocument(f.stage, URI);
  assert.deepEqual(await fs.readFile(f.target), ENCODED);
  assert.equal(f.templates.length, 2);
  assert.equal(f.handles.size, 0);
});

for (const kind of ['symlink', 'file']) {
  test(`publication rejects a ${kind} ancestor without touching the target or outside directory`, async t => {
    const f = await fixture(t);
    const outside = path.join(f.directory, 'outside');
    await fs.mkdir(outside);
    const ancestor = path.join(f.directory, 'files/PhotoCraft');
    if (kind === 'symlink') await fs.symlink(outside, ancestor);
    else await fs.writeFile(ancestor, 'ordinary file');
    await assert.rejects(f.bridge.publishDocument(f.stage, URI), /storage directory is invalid/);
    assert.deepEqual(await fs.readFile(f.target), ORIGINAL);
    assert.deepEqual(await fs.readdir(outside), []);
    assert(!f.trace.includes('destination-open-write'));
    assert.equal(f.templates.length, 0);
  });
}

test('destination seek EINVAL is logged by stage/code before mutation', async t => {
  const f = await fixture(t, 'seek-preflight');
  await assert.rejects(f.bridge.publishDocument(f.stage, URI), /original file is unchanged/);
  assert.deepEqual(await fs.readFile(f.target), ORIGINAL);
  assert(!f.trace.includes('destination-copy'));
  assert(f.logs.some(log => log[3] === 'seek-preflight' && log[4] === 13900020));
  for (const log of f.logs) {
    assert(!JSON.stringify(log).includes(URI));
    assert(!JSON.stringify(log).includes(f.directory));
    assert(!JSON.stringify(log).includes('selected-name'));
  }
});

test('journal directory sync EINVAL refuses publication without opening destination for write', async t => {
  const f = await fixture(t, 'directory-fsync');
  await assert.rejects(f.bridge.publishDocument(f.stage, URI), /original file is unchanged/);
  assert.deepEqual(await fs.readFile(f.target), ORIGINAL);
  assert(!f.trace.includes('destination-open-write'));
  assert(f.logs.some(log => log[3] === 'journal-directory-fsync' && log[4] === 13900020));
});

test('SDK adapter partial copy failure restores old bytes but rejects save', async t => {
  const f = await fixture(t, 'destination-copy');
  await assert.rejects(f.bridge.publishDocument(f.stage, URI), /original file was restored/);
  assert.deepEqual(await fs.readFile(f.target), ORIGINAL);
  assert.deepEqual(await fs.readFile(f.stage), ENCODED);
  assert.equal(f.trace.filter(step => step === 'destination-open-write').length, 1);
  assert(f.logs.some(log => log[3] === 'fd-copy' && log[4] === 13900020));
  assert.equal(f.handles.size, 0);
});

test('Revert and linked Smart Object intents refresh authorized external bytes without opening a picker', async t => {
  const f = await fixture(t);
  const previous = path.join(f.filesDir, 'PhotoCraft/Documents/imports/12-source/source.psd');
  await fs.mkdir(path.dirname(previous), {recursive:true}); await fs.writeFile(previous, ENCODED);
  await f.bridge.bindSource(previous, URI); f.permission.granted = true;
  const latest = Buffer.from('new external source bytes'); await fs.writeFile(f.target, latest);
  let id = 800;
  for (const intent of ['file.revert','layer.smartObjects.updateModifiedContent','layer.smartObjects.updateAllModifiedContent',
    'layer.smartObjects.editContents','layer.smartObjects.exportContents','layer.smartObjects.convertToEmbedded']) {
    const imported = await f.bridge.openRequestedDocument({id:id++,kind:'open',intent,chooseDestination:false,previousPath:previous});
    assert.deepEqual(await fs.readFile(imported.path), latest, intent);
    assert.equal(imported.sourceUri, URI); assert.notEqual(imported.path, previous);
  }
  assert.deepEqual(await fs.readFile(previous), ENCODED);
  assert.equal(f.permission.persisted, 0, 'an existing binding does not reacquire a temporary grant');
  assert.equal(f.permission.activated, 6);
});

test('revoked Smart Object authority errors while explicit Replace/Relink still select a new source', async t => {
  const f = await fixture(t);
  const previous = path.join(f.filesDir, 'PhotoCraft/Documents/imports/12-source/source.psd');
  await fs.mkdir(path.dirname(previous), {recursive:true}); await fs.writeFile(previous, ENCODED);
  await f.bridge.bindSource(previous, URI);
  await assert.rejects(f.bridge.openRequestedDocument({id:901,kind:'open',intent:'layer.smartObjects.updateModifiedContent',chooseDestination:false,previousPath:previous}), /Permission.*has expired/);
  assert.deepEqual(await fs.readFile(previous), ENCODED);
  let selected;
  f.bridge.selectDocument = async (id, intent) => { selected = {id,intent}; return undefined; };
  for (const intent of ['layer.smartObjects.replaceContents','layer.smartObjects.relinkToFile']) {
    assert.equal(await f.bridge.openRequestedDocument({id:902,kind:'open',intent,chooseDestination:true,previousPath:previous}), undefined);
    assert.deepEqual(selected, {id:902,intent});
  }
});
