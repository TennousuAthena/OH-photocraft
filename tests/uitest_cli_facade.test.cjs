'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Module = require('node:module');
const {execFileSync} = require('node:child_process');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH ||
  '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');
const source = path.resolve(__dirname, '../harmonyos/entry/src/ohosTest/ets/support/DeviceUiDriver.ets');
// Public component attributes and actual window bounds from an API 26 failure
// capture. Document contents and unrelated desktop windows are omitted.
const captured = JSON.parse(fs.readFileSync(path.join(__dirname, 'fixtures/uitest-photocraft.layout.json'), 'utf8'));
const BUNDLE = 'moe.kiwi.photocraft';
const RUN = 'cli-contract-run';
const CACHE = `/data/storage/el2/base/haps/entry/cache/PhotoCraftTestRuns/${RUN}/cache`;

function fixture() {
  const commands = [], printed = [], reads = [], fileOperations = [], relayCalls = [];
  const state = {tree: structuredClone(captured), reply: undefined, readReply: undefined,
    relayError: undefined, relayReply: undefined, relayClosed: false};
  const fileIo = {
    async readText(target, options) {
      reads.push({target, options});
      assert.equal(options.length, 4 * 1024 * 1024);
      if (state.readReply) {
        const value = await state.readReply(target);
        if (value !== undefined) return value;
      }
      return Buffer.from(JSON.stringify(state.tree), 'utf8').toString('utf8');
    },
    async lstat(target) {fileOperations.push(['lstat', target]); return {isDirectory: () => true, isSymbolicLink: () => false};},
    async mkdir(target) {fileOperations.push(['mkdir', target]);},
    async access(target) {fileOperations.push(['access', target]); return true;},
    async unlink(target) {fileOperations.push(['unlink', target]);},
    async rmdir(target) {fileOperations.push(['rmdir', target]);}
  };
  const delegator = {
    async executeShellCommand(command) {
      commands.push(command);
      if (state.reply) {
        const response = await state.reply(command);
        if (response !== undefined) return response;
      }
      const dump = command.startsWith('uitest dumpLayout ') ? command.match(/ -p (\S+)$/) : undefined;
      return {exitCode: 0, stdResult: dump ? `DumpLayout saved to:${dump[1]}\n` : ''};
    },
    async print(message) {printed.push(message);}
  };
  const loaded = new Module(source, module); loaded.filename = source;
  loaded.require = name => {
    if (name === './LayoutRelay') return {LayoutRelay: class {
      constructor(runId, bundle, cache) {assert.equal(runId, RUN); assert.equal(bundle, BUNDLE); assert.equal(cache, CACHE);}
      async read(target, bundle) {
        relayCalls.push({target, bundle});
        if (state.relayError) throw state.relayError;
        return state.relayReply === undefined ? JSON.stringify(state.tree) : state.relayReply;
      }
      async close() {state.relayClosed = true;}
    }};
    if (name === '@kit.CoreFileKit') return {fileIo};
    assert.equal(name, '@kit.TestKit');
    return {abilityDelegatorRegistry: {getAbilityDelegator: () => delegator}};
  };
  loaded._compile(ts.transpileModule(fs.readFileSync(source, 'utf8'), {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}
  }).outputText, source);
  const {DeviceUiDriver, DeviceOn} = loaded.exports;
  return {driver: new DeviceUiDriver(BUNDLE, RUN, null, CACHE), on: DeviceOn, DeviceUiDriver, commands, printed,
    reads, fileOperations, relayCalls, state,
    nodes: state.tree.children[0].children,
    inputs: () => commands.filter(command => command.startsWith('uitest uiInput '))};
}

function addInput(f) {
  f.nodes.push({attributes: {id: 'filename', type: 'TextInput', text: 'previous', focused: 'true',
    bounds: '[1440,1220][1700,1280]', enabled: 'true', visible: 'true'}, children: []});
}

test('null SDK selects CLI and exact inherited app id clicks actual captured bounds', async () => {
  const f = fixture();
  assert.equal(f.driver.backend, 'uitest-cli');
  const component = await f.driver.findComponent(f.on.id('photocraft-paste-authorize'));
  await component.click();
  assert.deepEqual(f.inputs(), ['uitest uiInput click 1480 1089']);
  assert.ok(f.commands.filter(command => command.startsWith('uitest dumpLayout '))
    .every(command => command.includes(`-b ${BUNDLE} `)));
});

test('text, type, focus and window selectors match exact public attributes', async () => {
  const f = fixture();
  assert.ok(await f.driver.findComponent(f.on.text('取消').type('Button').focused(false).inWindow(BUNDLE)));
  for (const selector of [f.on.text('取'), f.on.text('取消').type('Text'), f.on.text('取消').focused(true)]) {
    assert.equal(await f.driver.findComponent(selector), undefined);
  }
});

