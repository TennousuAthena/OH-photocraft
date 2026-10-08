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

function loadProduction(entry, kits, overrides={}) {
  const modules = new Map();
  function load(filename) {
    if (modules.has(filename)) return modules.get(filename).exports;
    const loaded = new Module(filename, module);
    modules.set(filename, loaded);
    loaded.filename = filename;
    loaded.require = specifier => {
      if (kits[specifier]) return kits[specifier];
      if (overrides[specifier]) return overrides[specifier];
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

async function fixture(t) {
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
    async unlink(location) {await fs.unlink(location);},
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
  const bridge = new production.FolderPublicationBridge({filesDir});
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
    stageRoot:stage.stageRoot,limits:{maxFiles:500,maxBytes:1024*1024},policy:'freshSubdirectory'};}
  async function publish(stage,id=101,callback=()=>{}) {return bridge.publishFolder(request(stage,id),callback);}
  return {directory,filesDir,destination,bridge,kits,production,trace,faults,uuids,logs,state,copyCalls,create,request,publish,handles};
}

const tick=()=>new Promise(resolve=>setImmediate(resolve));
async function until(predicate){for(let i=0;i<200;i++){if(await predicate())return;await new Promise(resolve=>setTimeout(resolve,5));}throw new Error('fixture did not settle');}
function coordinate(t,f,overrides={}) {
  const {FolderPublicationCoordinator}=loadProduction('FolderPublicationCoordinator',f.kits);
  const state={accepted:true,available:true}, inputs=[], attempts=[], activity=[], errors=[];
  const coordinator=new FolderPublicationCoordinator(overrides.publish||((request,progress)=>f.bridge.publishFolder(request,progress)),
    (id,target)=>f.bridge.cancel(id,target), serialized=>{
      const event=JSON.parse(serialized);attempts.push(event);
      if(state.accepted){inputs.push(event);return true;}return false;
    },()=>state.available,message=>errors.push(message),active=>activity.push(active));
  t.after(async()=>{f.state.gate?.resolve();coordinator.dispose();await coordinator.cancelAndWait();});
  return {coordinator,state,inputs,attempts,activity,errors};
}

test('completed immutable output uses real manifest count, exact target receipt and preserves unrelated destination root',async t=>{
  const f=await fixture(t), c=coordinate(t,f), stage=await f.create();
  c.coordinator.begin(f.request(stage));await until(()=>c.inputs.some(value=>value.kind==='folderPublishComplete'));
  const done=c.inputs.find(value=>value.kind==='folderPublishComplete');
  assert.equal(done.target,TARGET);assert.equal(done.result,'success');assert.equal(done.publishedCount,2);
  assert.equal(await fs.readFile(path.join(f.destination,'image.png'),'utf8'),'unrelated original root image');
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
  assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'complete');
});

test('mailbox backpressure keeps actual successful receipt and cannot turn already published output into Cancel',async t=>{
  const f=await fixture(t), c=coordinate(t,f), stage=await f.create();c.state.accepted=false;
  c.coordinator.begin(f.request(stage));await until(()=>c.attempts.some(value=>value.kind==='folderPublishComplete'));
  assert.equal(c.coordinator.hasPending(),true);assert.equal(c.inputs.length,0);
  assert.equal(c.coordinator.cancel(101,TARGET),false);c.coordinator.poll();
  assert.equal(c.attempts.at(-1).result,'success');assert.equal(f.copyCalls.length,1);
  c.state.accepted=true;c.coordinator.poll();assert.equal(c.coordinator.hasPending(),false);
  assert.equal(c.inputs.at(-1).publishedCount,2);assert.equal(c.inputs.at(-1).result,'success');
});

