const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const crypto = require('node:crypto');
const Module = require('node:module');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH ||
  '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');
const source = path.resolve(__dirname, '../harmonyos/entry/src/ohosTest/ets/support/LayoutRelay.ets');

function fixture(t, responder) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'photocraft-relay-'));
  t.after(() => fs.rmSync(directory, {recursive: true, force: true}));
  const cache = `${directory}/haps/entry/cache/PhotoCraftTestRuns/relay-run/cache`;
  fs.mkdirSync(cache, {recursive: true});
  const printed = [], opened = [], cleaned = [];
  let now = 1000, request;
  const io = {OpenMode: {READ_ONLY: fs.constants.O_RDONLY, NOFOLLOW: fs.constants.O_NOFOLLOW},
    lstat: async value => fs.lstatSync(value), mkdir: async value => fs.mkdirSync(value),
    access: async value => fs.existsSync(value), readText: async (value, options) => fs.readFileSync(value).subarray(0, options.length).toString('utf8'),
    open: async (value, flags) => {opened.push({value, flags}); return {fd: fs.openSync(value, flags)};},
    stat: async fd => fs.fstatSync(fd), read: async (fd, buffer) => fs.readSync(fd, Buffer.from(buffer), 0, buffer.byteLength, null),
    close: async handle => fs.closeSync(handle.fd), unlink: async value => {cleaned.push(value); fs.unlinkSync(value);},
    rmdir: async value => fs.rmdirSync(value)};
  function publish(req, body, modifyReady = x => x) {
    const bytes = Buffer.isBuffer(body) ? body : Buffer.from(JSON.stringify(body));
    fs.writeFileSync(req.replyPath, bytes);
    fs.writeFileSync(req.readyPath, JSON.stringify(modifyReady({runId: req.runId, nonce: req.nonce, ticket: req.ticket,
      path: req.path, payloadBytes: bytes.length, payloadSha256: crypto.createHash('sha256').update(bytes).digest('hex')})));
  }
  const loaded = new Module(source, module); loaded.filename = source;
  loaded.testClock = {now: () => now};
  loaded.testSetTimeout = resolve => {now += 100; setImmediate(resolve);};
  loaded.require = name => {
    if (name === '@kit.TestKit') return {abilityDelegatorRegistry: {getAbilityDelegator: () => ({print: async message => {
      printed.push(message);
      if (message.startsWith('PHOTOCRAFT_LAYOUT_REQUEST ')) {
        request = JSON.parse(message.slice('PHOTOCRAFT_LAYOUT_REQUEST '.length));
        await responder?.(request, publish, io);
      }
    }})}};
    if (name === '@kit.CoreFileKit') return {fileIo: io};
    if (name === '@kit.CryptoArchitectureKit') return {cryptoFramework: {
      createRandom: () => ({generateRandomSync: n => ({data: new Uint8Array(crypto.randomBytes(n))})}),
      createMd: () => {const hash = crypto.createHash('sha256'); return {
        update: async blob => hash.update(blob.data), digest: async () => ({data: new Uint8Array(hash.digest())})};}
    }};
    if (name === '@kit.ArkTS') return {util: {TextEncoder: class {encodeInto(value) {return new TextEncoder().encode(value);}},
      TextDecoder: {create: (...args) => {const decoder = new TextDecoder(...args); return {decodeToString: bytes => decoder.decode(bytes)};}}}};
    throw new Error(`Unexpected import ${name}`);
  };
  loaded._compile('const Date = module.testClock; const setTimeout = module.testSetTimeout;\n' + ts.transpileModule(fs.readFileSync(source, 'utf8'), {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
  }).outputText, source);
  const Relay = loaded.exports.LayoutRelay;
  return {relay: new Relay('relay-run', 'moe.kiwi.photocraft', cache), Relay, cache, printed, opened, cleaned, io,
    read: () => loaded.exports ? undefined : undefined, request: () => request};
}
const query = '/data/local/tmp/PhotoCraftTest-relay-run-123-0-query.json';
const bundle = 'moe.kiwi.photocraft';
const body = req => ({runId: req.runId, nonce: req.nonce, ticket: req.ticket, path: req.path,
  ok: true, layout: {attributes: {text: '中文😀'}, children: []}});