test('foreign bundle queries and window actions are rejected before any OS command', async () => {
  const f = fixture();
  await assert.rejects(f.driver.findComponent(f.on.id('x').inWindow('foreign.app')), /outside/);
  await assert.rejects(f.driver.findWindow({bundleName: 'com.huawei.hmos.filemanager'}), /restricted/);
  assert.deepEqual(f.commands, []);
  f.state.tree.children[0].attributes.bundleName = 'com.huawei.hmos.filemanager';
  assert.ok(await f.driver.findComponent(f.on.text('取消').inWindow('com.huawei.hmos.filemanager')));
});

test('foreign spoofed nodes are ignored and duplicate app matches cannot be acted on', async () => {
  const f = fixture();
  const node = structuredClone(f.nodes.find(n => n.attributes.id === 'photocraft-paste-authorize'));
  f.state.tree.children.push({attributes: {bundleName: 'foreign.app', type: 'root', bounds: '[0,0][2776,1775]'}, children: [node]});
  assert.ok(await f.driver.findComponent(f.on.id('photocraft-paste-authorize')));
  f.nodes.push(structuredClone(node));
  await assert.rejects(f.driver.findComponent(f.on.id('photocraft-paste-authorize')), /unique/);
  await assert.rejects(f.driver.componentAction(f.on.id('photocraft-paste-authorize'), 'click'), /one currently/);
  assert.deepEqual(f.inputs(), []);
});

test('a previously found component is re-resolved and stale actions fail', async () => {
  const f = fixture();
  const component = await f.driver.findComponent(f.on.id('photocraft-paste-authorize'));
  f.state.tree.children[0].children = f.nodes.filter(n => n.attributes.id !== 'photocraft-paste-authorize');
  await assert.rejects(component.click(), /one currently/);
  assert.deepEqual(f.inputs(), []);
});

test('unsafe or out-of-window bounds never generate coordinate input', async () => {
  for (const bounds of ['', '[1,1][1,2]', '[-1,1][3,3]', '[1.5,1][3,3]', '[1,1][9999999999999999999999999,3]',
                        '[1,1][32769,3]', '[0,0][3,3]', '[2700,1700][3000,1800]']) {
    const f = fixture();
    f.nodes.find(n => n.attributes.id === 'photocraft-paste-authorize').attributes.bounds = bounds;
    await assert.rejects(f.driver.componentAction(f.on.id('photocraft-paste-authorize'), 'click'), /bounds/);
    assert.deepEqual(f.inputs(), []);
  }
});

test('hidden subtrees are skipped and disabled visible buttons cannot be clicked', async () => {
  const f = fixture();
  f.state.tree.children[0].attributes.visible = 'false';
  assert.equal(await f.driver.findComponent(f.on.text('取消')), undefined);
  f.state.tree.children[0].attributes.visible = 'true';
  f.nodes.find(n => n.attributes.text === '取消').attributes.enabled = 'false';
  await assert.rejects(f.driver.componentAction(f.on.text('取消'), 'click'), /disabled/);
  assert.deepEqual(f.inputs(), []);
});

test('inputText requires an actual TextInput and uses click, real Ctrl+A and text in order', async () => {
  const f = fixture();
  await assert.rejects(f.driver.componentInput(f.on.id('photocraft-paste-authorize'), 'file'), /actual TextInput/);
  assert.deepEqual(f.inputs(), []);
  addInput(f);
  const editor = await f.driver.findComponent(f.on.id('filename').type('TextInput').focused(true));
  await editor.inputText('测试😀.pcraft');
  assert.deepEqual(f.inputs(), ['uitest uiInput click 1570 1250', 'uitest uiInput keyEvent 2072 2017',
                              "uitest uiInput text '测试😀.pcraft'"]);
});

test('input text shell quoting preserves apostrophes, shell metacharacters and Unicode literally', async () => {
  const f = fixture(); addInput(f);
  const text = "测试' ; $(printf expanded) `printf tick` 😀";
  await f.driver.componentInput(f.on.id('filename'), text);
  const quoted = f.inputs().at(-1).slice('uitest uiInput text '.length);
  const decoded = execFileSync('/bin/sh', ['-c', `printf '%s' ${quoted}`], {encoding: 'utf8'});
  assert.equal(decoded, text);
});

test('invalid control characters and text lengths fail before layout or input commands', async () => {
  for (const text of ['', 'x'.repeat(129), 'bad\0text', 'bad\ntext', 'bad\rtext']) {
    const f = fixture(); addInput(f);
    await assert.rejects(f.driver.componentInput(f.on.id('filename'), text), /invalid control character|input limit/);
    assert.deepEqual(f.commands, []);
  }
});