test('matching independent Cancel waits for provider settlement and keeps full stage and possible external partial',async t=>{
  const f=await fixture(t), c=coordinate(t,f), stage=await f.create();f.state.gate=defer();
  c.coordinator.begin(f.request(stage));await f.state.entered.promise;
  assert.equal(c.coordinator.cancel(101,'other-target'),false);assert.equal(c.coordinator.cancel(100,TARGET),false);
  assert.equal(c.coordinator.cancel(101,TARGET),true);assert.equal(f.state.cancelCount,1);
  let settled=false;const waiting=c.coordinator.cancelAndWait().then(()=>settled=true);await tick();
  assert.equal(settled,false);assert(!c.inputs.some(value=>value.kind==='folderPublishComplete'));
  f.state.gate.resolve();await waiting;await until(()=>c.inputs.some(value=>value.kind==='folderPublishComplete'));
  const done=c.inputs.find(value=>value.kind==='folderPublishComplete');
  assert.equal(done.result,'cancel');assert.equal(done.partialPossible,true);assert.equal(done.publishedCount,0);
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
  assert.equal(JSON.parse(await fs.readFile(done.journalPath,'utf8')).phase,'cancelled');
});

test('distinct concurrent request is rejected without releasing original stage, duplicate id never publishes twice',async t=>{
  const f=await fixture(t), c=coordinate(t,f), stage=await f.create();f.state.gate=defer();
  c.coordinator.begin(f.request(stage));await f.state.entered.promise;
  c.coordinator.begin(f.request(stage,101,'new-target'));c.coordinator.begin(f.request(stage,102));
  assert.equal(c.inputs.find(value=>value.id===102).result,'error');assert.equal(f.copyCalls.length,1);
  assert.equal(await fs.readFile(path.join(stage.ownerRoot,await fs.readdir(stage.ownerRoot).then(names=>names.find(name=>name.startsWith('PhotoCraft-Export-'))),'image.png'),'utf8'),'fresh output root image');
  f.state.gate.resolve();await until(()=>c.inputs.some(value=>value.id===101&&value.kind==='folderPublishComplete'));
  c.coordinator.begin(f.request(stage));assert.equal(f.copyCalls.length,1);
});

test('provider partial failure receipt preserves durable output and retry uses a new random destination child',async t=>{
  const f=await fixture(t), c=coordinate(t,f), stage=await f.create();f.state.copyMode='partial-error';
  c.coordinator.begin(f.request(stage));await until(()=>c.inputs.some(value=>value.kind==='folderPublishComplete'));
  const failed=c.inputs.find(value=>value.kind==='folderPublishComplete');
  assert.equal(failed.result,'error');assert.equal(failed.partialPossible,true);assert.equal(failed.publishedCount,0);
  assert.equal(JSON.parse(await fs.readFile(failed.journalPath,'utf8')).phase,'failed');
  assert.equal(await fs.readFile(path.join(failed.stageRoot,'image.png'),'utf8'),'fresh output root image');
  f.state.copyMode='success';c.coordinator.begin(f.request(failed,102));
  await until(()=>c.inputs.some(value=>value.id===102&&value.kind==='folderPublishComplete'));
  const done=c.inputs.find(value=>value.id===102&&value.kind==='folderPublishComplete');
  assert.equal(done.result,'success');assert.notEqual(done.publicationName,failed.publicationName);
  assert.equal(await fs.readFile(path.join(f.destination,'image.png'),'utf8'),'unrelated original root image');
});

test('page disposal waits on provider but sends no late receipt and retains complete recovery materials',async t=>{
  const f=await fixture(t), c=coordinate(t,f), stage=await f.create();f.state.gate=defer();
  c.coordinator.begin(f.request(stage));await f.state.entered.promise;c.coordinator.dispose();
  f.state.gate.resolve();await c.coordinator.cancelAndWait();
  assert(!c.inputs.some(value=>value.kind==='folderPublishComplete'));
  const journal=JSON.parse(await fs.readFile(stage.journalPath,'utf8'));
  assert.equal(journal.phase,'cancelled');assert.equal(await fs.readFile(path.join(journal.stageRoot,'image.png'),'utf8'),'fresh output root image');
});