test('two-stage debug relay authenticates raw UTF8 bytes, requires transfer digest and cleans only its own files', async t => {
  const f = fixture(t, (req, publish) => publish(req, body(req)));
  assert.deepEqual(JSON.parse(await f.relay.read(query, bundle)), body(f.request()).layout);
  assert.match(f.request().nonce, /^[0-9a-f]{64}$/); assert.equal(f.request().ticket, 'layout-1');
  assert.match(f.request().replyPath, /\/LayoutRelay-1000-1\/reply\.part$/);
  assert.equal(f.request().readyPath, f.request().replyPath.replace('reply.part', 'ready.json'));
  assert.equal(f.opened.length, 1); assert.ok(f.opened[0].flags & fs.constants.O_NOFOLLOW);
  assert.deepEqual(fs.readdirSync(f.cache), []);
  assert.deepEqual(f.cleaned.sort(), [f.request().replyPath, f.request().readyPath].sort());
});

test('wrong ready identity, digest, bounds, extra fields and reply identity never pass', async t => {
  for (const modify of [ready => ({...ready, nonce: '0'.repeat(64)}), ready => ({...ready, payloadSha256: '0'.repeat(64)}),
    ready => ({...ready, payloadBytes: 5 * 1024 * 1024}), ready => ({...ready, extra: true})]) {
    const f = fixture(t, (req, publish) => publish(req, body(req), modify));
    await assert.rejects(f.relay.read(query, bundle), /identity|digest|schema/);
    assert.deepEqual(fs.readdirSync(f.cache), []);
  }
  const f = fixture(t, (req, publish) => publish(req, {...body(req), ticket: 'layout-999'}));
  await assert.rejects(f.relay.read(query, bundle), /identity/);
});

test('host error and invalid UTF8 preserve honest failures and still clean owned directory', async t => {
  const errorFixture = fixture(t, (req, publish) => publish(req, {...body(req), ok: false, layout: undefined, error: 'HDC recv denied'}));
  await assert.rejects(errorFixture.relay.read(query, bundle), /HDC recv denied/);
  const utf8Fixture = fixture(t, (req, publish) => publish(req, Buffer.from([123, 34, 120, 34, 58, 34, 255, 34, 125])));
  await assert.rejects(utf8Fixture.relay.read(query, bundle));
  assert.deepEqual(fs.readdirSync(utf8Fixture.cache), []);
});

test('partial ready is polled to completed JSON and no ready times out instead of passing', async t => {
  const partial = fixture(t, (req, publish) => {
    fs.writeFileSync(req.readyPath, '{');
    setImmediate(() => publish(req, body(req)));
  });
  assert.ok(await partial.relay.read(query, bundle));
  const absent = fixture(t);
  await assert.rejects(absent.relay.read(query, bundle), /within 10 seconds/);
  assert.deepEqual(fs.readdirSync(absent.cache), []);
});

test('foreign bundle/query/ordinary root and concurrent reads fail before arbitrary IO; close cancels pending request', async t => {
  const f = fixture(t);
  assert.throws(() => new f.Relay('relay-run', bundle, '/ordinary/cache'), /validated/);
  await assert.rejects(f.relay.read('/data/local/tmp/foreign.json', bundle), /owned/);
  await assert.rejects(f.relay.read(query, 'foreign.app'), /outside/);
  assert.equal(f.printed.length, 0);
  const pending = f.relay.read(query, bundle);
  await assert.rejects(f.relay.read(query, bundle), /serial/);
  const rejection = assert.rejects(pending, /closed/);
  await f.relay.close(); await rejection;
  assert.deepEqual(fs.readdirSync(f.cache), []);
  await assert.rejects(f.relay.read(query, bundle), /open/);
});
