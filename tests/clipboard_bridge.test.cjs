'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {fileURLToPath, pathToFileURL} = require('node:url');
const Module = require('node:module');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH || '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');
const sourcePath = path.resolve(__dirname, '../harmonyos/entry/src/main/ets/platform/ClipboardBridge.ets');
const png = fs.readFileSync(path.join(__dirname, 'fixtures/ohos-smoke.png'));
const mime = {MIMETYPE_TEXT_PLAIN: 'text/plain', MIMETYPE_PIXELMAP: 'pixelmap', MIMETYPE_TEXT_URI: 'text/uri'};
const validSize = {width: png.readUInt32BE(16), height: png.readUInt32BE(20)};

// Run the production bridge; only the device Kit APIs are replaced. Files and PNG bytes are real.
function fixture(t, options = {}) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'photocraft-clipboard-'));
  t.after(() => fs.rmSync(directory, {recursive: true, force: true}));
  const roots = {filesDir: path.join(directory, 'files'), cacheDir: path.join(directory, 'cache')};
  fs.mkdirSync(roots.filesDir); fs.mkdirSync(roots.cacheDir);
  const calls = [], handles = new Set(), records = [];
  const pixelMap = (label = 'record', size = validSize) => ({
    getImageInfo: async () => {calls.push(['pixel-info', label]); return {size};},
    release: async () => {calls.push(['pixel-release', label]);}
  });
  const decoded = pixelMap('decoded', options.sourceSize || validSize);
  const io = {
    OpenMode: {READ_ONLY: fs.constants.O_RDONLY, WRITE_ONLY: fs.constants.O_WRONLY,
      CREATE: fs.constants.O_CREAT, TRUNC: fs.constants.O_TRUNC},
    mkdir: async (value, recursive) => {
      calls.push(['mkdir', value, recursive]);
      if (options.mkdirError) throw options.mkdirError;
      if (options.mkdirRace) fs.mkdirSync(value, {recursive: true});
      // OHOS mkdir(path, true) still rejects an existing final directory.
      // Node's recursive mkdir would silently accept it and conceal this regression.
      try {
        fs.lstatSync(value);
        throw Object.assign(new Error('File exists'), {code: 13900015});
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
      }
      fs.mkdirSync(value, {recursive});
    },
    open: async (value, flags) => {
      calls.push(['open', value, flags]);
      const fd = fs.openSync(value.startsWith('file://') ? fileURLToPath(value) : value, flags);
      handles.add(fd); return {fd};
    },
    close: async file => {calls.push(['close', file.fd]); fs.closeSync(file.fd); handles.delete(file.fd);},
    stat: async fd => {calls.push(['stat', fd]); return fs.fstatSync(fd);},
    lstat: async value => {
      calls.push(['lstat', value]);
      try { return fs.lstatSync(value); }
      catch (error) {
        if (error.code === 'ENOENT') error.code = 13900002;
        throw error;
      }
    },
    fsync: async fd => {calls.push(['fsync', fd]); if (options.syncError) throw options.syncError; fs.fsyncSync(fd);},
    access: async value => fs.existsSync(value),
    unlink: async value => {calls.push(['unlink', value]); fs.unlinkSync(value);}
  };
  const kitImage = {
    createImagePacker: () => ({
      packToFile: async (value, fd, packing) => {
        calls.push(['pack', value, fd, packing]);
        fs.writeSync(fd, options.packError ? png.subarray(0, 16) : png);
        if (options.packError) throw options.packError;
      },
      release: async () => {calls.push(['packer-release']);}
    }),
    createImageSource: fd => {
      calls.push(['source', fd]);
      assert.equal(typeof fd, 'number'); assert.ok(handles.has(fd));
      const bytes = Buffer.alloc(png.length); fs.readSync(fd, bytes, 0, bytes.length, 0);
      assert.deepEqual(bytes, png, 'decoder receives the real opened PNG');
      return {
        getImageInfo: async () => {calls.push(['source-info']); return {size: options.sourceSize || validSize};},
        createPixelMap: async () => {calls.push(['decode']); return decoded;},
        release: async () => {calls.push(['source-release']);}
      };
    }
  };
  const pasteboard = {...mime,
    getSystemPasteboard: () => ({
      getData: async () => ({getRecordCount: () => records.length, getRecord: index => records[index]}),
      setData: async data => {calls.push(['set-data', data]);}
    }),
    createData: (type, value) => ({type, value})
  };
  const loaded = new Module(sourcePath, module); loaded.filename = sourcePath; loaded.paths = module.paths;
  loaded.require = name => {
    if (name === '@kit.BasicServicesKit') return {pasteboard};
    if (name === '@kit.CoreFileKit') return {fileIo: io};
    if (name === '@kit.ImageKit') return {image: kitImage};
    if (name === '@kit.AbilityKit') return {};
    if (name === './ShellLocalization') return require('./load_pure_ets.cjs')(path.resolve(path.dirname(sourcePath), 'ShellLocalization.ets'));
    if (name === './AppStorageRoots') return {AppStorageRoots: class {constructor() {Object.assign(this, roots);}}};
    throw new Error(`Unexpected import ${name}`);
  };
  loaded._compile(ts.transpileModule(fs.readFileSync(sourcePath, 'utf8'), {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
  }).outputText, sourcePath);
  const bridge = new loaded.exports.ClipboardBridge(roots, roots);
  function record(values) {
    records.push({
      getValidTypes: requested => Object.keys(values).filter(value => requested.includes(value)),
      getData: async type => {calls.push(['read', type]); return typeof values[type] === 'function' ? values[type]() : values[type];}
    });
  }
  function uri(fileSize) {
    const input = path.join(directory, 'external-image.png'); fs.writeFileSync(input, png);
    if (fileSize !== undefined) fs.truncateSync(input, fileSize);
    return pathToFileURL(input).href;
  }
  return {bridge, roots, calls, handles, pixelMap, record, uri, io};
}
const count = (f, name) => f.calls.filter(call => call[0] === name).length;