test('busy file operation, invalid policy and invalid cap terminate without touching output or opening picker',async t=>{
  const f=await fixture(t), c=coordinate(t,f), stage=await f.create();c.state.available=false;
  c.coordinator.begin(f.request(stage));assert.equal(c.inputs.at(-1).result,'error');c.state.available=true;
  c.coordinator.begin({...f.request(stage,102),policy:'merge'});
  c.coordinator.begin({...f.request(stage,103),limits:{maxFiles:501,maxBytes:1024}});
  assert.equal(c.inputs.filter(value=>value.kind==='folderPublishComplete').length,3);
  assert(!f.trace.includes('picker'));assert.equal(await fs.readFile(path.join(stage.stageRoot,'image.png'),'utf8'),'fresh output root image');
});

test('malformed helper receipt or escaped failure is rejected without assuming zero provider side effects',async t=>{
  const f=await fixture(t), stage=await f.create();let count=0;
  const c=coordinate(t,f,{publish:async request=>{
    if(count++)throw new Error('provider secret must not reach product UI');
    return {kind:'folderPublishComplete',id:request.id,target:'forged-target',result:'success',ownerRoot:stage.ownerRoot,
      stageRoot:stage.stageRoot,journalPath:stage.journalPath,publicationName:'fake',publishedCount:2,partialPossible:false,retainStage:true,error:''};
  }});
  c.coordinator.begin(f.request(stage));await until(()=>c.inputs.length>0);
  assert.equal(c.inputs.at(-1).result,'error');assert.equal(c.inputs.at(-1).partialPossible,true);
  c.coordinator.begin(f.request(stage,102));await until(()=>c.inputs.some(value=>value.id===102));
  assert.equal(c.inputs.at(-1).result,'error');assert.equal(c.inputs.at(-1).partialPossible,true);
  assert(!c.inputs.at(-1).error.includes('secret'));assert.equal(await fs.readFile(path.join(stage.stageRoot,'image.png'),'utf8'),'fresh output root image');
});

function destination(f) {
  const {FolderDestinationBridge}=loadProduction('FolderDestinationBridge',f.kits);
  const bridge=new FolderDestinationBridge({filesDir:f.filesDir});
  const request=(id=7,target=TARGET)=>({kind:'folderDestination',id,target,intent:'file.scripts.imageProcessor'});
  return {bridge,request};
}

test('Output Browse returns a durable opaque handle, keeps URI private and reactivates only the original target',async t=>{
  const f=await fixture(t), d=destination(f);const done=await d.bridge.select(d.request());
  assert.equal(done.result,'success');assert.match(done.destinationHandle,/^[0-9a-f-]{36}$/);
  assert(!JSON.stringify(done).includes(URI));assert.equal(await d.bridge.resolve(done.destinationHandle,TARGET),URI);
  await assert.rejects(d.bridge.resolve(done.destinationHandle,'new-form-generation'),/belongs to another task/);
  const file=path.join(f.filesDir,'PhotoCraft/Documents/FolderDestinations',done.destinationHandle+'.json');
  const record=JSON.parse(await fs.readFile(file,'utf8'));
  assert.equal(record.uri,URI);assert.equal(record.target,TARGET);assert.equal(record.id,7);
  assert.equal(await fs.readFile(path.join(f.destination,'image.png'),'utf8'),'unrelated original root image');
  await d.bridge.release(done.destinationHandle,TARGET);await assert.rejects(fs.access(file));
});

test('Output Browse permission rejection or picker Cancel creates no ready handle and does not modify provider',async t=>{
  const f=await fixture(t), d=destination(f);f.state.permission=false;const failed=await d.bridge.select(d.request());
  assert.equal(failed.result,'error');assert.equal(failed.destinationHandle,'');assert(!failed.error.includes(URI));
  f.state.permission=true;f.state.selected=[];const cancelled=await d.bridge.select(d.request(8));
  assert.equal(cancelled.result,'cancel');assert.equal(cancelled.destinationHandle,'');assert.equal(f.copyCalls.length,0);
  assert.equal(await fs.readFile(path.join(f.destination,'image.png'),'utf8'),'unrelated original root image');
});

