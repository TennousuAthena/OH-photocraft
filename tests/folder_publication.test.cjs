'use strict';

// Exercise the production ArkTS helper with real sandbox trees and a documented
// SDK-contract shim. The provider is a host fixture, not a HarmonyOS device test.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const syncFs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const Module = require('node:module');
const ts = require(process.env.ARKTS_TYPESCRIPT_PATH ||
  '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');
const platformRoot = process.env.FOLDER_PUBLICATION_HELPER_ROOT ||
  path.resolve(__dirname, '../harmonyos/entry/src/main/ets/platform');
const URI = 'content://fixture/opaque-authorized-folder';
const TARGET = 'opaque-original-output-job-generation';

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

function defer() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return {promise, resolve};
}

async function fixture(t, options = {}) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'photocraft-folder-publication-'));
  const filesDir = path.join(directory, 'files');
  const destination = path.join(directory, 'provider-folder');
  await fs.mkdir(filesDir);
  await fs.mkdir(destination);
  await fs.writeFile(path.join(destination, 'image.png'), 'unrelated original root image');
  const handles = new Map();
  const logs = [];
  const trace = [];
  const faults = new Set();
  const uuids = [];
  const copyCalls = [];
  const state = {selected:[URI], permission:true, directory:true, gate:undefined,
    entered:defer(), cancelCount:0, copyMode:'success', progress:undefined, uuidFailure:false};
  const error = () => Object.assign(new Error('Injected private provider path must not appear in UI'), {code:13900020});
  const hit = name => {trace.push(name);if(faults.delete(name)) throw error();};
  const modes = {READ_ONLY:0, WRITE_ONLY:1, CREATE:64, TRUNC:512, NOFOLLOW:0x100000};
  const fileIo = {
    OpenMode:modes,
    TaskSignal:class {cancel() {state.cancelCount++;trace.push('cancel-copy');}},
    async mkdir(location) {hit('mkdir');await fs.mkdir(location);},
    async mkdtemp(template) {
      assert(template.endsWith('-XXXXXX'));
      trace.push('mkdtemp');return fs.mkdtemp(template.slice(0,-6));
    },
    async access(location) {try {await fs.access(location);return true;} catch {return false;}},
    async lstat(location) {
      trace.push('lstat:'+location);
      try {return await fs.lstat(location);} catch(error) {
        if(error.code==='ENOENT') error.code=13900002;
        throw error;
      }
    },
    async stat(uri) {assert.equal(uri,URI);hit('destination-stat');return {isDirectory:()=>state.directory};},
    async listFile(location) {return fs.readdir(location);},
    async readText(location) {return fs.readFile(location,'utf8');},
    async open(location, mode) {
      let flags = mode & modes.WRITE_ONLY ? syncFs.constants.O_WRONLY : syncFs.constants.O_RDONLY;
      if(mode & modes.CREATE) flags |= syncFs.constants.O_CREAT;
      if(mode & modes.TRUNC) flags |= syncFs.constants.O_TRUNC;
      if(mode & modes.NOFOLLOW) flags |= syncFs.constants.O_NOFOLLOW;
      const handle = await fs.open(location,flags);
      handles.set(handle.fd,{handle,location,phase:''});
      return {fd:handle.fd};
    },
    async write(fd, value) {
      const record = handles.get(fd);
      const bytes = Buffer.from(value);
      if(record.location.endsWith('/publication.json.next')) record.phase=JSON.parse(bytes.toString()).phase;
      return (await record.handle.write(bytes)).bytesWritten;
    },
    async fsync(fd) {
      const record = handles.get(fd);
      hit(record.phase ? 'journal-sync:'+record.phase : 'sandbox-fsync');
      await record.handle.sync();
    },
    async close(file) {const fd=typeof file==='number'?file:file.fd;await handles.get(fd).handle.close();handles.delete(fd);},
    async rename(source,dest) {
      hit(source.endsWith('.next')?'metadata-rename':'stage-rename');
      await fs.rename(source,dest);
    },
    async copy(sourceUri,destUri,options) {
      assert.equal(destUri,URI);assert(sourceUri.startsWith('sandbox-file:'));
      const source=sourceUri.slice('sandbox-file:'.length);
      const published=path.join(destination,path.basename(source));
      copyCalls.push({sourceUri,destUri,source,published});
      hit('provider-copy');
      assert.match(path.basename(source),/^PhotoCraft-Export-[0-9a-f-]+$/);
      assert.equal((await fs.readFile(path.join(path.dirname(source),'publication.json'),'utf8')).includes('"phase":"copying"'),true);
      state.entered.resolve();
      if(state.gate) await state.gate.promise;
      if(state.progress) for(const progress of state.progress) options.progressListener(progress);
      if(state.copyMode==='partial-error') {
        await fs.mkdir(published);
        await fs.writeFile(path.join(published,'partial.png'),'partial');
        throw error();
      }
      if(state.copyMode==='error') throw error();
      // Models the exact pinned CopyDirFunc basename layout, not SDK provider internals.
      await fs.cp(source,published,{recursive:true,force:true});
    }
  };
  class Encoder {encodeInto(text) {return new TextEncoder().encode(text);}}
  const kits = {
    '@kit.AbilityKit':{}, '@kit.BasicServicesKit':{},
    '@kit.ArkTS':{util:{TextEncoder:Encoder,generateRandomUUID(entropyCache) {
      assert.equal(entropyCache,true);hit('random-uuid');
      if(state.uuidFailure) throw error();
      return uuids.length ? uuids.shift() : crypto.randomUUID();
    }}},
    '@kit.CoreFileKit':{fileIo,fileUri:{getUriFromPath:location=>'sandbox-file:'+location},fileShare:{
      OperationMode:{READ_MODE:1,WRITE_MODE:2},
      async persistPermission(policies) {assert.deepEqual(policies,[{uri:URI,operationMode:3}]);hit('persist');},
      async checkPersistentPermission(policies) {assert.equal(policies[0].uri,URI);hit('permission-check');return [state.permission];},
      async activatePermission(policies) {assert.equal(policies[0].uri,URI);hit('activate');}
    },picker:{DocumentSelectMode:{FOLDER:2},DocumentSelectOptions:class {},DocumentViewPicker:class {
      async select(options) {assert.equal(options.selectMode,2);assert.equal(options.maxSelectNumber,1);hit('picker');return state.selected;}
    }}},
    '@kit.PerformanceAnalysisKit':{hilog:{error:(...args)=>logs.push(args)}}
  };
  const capability = global.canIUse;
  global.canIUse = ()=>true;
  const production = loadProduction('FolderPublicationBridge',kits);
  const destinationHandle = 'c05287c3-8b6a-4c7e-a09b-5c5ae9682df1';
  const resolver = options.reuseDestination ? async (handle, target) => {
    assert.equal(handle,destinationHandle);assert.equal(target,TARGET);
    hit('resolve-destination');return URI;
  } : undefined;
  const bridge = new production.FolderPublicationBridge({filesDir},resolver);
  t.after(async()=>{
    global.canIUse=capability;
    await Promise.allSettled([...handles.values()].map(record=>record.handle.close()));
    await fs.rm(directory,{recursive:true,force:true});
  });
  async function create() {
    const stage=await bridge.createStage(100,TARGET);
    await fs.mkdir(path.join(stage.stageRoot,'空目录'));
    await fs.mkdir(path.join(stage.stageRoot,'嵌套'));
    await fs.writeFile(path.join(stage.stageRoot,'image.png'),'fresh output root image');
    await fs.writeFile(path.join(stage.stageRoot,'嵌套','中文.psd'),'8BPS data');
    trace.length=0;
    return stage;
  }
  function request(stage,id=101,target=TARGET) {return {kind:'folderPublish',id,target,ownerRoot:stage.ownerRoot,
    stageRoot:stage.stageRoot,limits:{maxFiles:500,maxBytes:1024*1024},policy:'freshSubdirectory',
    ...(options.reuseDestination ? {destinationHandle} : {})};}
  async function publish(stage,id=101,callback=()=>{}) {return bridge.publishFolder(request(stage,id),callback);}
  return {directory,filesDir,destination,bridge,kits,production,trace,faults,uuids,logs,state,copyCalls,create,request,publish,handles};
}