test('actual key and Back commands are serial and invalid codes never run', async () => {
  const f = fixture();
  for (const code of [NaN, -1, 10001, 1.5]) await assert.rejects(f.driver.triggerKey(code), /Invalid/);
  assert.deepEqual(f.commands, []);
  await f.driver.triggerKey(2017); await f.driver.pressBack();
  assert.deepEqual(f.inputs(), ['uitest uiInput keyEvent 2017', 'uitest uiInput keyEvent Back']);
});

test('window close uses the exact real EnhanceCloseBtn with Button type inside its app window', async () => {
  const f = fixture();
  const window = await f.driver.findWindow({bundleName: BUNDLE});
  await window.close();
  assert.deepEqual(f.inputs(), ['uitest uiInput click 2711 290']);
  f.nodes.find(n => n.attributes.id === 'EnhanceCloseBtn').attributes.type = 'Text';
  f.nodes.push({attributes: {id: 'fake-close', text: '关闭', type: 'Button', bounds: '[400,400][500,500]'}});
  f.driver.waitForComponent = async selector => {
    assert.equal(selector.idValue, 'EnhanceCloseBtn'); assert.equal(selector.typeValue, 'Button');
    const component = await f.driver.findComponent(selector);
    assert.ok(component, 'no matching actual close Button');
    return component;
  };
  await assert.rejects(window.close(), /no matching actual close Button/);
  assert.equal(f.inputs().length, 1);
});

test('concurrent layout queries reject the second operation and recover after the first completes', async () => {
  const f = fixture();
  let release, entered;
  const active = new Promise(resolve => {entered = resolve;});
  const gate = new Promise(resolve => {release = resolve;});
  f.state.reply = async command => {
    if (command.startsWith('uitest dumpLayout ')) {entered(); await gate;}
  };
  const first = f.driver.findComponent(f.on.text('取消'));
  await active;
  await assert.rejects(f.driver.findComponent(f.on.text('取消')), /serial/);
  release(); assert.ok(await first);
  f.state.reply = undefined;
  assert.ok(await f.driver.findComponent(f.on.text('取消')));
});

test('relay failure retains the actual host error, resets busy state and avoids app-UID daemon removal', async () => {
  const f = fixture();
  f.state.readReply = async () => {throw Object.assign(new Error('read fixture denied'), {code: 13900001});};
  f.state.relayError = new Error('Host layout relay failed: owned source missing');
  await assert.rejects(f.driver.findComponent(f.on.text('取消')), /Host layout relay failed: owned source missing/);
  assert.ok(f.printed.some(line => line.startsWith('PHOTOCRAFT_QUERY_FILE ')));
  const stages = f.printed.filter(line => line.startsWith('PHOTOCRAFT_UI_LAYOUT_READ '))
    .map(line => JSON.parse(line.slice('PHOTOCRAFT_UI_LAYOUT_READ '.length)).stage);
  assert.deepEqual(stages, ['direct-read']);
  assert.ok(f.commands.every(command => !command.startsWith('rm ') && !command.startsWith('cat ')));
  f.state.readReply = undefined; f.state.relayError = undefined;
  assert.ok(await f.driver.findComponent(f.on.text('取消')));
  f.state.readReply = async () => 'not JSON';
  await assert.rejects(f.driver.findComponent(f.on.text('取消')), SyntaxError);
});

test('daemon-owned layouts are read successfully and publish unique exact files for host cleanup', async () => {
  const f = fixture();
  f.state.reply = async command => command.startsWith('rm ') ? {exitCode: 1, stdResult: 'Permission denied (app UID)'} : undefined;
  await f.driver.findComponent(f.on.text('取消'));
  await f.driver.findComponent(f.on.id('photocraft-surface'));
  const records = f.printed.filter(line => line.startsWith('PHOTOCRAFT_QUERY_FILE '))
    .map(line => JSON.parse(line.slice('PHOTOCRAFT_QUERY_FILE '.length)));
  assert.equal(records.length, 2);
  assert.equal(new Set(records.map(record => record.path)).size, 2);
  records.forEach((record, sequence) => {
    assert.equal(record.runId, RUN); assert.equal(record.bundleName, BUNDLE);
    assert.match(record.path, new RegExp(`^/data/local/tmp/PhotoCraftTest-${RUN}-[0-9]+-${sequence}-query\\.json$`));
  });
  assert.equal(f.reads.length, 2);
  assert.ok(f.reads.every(({target}) => records.some(record => record.path === target)));
  assert.ok(f.commands.every(command => !command.startsWith('rm ') && !command.startsWith('cat ')));
});