test('retained destination handle is never rebound after grant expiry; symlink and forged marker cannot read or remove another record',async t=>{
  const f=await fixture(t), d=destination(f);const done=await d.bridge.select(d.request());
  f.state.permission=false;await assert.rejects(d.bridge.resolve(done.destinationHandle,TARGET),/permission has expired/);
  f.state.permission=true;const file=path.join(f.filesDir,'PhotoCraft/Documents/FolderDestinations',done.destinationHandle+'.json');
  const original=await fs.readFile(file,'utf8');await fs.unlink(file);
  const sentinel=path.join(f.directory,'outside.json');await fs.writeFile(sentinel,original);await fs.symlink(sentinel,file);
  await assert.rejects(d.bridge.resolve(done.destinationHandle,TARGET),/Invalid folder selection record/);
  await assert.rejects(d.bridge.release(done.destinationHandle,TARGET),/Invalid folder selection record/);
  assert.equal(await fs.readFile(sentinel,'utf8'),original);
});

test('known handle or dangling next-file UUID collision is skipped without replacing retained record',async t=>{
  const f=await fixture(t), d=destination(f);const uuid='00000000-0000-4000-8000-000000000001';
  f.uuids.push(uuid);const first=await d.bridge.select(d.request());assert.equal(first.result,'success');
  f.uuids.push(uuid,'00000000-0000-4000-8000-000000000002');const second=await d.bridge.select(d.request(8));
  assert.equal(second.result,'success');assert.notEqual(second.destinationHandle,first.destinationHandle);
  assert.equal(await d.bridge.resolve(first.destinationHandle,TARGET),URI);
});

test('unsafe handle path, malformed target and unsupported intent are rejected before provider access',async t=>{
  const f=await fixture(t), d=destination(f);
  await assert.rejects(d.bridge.resolve('../outside',TARGET),/Invalid folder selection identifier/);
  await assert.rejects(d.bridge.release('00000000-0000-4000-8000-000000000001','bad\x00target'),/Invalid folder selection identifier/);
  const done=await d.bridge.select({...d.request(),intent:'file.automate.unsupported'});
  assert.equal(done.result,'error');assert(!f.trace.includes('picker'));assert.equal(f.copyCalls.length,0);
});

function outputCoordinate(t,f) {
  const d=destination(f);
  const {FolderPublicationBridge}=loadProduction('FolderPublicationBridge',f.kits);
  const publisher=new FolderPublicationBridge({filesDir:f.filesDir},(handle,target)=>d.bridge.resolve(handle,target));
  const {FolderStageBridge}=loadProduction('FolderStageBridge',f.kits), stageService=new FolderStageBridge(d.bridge,publisher);
  const {FolderOutputCoordinator}=loadProduction('FolderOutputCoordinator',f.kits);
  const state={accepted:true,available:true}, inputs=[], attempts=[], released=[], errors=[];
  const coordinator=new FolderOutputCoordinator(request=>d.bridge.select(request),request=>stageService.allocate(request),
    (id,target)=>d.bridge.cancel(id,target),async(handle,target)=>{released.push({handle,target});await d.bridge.release(handle,target);},
    serialized=>{const value=JSON.parse(serialized);attempts.push(value);if(state.accepted){inputs.push(value);return true;}return false;},
    ()=>state.available,message=>errors.push(message),()=>{});
  t.after(async()=>{coordinator.dispose();await coordinator.cancelAndWait();});
  const allocate=(handle,id=8,target=TARGET)=>({kind:'folderStageAllocate',id,target,intent:'file.scripts.imageProcessor',destinationHandle:handle});
  return {...d,coordinator,state,inputs,attempts,released,errors,allocate,publisher};
}