test('production adapter publishes a named complete tree after durable journal; siblings and hierarchy are preserved',async t=>{
  const f=await fixture(t);const stage=await f.create();const progress=[];
  const done=await f.publish(stage,101,p=>progress.push(p));
  assert.equal(done.result,'success');assert.equal(done.publishedCount,2);assert.equal(done.partialPossible,false);
  assert.equal(done.retainStage,true);assert.equal(done.ownerRoot,stage.ownerRoot);
  assert.match(done.publicationName,/^PhotoCraft-Export-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  const child=path.join(f.destination,done.publicationName);
  assert.equal(await fs.readFile(path.join(child,'image.png'),'utf8'),'fresh output root image');
  assert.equal(await fs.readFile(path.join(child,'嵌套','中文.psd'),'utf8'),'8BPS data');
  assert.deepEqual(await fs.readdir(path.join(child,'空目录')),[]);
  for(const name of ['contents','owner.json','manifest.json','publication.json']) await assert.rejects(fs.access(path.join(child,name)));
  assert.equal(await fs.readFile(path.join(f.destination,'image.png'),'utf8'),'unrelated original root image');
  assert(f.trace.indexOf('journal-sync:copying')<f.trace.indexOf('provider-copy'));
  assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'complete');
  assert.equal(progress.at(-1).filesDone,2);assert.equal(f.handles.size,0);
});