test('direct read denial uses the exact authenticated host relay without app shell file commands', async () => {
  const f = fixture();
  f.state.readReply = async () => {throw Object.assign(new Error('direct read blocked'), {code: 13900001});};
  assert.ok(await f.driver.findComponent(f.on.text('取消')));
  assert.equal(f.fileOperations.length, 0);
  assert.equal(f.relayCalls.length, 1);
  assert.match(f.relayCalls[0].target, /^\/data\/local\/tmp\/PhotoCraftTest-cli-contract-run-[0-9]+-0-query\.json$/);
  assert.equal(f.relayCalls[0].bundle, BUNDLE);
  assert.ok(f.commands.every(command => command.startsWith('uitest dumpLayout ')));
  assert.equal(f.printed.filter(line => line.startsWith('PHOTOCRAFT_UI_LAYOUT_READ ')).length, 1);
});

test('repeated denied reads use one validated relay and driver close ends its owned relay', async () => {
  const f = fixture();
  f.state.readReply = async () => {throw new Error('direct denied');};
  assert.ok(await f.driver.findComponent(f.on.text('取消')));
  assert.ok(await f.driver.findComponent(f.on.id('photocraft-surface')));
  assert.equal(f.relayCalls.length, 2);
  await f.driver.close();
  assert.equal(f.state.relayClosed, true);
  assert.equal(f.fileOperations.length, 0);
});

test('UiTest exception text with exit code zero cannot be reported as successful system input', async () => {
  const f = fixture();
  f.state.reply = async command => command.startsWith('uitest uiInput ') ?
    {exitCode: 0, stdResult: 'exceptionMessage: Input dispatch failed'} : undefined;
  await assert.rejects(f.driver.triggerKey(2017), /Input dispatch failed/);
  assert.ok(f.printed.some(line => line.startsWith('PHOTOCRAFT_UI_COMMAND_ERROR ')));
});

test('nonempty dump output must confirm the exact owned file; ownership marker precedes guard failure', async () => {
  for (const output of ['exceptionMessage: layout failed', 'DumpLayout saved to:/data/local/tmp/other.json']) {
    const f = fixture();
    f.state.reply = async command => command.startsWith('uitest dumpLayout ') ? {exitCode: 0, stdResult: output} : undefined;
    await assert.rejects(f.driver.findComponent(f.on.text('取消')), /unexpected layout output/i);
    assert.deepEqual(f.reads, []);
    assert.equal(f.printed.filter(line => line.startsWith('PHOTOCRAFT_QUERY_FILE ')).length, 1);
    assert.deepEqual(f.inputs(), []);
  }
});

test('empty dump stdout succeeds only after the exact owned file is read as complete actual layout JSON', async () => {
  const f = fixture();
  f.state.reply = async command => command.startsWith('uitest dumpLayout ') ? {exitCode: 0, stdResult: ''} : undefined;
  assert.ok(await f.driver.findComponent(f.on.text('取消')));
  assert.equal(f.reads.length, 1);
  const marker = JSON.parse(f.printed.find(line => line.startsWith('PHOTOCRAFT_QUERY_FILE '))
    .slice('PHOTOCRAFT_QUERY_FILE '.length));
  assert.equal(f.reads[0].target, marker.path);
  assert.ok(f.printed.some(line => line.startsWith('PHOTOCRAFT_UI_LAYOUT_DUMP ') &&
    JSON.parse(line.slice('PHOTOCRAFT_UI_LAYOUT_DUMP '.length)).outputKind === 'empty'));
});

test('empty dump stdout cannot hide an absent owned file, invalid JSON or incomplete tree', async () => {
  for (const contents of [undefined, '', 'not JSON', '{}']) {
    const f = fixture();
    f.state.reply = async command => command.startsWith('uitest dumpLayout ') ? {exitCode: 0, stdResult: ''} : undefined;
    f.state.readReply = async () => {
      if (contents === undefined) throw Object.assign(new Error('owned layout not found'), {code: 13900002});
      return contents;
    };
    f.state.relayError = new Error('owned layout not found');
    await assert.rejects(f.driver.findComponent(f.on.text('取消')), /not found|JSON|layout|tree/i);
    assert.deepEqual(f.inputs(), []);
    assert.equal(f.printed.filter(line => line.startsWith('PHOTOCRAFT_QUERY_FILE ')).length, 1);
  }
});

test('invalid bundle and run namespace cannot reach the shell', () => {
  const f = fixture();
  assert.throws(() => new f.DeviceUiDriver('app;bad', RUN), /bundle/);
  assert.throws(() => new f.DeviceUiDriver(BUNDLE, '../bad'), /namespace/);
  for (const cache of ['/data/storage/el2/base/cache', CACHE.replace(RUN, 'other-run'), `${CACHE}/../cache`, `${CACHE}/`]) {
    assert.throws(() => new f.DeviceUiDriver(BUNDLE, RUN, null, cache), /validated entry-module test cache/);
  }
  assert.deepEqual(f.commands, []);
});