test('preauthorized Output Browse allocates an exact original-target empty owned stage; publish uses handle with no second picker',async t=>{
  const f=await fixture(t), c=outputCoordinate(t,f);
  c.coordinator.begin(c.request());await until(()=>c.inputs.some(value=>value.kind==='folderDestinationComplete'));
  const selected=c.inputs.find(value=>value.kind==='folderDestinationComplete');
  c.coordinator.begin(c.allocate(selected.destinationHandle));await until(()=>c.inputs.some(value=>value.kind==='folderStageComplete'));
  const stage=c.inputs.find(value=>value.kind==='folderStageComplete');
  assert.equal(stage.result,'success');assert.match(stage.ownerRoot,/\/8-[a-zA-Z0-9]{6}$/);
  assert.deepEqual(await fs.readdir(stage.stageRoot),[]);
  const owner=JSON.parse(await fs.readFile(path.join(stage.ownerRoot,'owner.json'),'utf8'));
  assert.equal(owner.target,TARGET);assert.equal(owner.creationId,8);
  await fs.writeFile(path.join(stage.stageRoot,'a.png'),'encoded snapshot');
  const done=await c.publisher.publishFolder({...f.request(stage,9),destinationHandle:selected.destinationHandle},()=>{});
  assert.equal(done.result,'success');assert.equal(done.publishedCount,1);assert.equal(f.trace.filter(value=>value==='picker').length,1);
  assert.equal(await fs.readFile(path.join(f.destination,done.publicationName,'a.png'),'utf8'),'encoded snapshot');
});

test('only a matching unaccepted Output Browse cancel releases its private handle; accepted duplicate and late cancel never release',async t=>{
  const f=await fixture(t), c=outputCoordinate(t,f);c.state.accepted=false;
  c.coordinator.begin(c.request());await until(()=>c.attempts.some(value=>value.result==='success'));
  const selected=c.attempts.find(value=>value.result==='success');
  assert.equal(c.coordinator.cancel('folderDestination',7,'new-form'),false);
  assert.equal(c.coordinator.cancel('folderDestination',7,TARGET),true);
  await until(()=>c.released.length===1&&c.attempts.some(value=>value.result==='cancel'));
  assert.equal(c.released[0].handle,selected.destinationHandle);
  await assert.rejects(c.bridge.resolve(selected.destinationHandle,TARGET));
  c.state.accepted=true;c.coordinator.poll();c.coordinator.begin(c.request(9));
  await until(()=>c.inputs.some(value=>value.id===9&&value.result==='success'));
  const accepted=c.inputs.find(value=>value.id===9&&value.result==='success');
  c.coordinator.begin(c.request(9,'changed-generation'));assert.equal(c.coordinator.cancel('folderDestination',9,TARGET),false);
  assert.equal(c.released.length,1);assert.equal(await c.bridge.resolve(accepted.destinationHandle,TARGET),URI);
});

test('stage queue backpressure and matching Cancel transfer a cancel receipt with owned empty root for Rust validation',async t=>{
  const f=await fixture(t), c=outputCoordinate(t,f);const selected=await c.bridge.select(c.request());
  c.state.accepted=false;c.coordinator.begin(c.allocate(selected.destinationHandle));
  await until(()=>c.attempts.some(value=>value.kind==='folderStageComplete'));
  const pending=c.attempts.find(value=>value.kind==='folderStageComplete');assert.equal(pending.result,'success');
  assert.equal(c.coordinator.cancel('folderDestination',8,TARGET),false);
  assert.equal(c.coordinator.cancel('folderStageAllocate',8,TARGET),true);
  await until(()=>c.attempts.some(value=>value.result==='cancel'));
  const cancelled=c.attempts.find(value=>value.result==='cancel');assert.equal(cancelled.ownerRoot,pending.ownerRoot);
  assert.deepEqual(await fs.readdir(cancelled.stageRoot),[]);assert.equal(c.released.length,0);
  c.state.accepted=true;c.coordinator.poll();assert.equal(c.inputs.at(-1).result,'cancel');
});