test('a form-persisted destination publishes after its picker temporary grant has expired',async t=>{
  const f=await fixture(t,{reuseDestination:true});const stage=await f.create();
  f.faults.add('persist'); // A second persistence attempt is rejected by this provider.
  const done=await f.publish(stage);
  assert.equal(done.result,'success');assert.equal(done.publishedCount,2);
  assert(f.faults.has('persist'));assert(!f.trace.includes('persist'));
  assert(f.trace.indexOf('resolve-destination')<f.trace.indexOf('permission-check'));
  assert(f.trace.indexOf('permission-check')<f.trace.indexOf('activate'));
  assert(f.trace.indexOf('activate')<f.trace.indexOf('provider-copy'));
  assert.equal(await fs.readFile(path.join(f.destination,done.publicationName,'嵌套','中文.psd'),'utf8'),'8BPS data');
  assert(!f.trace.includes('picker'));
});

test('a revoked form-persisted grant fails before provider writes and retains the encoded tree',async t=>{
  const f=await fixture(t,{reuseDestination:true});const stage=await f.create();
  f.state.permission=false;f.faults.add('persist');
  const done=await f.publish(stage);
  assert.equal(done.result,'error');assert.equal(done.partialPossible,false);assert.equal(f.copyCalls.length,0);
  assert(f.faults.has('persist'));assert(!f.trace.includes('activate'));
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
  assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'failed');
  assert.deepEqual(await fs.readdir(f.destination),['image.png']);
});

for(const boundary of ['persist','permission-check','activate','destination-stat']) {
  test(`${boundary} failure retains full tree and journal before any provider mutation`,async t=>{
    const f=await fixture(t);const stage=await f.create();f.faults.add(boundary);
    const done=await f.publish(stage);
    assert.equal(done.result,'error');assert.equal(done.partialPossible,false);assert.equal(f.copyCalls.length,0);
    assert.equal(await fs.readFile(path.join(done.stageRoot,'嵌套','中文.psd'),'utf8'),'8BPS data');
    assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'failed');
    assert.deepEqual(await fs.readdir(f.destination),['image.png']);
    assert(!JSON.stringify(done).includes(URI));assert(!done.error.includes('private provider'));
  });
}