test('delayed PixelMap writes a complete PNG into the native-owned cache/Clipboard directory', async t => {
  const f = fixture(t), map = f.pixelMap();
  const rustDirectory = path.join(f.roots.cacheDir, 'Clipboard');
  fs.mkdirSync(rustDirectory);
  await assert.rejects(f.io.mkdir(rustDirectory, true), error => error.code === 13900015);
  f.record({[mime.MIMETYPE_TEXT_PLAIN]: '说明', [mime.MIMETYPE_PIXELMAP]: () => new Promise(resolve => setImmediate(() => resolve(map)))});
  const content = await f.bridge.read(42, false);
  assert.equal(content.text, '说明');
  assert.equal(path.dirname(content.imagePath), path.join(f.roots.cacheDir, 'Clipboard'));
  assert.match(path.basename(content.imagePath), /^paste-42-\d+-0\.png$/);
  assert.deepEqual(fs.readFileSync(content.imagePath), png);
  assert.deepEqual(f.calls.find(call => call[0] === 'pack').slice(1, 2), [map]);
  assert.deepEqual(f.calls.find(call => call[0] === 'pack')[3], {format: 'image/png', quality: 100});
  assert.equal(count(f, 'fsync'), 1); assert.equal(count(f, 'packer-release'), 1);
  assert.equal(count(f, 'pixel-release'), 1); assert.equal(f.handles.size, 0);
});

test('repeated paste uses unique files and removes only the requested sandbox cache file', async t => {
  const f = fixture(t); f.record({[mime.MIMETYPE_PIXELMAP]: () => f.pixelMap()});
  // Platform::new has already created this directory before the first paste.
  fs.mkdirSync(path.join(f.roots.cacheDir, 'Clipboard'));
  const first = await f.bridge.read(7, false), second = await f.bridge.read(7, false);
  assert.notEqual(first.imagePath, second.imagePath);
  await f.bridge.removeCacheFile(first.imagePath);
  assert.equal(fs.existsSync(first.imagePath), false); assert.deepEqual(fs.readFileSync(second.imagePath), png);
  const foreign = path.join(f.roots.filesDir, 'ordinary.png'); fs.writeFileSync(foreign, png);
  await assert.rejects(f.bridge.removeCacheFile(foreign));
  await assert.rejects(f.bridge.removeCacheFile(`${f.roots.cacheDir}/../files/ordinary.png`));
  assert.deepEqual(fs.readFileSync(foreign), png);
  assert.equal(count(f, 'pixel-release'), 2); assert.equal(count(f, 'packer-release'), 2); assert.equal(f.handles.size, 0);
});