test('selection disposal after delayed picker returns removes only its still-owned handle and emits no late completion',async t=>{
  const f=await fixture(t), c=outputCoordinate(t,f), gate=defer(), entered=defer();
  f.kits['@kit.CoreFileKit'].picker.DocumentViewPicker=class{async select(){entered.resolve();await gate.promise;return [URI];}};
  c.coordinator.begin(c.request());await entered.promise;c.coordinator.dispose();gate.resolve();await c.coordinator.cancelAndWait();
  assert.equal(c.inputs.length,0);assert.equal(c.coordinator.hasPending(),false);
  assert.equal(f.copyCalls.length,0);assert.equal(await fs.readFile(path.join(f.destination,'image.png'),'utf8'),'unrelated original root image');
});

test('stage allocation refuses expired handle and changed target without creating arbitrary output directory',async t=>{
  const f=await fixture(t), c=outputCoordinate(t,f);const selected=await c.bridge.select(c.request());
  c.coordinator.begin(c.allocate(selected.destinationHandle,8,'changed-generation'));
  await until(()=>c.inputs.some(value=>value.id===8));assert.equal(c.inputs.at(-1).result,'error');assert.equal(c.inputs.at(-1).ownerRoot,'');
  f.state.permission=false;c.coordinator.begin(c.allocate(selected.destinationHandle,9));
  await until(()=>c.inputs.some(value=>value.id===9));assert.equal(c.inputs.at(-1).result,'error');
  await assert.rejects(fs.access(path.join(f.filesDir,'PhotoCraft/Documents/FolderPublication')));
});

test('publication handle cannot be substituted by a new form target and never falls back to picker',async t=>{
  const f=await fixture(t), d=destination(f), selected=await d.bridge.select(d.request());
  const {FolderPublicationBridge}=loadProduction('FolderPublicationBridge',f.kits);
  const publisher=new FolderPublicationBridge({filesDir:f.filesDir},(handle,target)=>d.bridge.resolve(handle,target));
  const stage=await publisher.createStage(11,'different-form-target');await fs.writeFile(path.join(stage.stageRoot,'encoded.psd'),'8BPS');
  const done=await publisher.publishFolder({...f.request(stage,12,'different-form-target'),destinationHandle:selected.destinationHandle},()=>{});
  assert.equal(done.result,'error');assert.equal(done.partialPossible,false);assert.equal(done.publishedCount,0);
  assert.equal(f.copyCalls.length,0);assert.equal(f.trace.filter(value=>value==='picker').length,1);
  assert.equal(await fs.readFile(path.join(done.stageRoot,'encoded.psd'),'utf8'),'8BPS');
});

async function makePlatform(t,f) {
  const outputs=[],inputs=[],errors=[];let terminated=false;
  const native={platformInput:value=>{inputs.push(JSON.parse(value));return true;},takePlatformOutput:()=>outputs.shift()||'',
    releasePlatform(){},applyPlatformOutput(){return false;}};
  const kits={...f.kits,'@kit.ArkUI':{window:{getLastWindow:async()=>({on(){},off(){},getWindowProperties:()=>({id:1})})}},
    'libphotocraft.so':{default:native}};
  const overrides={
    './ClipboardBridge':{ClipboardBridge:class{}},'./PasteAuthorization':{ClipboardReadCancelled:class extends Error{}},
    './PrintingBridge':{PrintingBridge:class{dispose(){}async submitPdf(){return false;}}},
    './ViewportBridge':{ViewportBridge:class{initialize(){}dispose(){}async apply(){}}}
  };
  const {PlatformBridge}=loadProduction('PlatformBridge',kits,overrides);
  const platform=new PlatformBridge({filesDir:f.filesDir,terminateSelf:async()=>{terminated=true;}},
    {getComponentUtils:()=>({getRectangleById:()=>({size:{width:800,height:600},screenOffset:{x:30,y:40}})})},
    message=>errors.push(message),()=>{},async()=>({text:'',imagePath:''}),()=>{},()=>true);
  await platform.initialize();
  t.after(async()=>{f.state.gate?.resolve();platform.dispose();await Promise.all([
    platform.folders.cancelAndWait(),platform.outputFolders.cancelAndWait(),platform.publications.cancelAndWait()]);});
  const send=events=>{outputs.push(JSON.stringify({events,closeState:null}));platform.poll();};
  return {platform,send,inputs,errors,get terminated(){return terminated;}};
}