test('picker cancel retains the renamed full output and cancellation journal without external writes',async t=>{
  const f=await fixture(t);const stage=await f.create();f.state.selected=[];
  const done=await f.publish(stage);
  assert.equal(done.result,'cancel');assert.equal(done.partialPossible,false);assert.equal(f.copyCalls.length,0);
  assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'cancelled');
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
});

test('provider rejection after partial writes preserves local tree and partial child; retry uses a fresh name',async t=>{
  const f=await fixture(t);const stage=await f.create();f.state.copyMode='partial-error';
  const first=await f.publish(stage);
  assert.equal(first.result,'error');assert.equal(first.partialPossible,true);assert.equal(first.publishedCount,0);
  assert.equal(await fs.readFile(path.join(first.stageRoot,'image.png'),'utf8'),'fresh output root image');
  assert.equal(await fs.readFile(path.join(f.destination,first.publicationName,'partial.png'),'utf8'),'partial');
  f.state.copyMode='success';
  const second=await f.publish({...stage,stageRoot:first.stageRoot},102);
  assert.equal(second.result,'success');assert.notEqual(second.publicationName,first.publicationName);
  assert.equal(await fs.readFile(path.join(f.destination,first.publicationName,'partial.png'),'utf8'),'partial');
  assert.equal(await fs.readFile(path.join(f.destination,second.publicationName,'嵌套','中文.psd'),'utf8'),'8BPS data');
});

test('cancel matches both id and target and waits for delayed provider settlement without deleting any stage',async t=>{
  const f=await fixture(t);const stage=await f.create();f.state.gate=defer();
  let settled=false;const pending=f.publish(stage).then(done=>{settled=true;return done;});
  await f.state.entered.promise;
  assert.equal(f.bridge.cancel(999,TARGET),false);assert.equal(f.bridge.cancel(101,'new-generation'),false);
  assert.equal(f.bridge.cancel(101,TARGET),true);
  assert.equal(f.state.cancelCount,1);
  const busy=await f.publish(stage,102);assert.equal(busy.result,'error');assert.match(busy.error,/in progress/);
  await new Promise(resolve=>setImmediate(resolve));assert.equal(settled,false);
  assert.equal(await fs.readFile(path.join(f.copyCalls[0].source,'image.png'),'utf8'),'fresh output root image');
  f.state.gate.resolve();const done=await pending;
  assert.equal(done.result,'cancel');assert.equal(done.partialPossible,true);
  assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'cancelled');
  assert.equal(f.bridge.cancel(101,TARGET),false);
});

test('journal sync failure before provider copy cannot publish',async t=>{
  const f=await fixture(t);const stage=await f.create();f.faults.add('journal-sync:copying');
  const done=await f.publish(stage);
  assert.equal(done.result,'error');assert.equal(done.partialPossible,false);assert.equal(f.copyCalls.length,0);
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
  assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'failed');
});

test('journal failure after completed provider copy rejects confirmation and preserves recovery materials',async t=>{
  const f=await fixture(t);const stage=await f.create();f.faults.add('journal-sync:complete');
  const done=await f.publish(stage);
  assert.equal(done.result,'error');assert.equal(done.partialPossible,true);assert.equal(done.publishedCount,0);
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
  assert.equal(await fs.readFile(path.join(f.destination,done.publicationName,'image.png'),'utf8'),'fresh output root image');
});

test('failed journal updates retain the earlier durable journal and full local tree',async t=>{
  const f=await fixture(t);const stage=await f.create();f.state.copyMode='partial-error';f.faults.add('journal-sync:failed');
  const done=await f.publish(stage);
  assert.equal(done.result,'error');assert.equal(done.partialPossible,true);assert.match(done.error,/record could not be updated/);
  assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'copying');
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
});