test('mkdir EEXIST race proceeds only after lstat proves the created path is a real directory', async t => {
  const f = fixture(t, {mkdirRace: true}); f.record({[mime.MIMETYPE_PIXELMAP]: f.pixelMap()});
  const content = await f.bridge.read(8, false);
  assert.deepEqual(fs.readFileSync(content.imagePath), png);
  assert.equal(count(f, 'mkdir'), 1); assert.ok(count(f, 'lstat') >= 1);
  assert.equal(count(f, 'pixel-release'), 1); assert.equal(count(f, 'packer-release'), 1); assert.equal(f.handles.size, 0);
});

test('Clipboard path occupied by a file or symlink is rejected without writing PNG bytes', async t => {
  for (const kind of ['file', 'symlink']) {
    const f = fixture(t), directory = path.join(f.roots.cacheDir, 'Clipboard');
    const target = path.join(f.roots.filesDir, 'ordinary'); fs.mkdirSync(target);
    if (kind === 'file') fs.writeFileSync(directory, 'ordinary file');
    else fs.symlinkSync(target, directory, 'dir');
    f.record({[mime.MIMETYPE_PIXELMAP]: f.pixelMap()});
    await assert.rejects(f.bridge.read(9, false));
    assert.equal(count(f, 'pack'), 0); assert.equal(count(f, 'open'), 0);
    assert.equal(count(f, 'pixel-release'), 1); assert.equal(f.handles.size, 0);
    assert.deepEqual(fs.readdirSync(target), []);
    if (kind === 'file') assert.equal(fs.readFileSync(directory, 'utf8'), 'ordinary file');
    else assert.equal(fs.readlinkSync(directory), target);
  }
});

test('non-EEXIST mkdir failure is preserved and cannot create a successful paste result', async t => {
  const failure = Object.assign(new Error('Permission denied'), {code: 13900001});
  const f = fixture(t, {mkdirError: failure}); f.record({[mime.MIMETYPE_PIXELMAP]: f.pixelMap()});
  await assert.rejects(f.bridge.read(10, false), error => error === failure);
  assert.deepEqual(fs.readdirSync(f.roots.cacheDir), []);
  assert.equal(count(f, 'open'), 0); assert.equal(count(f, 'pack'), 0);
  assert.equal(count(f, 'pixel-release'), 1); assert.equal(f.handles.size, 0);
});

test('packer failure removes partial bytes and releases PixelMap, packer and file', async t => {
  const failure = new Error('PNG encoder failed'), f = fixture(t, {packError: failure});
  f.record({[mime.MIMETYPE_PIXELMAP]: f.pixelMap()});
  await assert.rejects(f.bridge.read(1, false), error => error === failure);
  assert.deepEqual(fs.readdirSync(path.join(f.roots.cacheDir, 'Clipboard')), []);
  assert.equal(count(f, 'unlink'), 1); assert.equal(count(f, 'packer-release'), 1);
  assert.equal(count(f, 'pixel-release'), 1); assert.equal(f.handles.size, 0);
});

test('fsync failure cannot return a successful image path and removes the encoded file', async t => {
  const failure = new Error('fsync failed'), f = fixture(t, {syncError: failure});
  f.record({[mime.MIMETYPE_PIXELMAP]: f.pixelMap()});
  await assert.rejects(f.bridge.read(1, false), error => error === failure);
  assert.deepEqual(fs.readdirSync(path.join(f.roots.cacheDir, 'Clipboard')), []);
  assert.equal(count(f, 'packer-release'), 1); assert.equal(count(f, 'pixel-release'), 1); assert.equal(f.handles.size, 0);
});

test('textOnly reads text without requesting delayed PixelMap or URI content', async t => {
  const f = fixture(t);
  f.record({[mime.MIMETYPE_PIXELMAP]: () => {throw new Error('image read forbidden');},
    [mime.MIMETYPE_TEXT_URI]: () => {throw new Error('URI read forbidden');}});
  f.record({[mime.MIMETYPE_TEXT_PLAIN]: '中文😀'});
  assert.deepEqual(await f.bridge.read(1, true), {text: '中文😀', imagePath: ''});
  assert.deepEqual(f.calls.filter(call => call[0] === 'read').map(call => call[1]), [mime.MIMETYPE_TEXT_PLAIN]);
  assert.equal(count(f, 'open'), 0); assert.equal(count(f, 'source'), 0); assert.equal(count(f, 'pack'), 0);
});