test('real platform dispatch consumes folderPublishCancel while long provider copy is unfinished; normal running remains free',async t=>{
  const f=await fixture(t), d=destination(f), selected=await d.bridge.select(d.request()), stage=await f.create();
  const p=await makePlatform(t,f);f.state.gate=defer();
  p.send([{...f.request(stage),destinationHandle:selected.destinationHandle}]);await f.state.entered.promise;
  assert.equal(p.platform.running,false);assert.equal(p.platform.hasFolderWork(),true);
  p.send([{kind:'folderPublishCancel',id:101,target:TARGET}]);assert.equal(f.state.cancelCount,1);
  f.state.gate.resolve();await until(()=>p.inputs.some(value=>value.kind==='folderPublishComplete'));
  assert.equal(p.inputs.find(value=>value.kind==='folderPublishComplete').result,'cancel');
});

test('safe close waits for output provider settlement and retains cancelled journal before terminating ability',async t=>{
  const f=await fixture(t), d=destination(f), selected=await d.bridge.select(d.request()), stage=await f.create();
  const p=await makePlatform(t,f);f.state.gate=defer();
  p.send([{...f.request(stage),destinationHandle:selected.destinationHandle}]);await f.state.entered.promise;
  p.send([{kind:'close',id:999}]);await tick();assert.equal(p.terminated,false);assert(f.state.cancelCount>0);
  f.state.gate.resolve();await until(()=>p.terminated);
  const journal=JSON.parse(await fs.readFile(stage.journalPath,'utf8'));assert.equal(journal.phase,'cancelled');
  assert.equal(await fs.readFile(path.join(journal.stageRoot,'image.png'),'utf8'),'fresh output root image');
});

test('platform Output Browse allocation and publication preserve the captured identity with distinct ids and exactly one picker',async t=>{
  const f=await fixture(t), p=await makePlatform(t,f);const request={kind:'folderDestination',id:20,target:TARGET,intent:'file.scripts.imageProcessor'};
  p.send([request]);await until(()=>p.inputs.some(value=>value.kind==='folderDestinationComplete'));
  const selected=p.inputs.find(value=>value.kind==='folderDestinationComplete');
  p.send([{kind:'folderStageAllocate',id:21,target:TARGET,intent:request.intent,destinationHandle:selected.destinationHandle}]);
  await until(()=>p.inputs.some(value=>value.kind==='folderStageComplete'));
  const stage=p.inputs.find(value=>value.kind==='folderStageComplete');assert.equal(stage.result,'success');
  await fs.writeFile(path.join(stage.stageRoot,'真实输出.png'),'processed-encoded-by-engine');
  p.send([{...f.request(stage,22),destinationHandle:selected.destinationHandle}]);
  await until(()=>p.inputs.some(value=>value.kind==='folderPublishComplete'));
  const done=p.inputs.find(value=>value.kind==='folderPublishComplete');
  assert.equal(done.result,'success');assert.equal(done.publishedCount,1);assert.equal(done.target,TARGET);
  assert.equal(f.trace.filter(value=>value==='picker').length,1);
  assert.equal(await fs.readFile(path.join(f.destination,done.publicationName,'真实输出.png'),'utf8'),'processed-encoded-by-engine');
  p.send([{kind:'folderDestinationRelease',id:23,target:TARGET,destinationHandle:selected.destinationHandle}]);
  await until(()=>fs.access(path.join(f.filesDir,'PhotoCraft/Documents/FolderDestinations',selected.destinationHandle+'.json')).then(()=>false,()=>true));
});