test('progress callback failure cancels provider signal, returns error, and retains both results',async t=>{
  const f=await fixture(t);const stage=await f.create();f.state.progress=[{processedSize:1,totalSize:30}];
  const done=await f.publish(stage,101,()=>{throw new Error('consumer gone');});
  assert.equal(done.result,'error');assert.equal(done.partialPossible,true);assert.equal(f.state.cancelCount,1);
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
});

test('invalid progress cancels without accepting NaN or over-limit accounting',async t=>{
  const f=await fixture(t);const stage=await f.create();f.state.progress=[{processedSize:NaN,totalSize:30}];
  const done=await f.publish(stage);
  assert.equal(done.result,'error');assert.equal(done.partialPossible,true);assert.equal(f.state.cancelCount,1);
});

test('known local random-name collision is skipped without overwriting its contents',async t=>{
  const f=await fixture(t);const stage=await f.create();const collision=crypto.randomUUID();
  const existing=path.join(stage.ownerRoot,'PhotoCraft-Export-'+collision);await fs.mkdir(existing);
  await fs.writeFile(path.join(existing,'keep'),'known directory must survive');f.uuids.push(collision,crypto.randomUUID());
  const done=await f.publish(stage);
  assert.equal(done.result,'success');assert.notEqual(done.publicationName,'PhotoCraft-Export-'+collision);
  assert.equal(await fs.readFile(path.join(existing,'keep'),'utf8'),'known directory must survive');
});

test('CSPRNG failure has no timestamp fallback and cannot start the provider',async t=>{
  const f=await fixture(t);const stage=await f.create();f.state.uuidFailure=true;
  const done=await f.publish(stage);
  assert.equal(done.result,'error');assert.equal(done.publicationName,'');assert.equal(f.copyCalls.length,0);
  assert.equal(await fs.readFile(path.join(stage.stageRoot,'image.png'),'utf8'),'fresh output root image');
  await fs.access(stage.journalPath);
});

test('a dangling symlink occupying a random basename counts as a collision and is never replaced',async t=>{
  const f=await fixture(t);const stage=await f.create();const collision=crypto.randomUUID();
  const existing=path.join(stage.ownerRoot,'PhotoCraft-Export-'+collision);
  await fs.symlink(path.join(f.directory,'does-not-exist'),existing);f.uuids.push(collision,crypto.randomUUID());
  const done=await f.publish(stage);
  assert.equal(done.result,'success');assert.notEqual(done.publicationName,'PhotoCraft-Export-'+collision);
  assert.equal((await fs.lstat(existing)).isSymbolicLink(),true);
});

test('metadata symlink is refused before any publication and its outside contents survive',async t=>{
  const f=await fixture(t);const stage=await f.create();const outside=path.join(f.directory,'outside-record');
  await fs.writeFile(outside,'outside original');
  await fs.symlink(outside,path.join(stage.ownerRoot,'manifest.json'));
  const done=await f.publish(stage);
  assert.equal(done.result,'error');assert.equal(f.copyCalls.length,0);assert.equal(f.trace.includes('picker'),false);
  assert.equal(await fs.readFile(outside,'utf8'),'outside original');
});

test('stage rename rejection preserves the original complete tree and journal',async t=>{
  const f=await fixture(t);const stage=await f.create();f.faults.add('stage-rename');
  const done=await f.publish(stage);
  assert.equal(done.result,'error');assert.equal(done.stageRoot,stage.stageRoot);assert.equal(f.copyCalls.length,0);
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
  assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'failed');
});

test('sandbox fsync failure preserves the stage and seeded durable journal without starting the picker',async t=>{
  const f=await fixture(t);const stage=await f.create();f.faults.add('sandbox-fsync');
  const done=await f.publish(stage);
  assert.equal(done.result,'error');assert.equal(f.copyCalls.length,0);assert.equal(f.trace.includes('picker'),false);
  assert.equal(await fs.readFile(path.join(stage.stageRoot,'image.png'),'utf8'),'fresh output root image');
  await fs.access(stage.journalPath);
});