test('file URI fallback decodes a real READ_ONLY file descriptor after checking its dimensions', async t => {
  const f = fixture(t), uri = f.uri(); f.record({[mime.MIMETYPE_TEXT_URI]: uri});
  const content = await f.bridge.read(9, false);
  assert.equal(content.text, ''); assert.deepEqual(fs.readFileSync(content.imagePath), png);
  const input = f.calls.find(call => call[0] === 'open');
  assert.ok(input[1] === uri || input[1] === fileURLToPath(uri)); assert.equal(input[2], fs.constants.O_RDONLY);
  assert.ok(f.calls.findIndex(call => call[0] === 'source-info') < f.calls.findIndex(call => call[0] === 'decode'));
  assert.equal(count(f, 'source-release'), 1); assert.equal(count(f, 'pixel-release'), 1); assert.equal(f.handles.size, 0);
});

test('a later PixelMap takes precedence over an earlier image URI', async t => {
  const f = fixture(t), map = f.pixelMap('later');
  f.record({[mime.MIMETYPE_TEXT_URI]: f.uri()}); f.record({[mime.MIMETYPE_PIXELMAP]: map});
  const content = await f.bridge.read(10, false);
  assert.deepEqual(fs.readFileSync(content.imagePath), png); assert.equal(count(f, 'source'), 0);
  assert.equal(count(f, 'open'), 1, 'only the PNG output is opened');
  assert.equal(f.calls.find(call => call[0] === 'pack')[1], map);
});

test('URI dimensions are bounded before decoding and invalid sources release their descriptor', async t => {
  for (const size of [{width: 16385, height: 1}, {width: 8192, height: 8192},
    {width: 0, height: 1}, {width: NaN, height: 1}]) {
    const f = fixture(t, {sourceSize: size}); f.record({[mime.MIMETYPE_TEXT_URI]: f.uri()});
    await assert.rejects(f.bridge.read(11, false));
    assert.equal(count(f, 'decode'), 0); assert.equal(count(f, 'pack'), 0);
    assert.equal(count(f, 'source-release'), 1); assert.equal(f.handles.size, 0);
  }
  for (const bytes of [0, 64 * 1024 * 1024 + 1]) {
    const f = fixture(t); f.record({[mime.MIMETYPE_TEXT_URI]: f.uri(bytes)});
    await assert.rejects(f.bridge.read(11, false), /file|exceeds/);
    assert.equal(count(f, 'source'), 0); assert.equal(count(f, 'decode'), 0); assert.equal(f.handles.size, 0);
  }
});

test('absent content and unsupported URI report failure without producing a PNG', async t => {
  const absent = fixture(t);
  await assert.rejects(absent.bridge.read(1, false), /clipboard|paste|content/);
  const unsupported = fixture(t); unsupported.record({[mime.MIMETYPE_TEXT_URI]: 'https://example.invalid/image.png'});
  await assert.rejects(unsupported.bridge.read(1, false));
  assert.equal(count(unsupported, 'source'), 0); assert.equal(count(unsupported, 'pack'), 0);
  assert.deepEqual(fs.readdirSync(unsupported.roots.cacheDir), []);
});

test('image copy checks the sandbox and releases the real file, decoder and PixelMap', async t => {
  const f = fixture(t), local = path.join(f.roots.filesDir, 'copy.png'); fs.writeFileSync(local, png);
  await assert.rejects(f.bridge.writeImage('/ordinary/foreign.png'));
  assert.equal(count(f, 'open'), 0);
  await f.bridge.writeImage(local);
  assert.equal(f.calls.find(call => call[0] === 'set-data')[1].type, mime.MIMETYPE_PIXELMAP);
  assert.equal(count(f, 'pixel-release'), 1); assert.equal(count(f, 'source-release'), 1); assert.equal(f.handles.size, 0);
});