test('background IME suspension leaves output copy running; explicit page hide cancels and retains complete stage',async t=>{
  const f=await fixture(t), d=destination(f), selected=await d.bridge.select(d.request()), stage=await f.create();
  const p=await makePlatform(t,f);f.state.gate=defer();
  p.send([{...f.request(stage),destinationHandle:selected.destinationHandle}]);await f.state.entered.promise;
  p.platform.suspend();assert.equal(f.state.cancelCount,0);
  p.platform.setPageVisible(false);assert.equal(f.state.cancelCount,1);
  f.state.gate.resolve();await until(()=>p.inputs.some(value=>value.kind==='folderPublishComplete'));
  const done=p.inputs.find(value=>value.kind==='folderPublishComplete');assert.equal(done.result,'cancel');
  assert.equal(await fs.readFile(path.join(done.stageRoot,'image.png'),'utf8'),'fresh output root image');
});

test('platform StageCancel matches allocation identity during authorization and hands cancelled empty tree to Rust only after allocation settles',async t=>{
  const f=await fixture(t), d=destination(f), selected=await d.bridge.select(d.request()), p=await makePlatform(t,f);
  const gate=defer(), entered=defer(), original=p.platform.destinations.resolve.bind(p.platform.destinations);
  p.platform.destinations.resolve=async(...args)=>{entered.resolve();await gate.promise;return original(...args);};
  p.send([{kind:'folderStageAllocate',id:31,target:TARGET,intent:'file.scripts.imageProcessor',destinationHandle:selected.destinationHandle}]);
  await entered.promise;p.send([{kind:'folderStageCancel',id:31,target:'different-generation'}]);
  assert.equal(p.platform.outputFolders.active.cancelled,false);
  p.send([{kind:'folderStageCancel',id:31,target:TARGET}]);assert.equal(p.platform.outputFolders.active.cancelled,true);
  assert(!p.inputs.some(value=>value.kind==='folderStageComplete'));gate.resolve();
  await until(()=>p.inputs.some(value=>value.kind==='folderStageComplete'));
  const done=p.inputs.find(value=>value.kind==='folderStageComplete');assert.equal(done.result,'cancel');
  assert.equal(done.destinationHandle,selected.destinationHandle);assert.match(done.ownerRoot,/\/31-[A-Za-z0-9]{6}$/);
  assert.deepEqual(await fs.readdir(done.stageRoot),[]);
});

for(const intent of ['file.automate.batch','file.automate.lensCorrection']) test(`original ${intent} output Browse preauthorizes, allocates, and publishes using the same target and one picker`,async t=>{
  const f=await fixture(t), p=await makePlatform(t,f);const request={kind:'folderDestination',id:20,target:TARGET,intent};
  p.send([request]);await until(()=>p.inputs.some(value=>value.kind==='folderDestinationComplete'));
  const selected=p.inputs.find(value=>value.kind==='folderDestinationComplete');
  p.send([{kind:'folderStageAllocate',id:21,target:TARGET,intent:request.intent,destinationHandle:selected.destinationHandle}]);
  await until(()=>p.inputs.some(value=>value.kind==='folderStageComplete'));
  const stage=p.inputs.find(value=>value.kind==='folderStageComplete');assert.equal(stage.result,'success');
  await fs.writeFile(path.join(stage.stageRoot,'真实输出.png'),'processed-encoded-by-engine');
  p.send([{...f.request(stage,22),destinationHandle:selected.destinationHandle}]);
  await until(()=>p.inputs.some(value=>value.kind==='folderPublishComplete'));
  const done=p.inputs.find(value=>value.kind==='folderPublishComplete');
  assert.equal(done.result,'success');assert.equal(done.publishedCount,1);assert.equal(done.target,TARGET);
  assert.equal(f.trace.filter(value=>value==='picker').length,1);
  assert.equal(await fs.readFile(path.join(f.destination,done.publicationName,'真实输出.png'),'utf8'),'processed-encoded-by-engine');
  p.send([{kind:'folderDestinationRelease',id:23,target:TARGET,destinationHandle:selected.destinationHandle}]);
  await until(()=>fs.access(path.join(f.filesDir,'PhotoCraft/Documents/FolderDestinations',selected.destinationHandle+'.json')).then(()=>false,()=>true));
});