for(const kind of ['file','directory']) {
  test(`stage ${kind} symlink is rejected before picker and its outside target survives`,async t=>{
    const f=await fixture(t);const stage=await f.create();const outside=path.join(f.directory,'outside');
    if(kind==='directory') {await fs.mkdir(outside);await fs.writeFile(path.join(outside,'keep'),'outside');}
    else await fs.writeFile(outside,'outside');
    await fs.symlink(outside,path.join(stage.stageRoot,'link'));
    const done=await f.publish(stage);
    assert.equal(done.result,'error');assert.equal(f.trace.includes('picker'),false);assert.equal(f.copyCalls.length,0);
    assert.equal(await fs.readFile(kind==='directory'?path.join(outside,'keep'):outside,'utf8'),'outside');
    assert.equal((await fs.lstat(path.join(stage.stageRoot,'link'))).isSymbolicLink(),true);
  });
}

test('wrong target, outside source, empty output, and over-limit tree do not trigger a picker',async t=>{
  const f=await fixture(t);const stage=await f.create();
  const wrong=await f.bridge.publishFolder(f.request(stage,101,'replaced-owner'),()=>{});
  assert.equal(wrong.result,'error');
  const outside=await f.publish({...stage,stageRoot:f.destination},102);assert.equal(outside.result,'error');
  const big=f.request(stage,103);big.limits.maxBytes=1;assert.equal((await f.bridge.publishFolder(big,()=>{})).result,'error');
  const empty=await f.bridge.createStage(200,TARGET);assert.equal((await f.publish(empty,104)).result,'error');
  assert.equal(f.trace.includes('picker'),false);assert.equal(f.copyCalls.length,0);
  assert.equal(await fs.readFile(path.join(stage.stageRoot,'image.png'),'utf8'),'fresh output root image');
});

test('root ancestors must be ordinary existing sandbox directories and cannot be links',async t=>{
  const f=await fixture(t);const outside=path.join(f.directory,'outside');await fs.mkdir(outside);
  await fs.symlink(outside,path.join(f.filesDir,'PhotoCraft'));
  await assert.rejects(f.bridge.createStage(1,TARGET),/Links will not be followed/);
  assert.deepEqual(await fs.readdir(outside),[]);assert.equal(f.copyCalls.length,0);
});

test('new helper instance reports retained incomplete journals without replay, external deletion, or URI disclosure',async t=>{
  const f=await fixture(t);const stage=await f.create();f.state.copyMode='partial-error';const done=await f.publish(stage);
  const restarted=new f.production.FolderPublicationBridge({filesDir:f.filesDir});
  f.trace.length=0;const recovery=await restarted.discoverRecovery();
  assert.equal(recovery.length,1);assert.equal(recovery[0].stageRoot,done.stageRoot);assert.equal(recovery[0].partialPossible,true);
  assert.equal(f.trace.includes('provider-copy'),false);assert.equal(f.trace.includes('activate'),false);
  assert(!JSON.stringify(recovery).includes(URI));
  assert.equal(await fs.readFile(path.join(f.destination,done.publicationName,'partial.png'),'utf8'),'partial');
});

test('duplicate attempt id is rejected; repeated jobs and retries do not fail mkdir on existing parents',async t=>{
  const f=await fixture(t);const stage=await f.create();const first=await f.publish(stage);
  const duplicate=await f.publish({...stage,stageRoot:first.stageRoot},101);assert.equal(duplicate.result,'error');
  assert.equal(f.copyCalls.length,1);
  const next=await f.create();assert.notEqual(next.ownerRoot,stage.ownerRoot);
  assert.equal((await f.publish(next,102)).result,'success');assert.equal(f.copyCalls.length,2);
});
