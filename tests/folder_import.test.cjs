'use strict';
const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs/promises');
const syncFs=require('node:fs');
const os=require('node:os');
const path=require('node:path');
const Module=require('node:module');
const ts=require(process.env.ARKTS_TYPESCRIPT_PATH ||
  '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/build-tools/ets-loader/node_modules/typescript/lib/typescript.js');
const platformRoot=process.env.FOLDER_INPUT_HELPER_ROOT || path.resolve(__dirname,'../harmonyos/entry/src/main/ets/platform');
const URI='content://fixture/opaque-folder-identity-unrelated-to-name';
const TARGET='original-dialog-field-document-generation';
const LIMITS={maxFiles:500,maxBytes:1024*1024};
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function defer(){let resolve;const promise=new Promise(done=>{resolve=done;});return {promise,resolve};}
async function until(predicate){for(let count=0;count<200;count++){if(predicate())return;await new Promise(resolve=>setTimeout(resolve,5));}throw new Error('fixture did not settle');}

function loader(kits, overrides={}) {
  const modules=new Map();
  function load(filename){
    if(modules.has(filename))return modules.get(filename).exports;
    const loaded=new Module(filename,module);modules.set(filename,loaded);loaded.filename=filename;
    loaded.require=specifier=>{
      if(kits[specifier])return kits[specifier];
      if(overrides[specifier])return overrides[specifier];
      if(specifier.startsWith('./'))return load(path.resolve(path.dirname(filename),specifier+'.ets'));
      throw new Error('unexpected production import '+specifier);
    };
    loaded._compile(ts.transpileModule(syncFs.readFileSync(filename,'utf8'),{
      compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}
    }).outputText,filename);
    return loaded.exports;
  }
  return entry=>load(path.join(platformRoot,entry+'.ets'));
}

async function fixture(t) {
  const base=await fs.mkdtemp(path.join(os.tmpdir(),'photocraft-folder-input-'));
  const filesDir=path.join(base,'files');const source=path.join(base,'用户输入-例');
  await fs.mkdir(filesDir);await fs.mkdir(source);
  await fs.mkdir(path.join(source,'sub'));await fs.mkdir(path.join(source,'sub','deep'));
  await fs.writeFile(path.join(source,'顶层.png'),'PNG');
  await fs.writeFile(path.join(source,'工程.psd'),'8BPS');
  await fs.writeFile(path.join(source,'marker.txt'),'unchanged source');
  await fs.writeFile(path.join(source,'sub','deep','nested.png'),'nestedPNG');
  const handles=new Map(),trace=[],inputs=[],attempts=[],reports=[],activities=[],copyCalls=[];
  const state={selected:[URI],permission:true,layout:'wrapped',copyGate:undefined,pickerGate:undefined,
    copyEntered:defer(),pickerEntered:defer(),cancelCount:0,completionAccepted:true,progressAccepted:true,
    available:true,progressListener:undefined,copyError:false,cleanupError:false,terminated:false};
  const fault=()=>Object.assign(new Error('private provider URI '+URI),{code:13900020});
  const modes={READ_ONLY:0,WRITE_ONLY:1,CREATE:64,TRUNC:512,NOFOLLOW:0x100000};
  const fileIo={
    OpenMode:modes,
    TaskSignal:class{cancel(){state.cancelCount++;trace.push('copy-cancel');}},
    async access(location){try{await fs.access(location);return true;}catch{return false;}},
    async mkdir(location){await fs.mkdir(location);},
    async mkdtemp(template){assert(template.endsWith('-XXXXXX'));return fs.mkdtemp(template.slice(0,-6));},
    async lstat(location){try{return await fs.lstat(location);}catch(error){if(error.code==='ENOENT')error.code=13900002;throw error;}},
    async listFile(location){return fs.readdir(location);},
    async readText(location){return fs.readFile(location,'utf8');},
    async rename(source,destination){await fs.rename(source,destination);},
    async unlink(location){if(state.cleanupError)throw fault();await fs.unlink(location);},
    async rmdir(location){assert.deepEqual(await fs.readdir(location),[]);await fs.rmdir(location);},
    async open(location,mode){
      let flags=mode&modes.WRITE_ONLY?syncFs.constants.O_WRONLY:syncFs.constants.O_RDONLY;
      if(mode&modes.CREATE)flags|=syncFs.constants.O_CREAT;
      if(mode&modes.TRUNC)flags|=syncFs.constants.O_TRUNC;
      if(mode&modes.NOFOLLOW)flags|=syncFs.constants.O_NOFOLLOW;
      const handle=await fs.open(location,flags);handles.set(handle.fd,handle);return {fd:handle.fd};
    },
    async write(fd,bytes){return (await handles.get(fd).write(Buffer.from(bytes))).bytesWritten;},
    async fsync(fd){await handles.get(fd).sync();},
    async close(file){const fd=typeof file==='number'?file:file.fd;await handles.get(fd).close();handles.delete(fd);},
    async copyFile(sourceFd,destinationFd){
      const sourceHandle=handles.get(sourceFd),destinationHandle=handles.get(destinationFd);
      const bytes=Buffer.alloc(Number((await sourceHandle.stat()).size));
      await sourceHandle.read(bytes,0,bytes.length,0);await destinationHandle.truncate(0);
      await destinationHandle.write(bytes,0,bytes.length,0);
    },
    async copy(srcUri,destUri,options){
      assert.equal(srcUri,URI);assert(destUri.startsWith('sandbox-file:'));
      const contents=destUri.slice('sandbox-file:'.length);
      assert.deepEqual(await fs.readdir(contents),[]);
      copyCalls.push({srcUri,destUri,contents});state.progressListener=options.progressListener;state.copyEntered.resolve();
      if(state.copyGate)await state.copyGate.promise;
      if(state.copyError){await fs.writeFile(path.join(contents,'partial'),'partial');throw fault();}
      const destination=state.layout==='wrapped'?path.join(contents,path.basename(source)):contents;
      // Matches the fixed official CopyDirFunc contract; this is a host provider shim.
      await fs.cp(source,destination,{recursive:true,dereference:false});
    }
  };
  class Encoder{encodeInto(text){return new TextEncoder().encode(text);}}
  const outputs=[];
  const native={
    platformInput(serialized){
      const value=JSON.parse(serialized);attempts.push(value);
      const accepted=value.kind==='folderComplete'?state.completionAccepted:
        value.kind==='folderProgress'?state.progressAccepted:true;
      if(accepted)inputs.push(value);return accepted;
    },
    takePlatformOutput(){return outputs.shift()||'';},
    applyPlatformOutput(){trace.push('apply-platform');return false;},
    releasePlatform(){trace.push('release-platform');}
  };
  const windowHandlers=new Map();
  const mainWindow={on:(kind,callback)=>windowHandlers.set(kind,callback),off:kind=>windowHandlers.delete(kind),
    getWindowProperties:()=>({id:10})};
  const kits={
    '@kit.AbilityKit':{},'@kit.BasicServicesKit':{},
    '@kit.ArkTS':{util:{TextEncoder:Encoder}},
    '@kit.ArkUI':{window:{getLastWindow:async()=>mainWindow}},
    '@kit.CoreFileKit':{fileIo,fileUri:{getUriFromPath:location=>'sandbox-file:'+location},fileShare:{
      OperationMode:{READ_MODE:1,WRITE_MODE:2},
      async persistPermission(policies){assert.deepEqual(policies,[{uri:URI,operationMode:1}]);trace.push('persist-read');},
      async checkPersistentPermission(policies){assert.equal(policies[0].uri,URI);return [state.permission];},
      async activatePermission(policies){assert.equal(policies[0].uri,URI);trace.push('activate-read');}
    },picker:{DocumentSelectMode:{FOLDER:2},DocumentSelectOptions:class{},DocumentViewPicker:class{
      async select(options){assert.equal(options.selectMode,2);assert.equal(options.maxSelectNumber,1);
        state.pickerEntered.resolve();if(state.pickerGate)await state.pickerGate.promise;return state.selected;}
    }}},
    '@kit.PerformanceAnalysisKit':{hilog:{error:(...args)=>trace.push(args),warn:(...args)=>trace.push(args)}},
    'libphotocraft.so':{default:native}
  };
  const overrides={
    './ClipboardBridge':{ClipboardBridge:class{}},
    './PasteAuthorization':{ClipboardReadCancelled:class extends Error{}},
    './PrintingBridge':{PrintingBridge:class{dispose(){} async submitPdf(){return false;}}},
    './ViewportBridge':{ViewportBridge:class{initialize(){}dispose(){} async apply(){}}}
  };
  const previous=global.canIUse;global.canIUse=()=>true;
  const load=loader(kits,overrides);
  const {FolderImportBridge}=load('FolderImportBridge');
  const {FolderImportCoordinator}=load('FolderImportCoordinator');
  const service=new FolderImportBridge({filesDir});
  const coordinator=new FolderImportCoordinator(service,value=>native.platformInput(value),()=>state.available,
    message=>reports.push(message),active=>activities.push(active));
  let platform;
  t.after(async()=>{
    state.copyGate?.resolve();state.pickerGate?.resolve();coordinator.dispose();await coordinator.cancelAndWait();
    if(platform){platform.dispose();await platform.folders.cancelAndWait();}
    global.canIUse=previous;
    await Promise.allSettled([...handles.values()].map(handle=>handle.close()));
    await fs.rm(base,{recursive:true,force:true});
  });
  const request=(id=7,target=TARGET)=>({kind:'folderImport',id,target,intent:'file.scripts.loadFilesIntoStack',
    scope:'topLevel',copyLayout:'sourceBasenameChild',limits:LIMITS});
  const helperRequest=(id=7)=>({...request(id),recursive:true});
  async function importNow(id=7){return service.importFolder(helperRequest(id),()=>{});}
  async function makePlatform(drain){
    const {PlatformBridge}=load('PlatformBridge');
    platform=new PlatformBridge({filesDir,terminateSelf:async()=>{state.terminated=true;}},
      {getComponentUtils:()=>({getRectangleById:()=>({size:{width:800,height:600},screenOffset:{x:30,y:40}})})},
      message=>reports.push(message),()=>trace.push('focus'),async()=>({text:'',imagePath:''}),()=>{},()=>state.available,drain);
    await platform.initialize();return platform;
  }
  return {base,filesDir,source,service,coordinator,request,helperRequest,importNow,inputs,attempts,reports,activities,state,
    trace,copyCalls,outputs,native,makePlatform,handles,load};
}

test('explicit SDK wrapper root yields top-level PNG/PSD and a correctly rebased bounded manifest',async t=>{
  const f=await fixture(t);const done=await f.importNow();
  assert.equal(done.result,'success');assert.equal(done.target,TARGET);assert.equal(done.copyLayout,'sourceBasenameChild');
  assert.equal(done.resolvedRoot,path.join(done.ownerRoot,'contents','用户输入-例'));
  const manifest=JSON.parse(await fs.readFile(done.manifestPath,'utf8'));
  assert.deepEqual(manifest.entries.filter(entry=>entry.kind==='file'&&!entry.relativePath.includes('/')).map(entry=>entry.relativePath),
    ['marker.txt','工程.psd','顶层.png']);
  assert(manifest.entries.some(entry=>entry.relativePath==='sub/deep/nested.png'));
  assert(!manifest.entries.some(entry=>entry.relativePath.startsWith('用户输入-例/')));
  assert.equal(manifest.fileCount,4);assert.equal(f.handles.size,0);
  assert.equal(await fs.readFile(path.join(f.source,'marker.txt'),'utf8'),'unchanged source');
  await f.service.releaseRetained(done,7,TARGET);await assert.rejects(fs.access(done.ownerRoot));
});

test('flat provider layout is rejected and partial sandbox is cleaned rather than guessed',async t=>{
  const f=await fixture(t);f.state.layout='flat';const done=await f.importNow();
  assert.equal(done.result,'error');assert.match(done.error,/copy hierarchy/);assert.equal(done.ownerRoot,'');
  assert.deepEqual(await fs.readdir(path.join(f.filesDir,'PhotoCraft/Documents/folder-imports')),[]);
  assert.equal(await fs.readFile(path.join(f.source,'顶层.png'),'utf8'),'PNG');
});

test('provider copy error and rejected persistent READ yield visible terminal errors without URI text',async t=>{
  const f=await fixture(t);f.state.copyError=true;const failed=await f.importNow();
  assert.equal(failed.result,'error');assert(!failed.error.includes(URI));assert.match(failed.error,/13900020/);
  assert.deepEqual(await fs.readdir(path.join(f.filesDir,'PhotoCraft/Documents/folder-imports')),[]);
  f.state.copyError=false;f.state.permission=false;const permission=await f.importNow(8);
  assert.equal(permission.result,'error');assert.match(permission.error,/retain permission/);assert.equal(f.copyCalls.length,1);
});

test('copied file symlink is rejected without altering the source or its outside target',async t=>{
  const f=await fixture(t);const outside=path.join(f.base,'keep');await fs.writeFile(outside,'outside original');
  await fs.symlink(outside,path.join(f.source,'linked.png'));
  const done=await f.importNow();assert.equal(done.result,'error');assert.match(done.error,/links or special/);
  assert.equal(await fs.readFile(outside,'utf8'),'outside original');
  assert.equal((await fs.lstat(path.join(f.source,'linked.png'))).isSymbolicLink(),true);
});

test('cleanup verifies original id/target marker and every owned ancestor before removing a retained tree',async t=>{
  const f=await fixture(t);const done=await f.importNow();
  await assert.rejects(f.service.releaseRetained(done,8,TARGET),/belongs to another request/);
  await assert.rejects(f.service.releaseRetained(done,7,'new-dialog'),/target does not match/);
  assert.equal(await fs.readFile(path.join(done.resolvedRoot,'顶层.png'),'utf8'),'PNG');
  await f.service.releaseRetained(done,7,TARGET);
});

test('queue rejection retains the owned directory; true completion transfers it exactly once',async t=>{
  const f=await fixture(t);f.state.completionAccepted=false;f.coordinator.begin(f.request());
  await until(()=>f.attempts.some(value=>value.kind==='folderComplete'));
  const retained=f.attempts.find(value=>value.kind==='folderComplete');
  assert.equal(retained.result,'success');await fs.access(retained.ownerRoot);assert.equal(f.coordinator.hasPending(),true);
  f.coordinator.poll();assert.equal(f.inputs.filter(value=>value.kind==='folderComplete').length,0);
  f.state.completionAccepted=true;f.coordinator.poll();assert.equal(f.coordinator.hasPending(),false);
  assert.equal(f.inputs.filter(value=>value.kind==='folderComplete').length,1);
  f.coordinator.cancel(7,TARGET);f.coordinator.dispose();await fs.access(retained.ownerRoot);
});

test('duplicate request id and late cancel cannot import again or delete already transferred results',async t=>{
  const f=await fixture(t);f.coordinator.begin(f.request());
  await until(()=>f.inputs.some(value=>value.kind==='folderComplete'));
  const transferred=f.inputs.find(value=>value.kind==='folderComplete');
  f.coordinator.begin(f.request());f.coordinator.begin(f.request(7,'new-target'));
  assert.equal(f.coordinator.cancel(7,TARGET),false);assert.equal(f.copyCalls.length,1);
  await fs.access(transferred.ownerRoot);assert.equal(f.inputs.filter(value=>value.kind==='folderComplete').length,1);
});

test('cancel after successful copy but before accepted completion cleans only the still-owned job',async t=>{
  const f=await fixture(t);f.state.completionAccepted=false;f.coordinator.begin(f.request());
  await until(()=>f.attempts.some(value=>value.kind==='folderComplete'));
  const retained=f.attempts.find(value=>value.kind==='folderComplete');
  assert.equal(f.coordinator.cancel(7,'other-target'),false);await fs.access(retained.ownerRoot);
  assert.equal(f.coordinator.cancel(7,TARGET),true);
  await until(()=>f.attempts.some(value=>value.kind==='folderComplete'&&value.result==='cancel'));
  await assert.rejects(fs.access(retained.ownerRoot));
  f.state.completionAccepted=true;f.coordinator.poll();
  assert.deepEqual(f.inputs.filter(value=>value.kind==='folderComplete').map(value=>value.result),['cancel']);
});

test('long copy remains independently cancellable; stale id/target cannot cancel a newer job',async t=>{
  const f=await fixture(t);f.state.copyGate=defer();f.coordinator.begin(f.request());await f.state.copyEntered.promise;
  assert.equal(f.coordinator.cancel(6,TARGET),false);assert.equal(f.coordinator.cancel(7,'replaced-target'),false);
  f.coordinator.begin(f.request(8,'second-job'));assert.equal(f.inputs.find(value=>value.id===8).result,'error');
  assert.equal(f.coordinator.cancel(7,TARGET),true);assert.equal(f.state.cancelCount,1);
  await tick();assert.equal(f.inputs.some(value=>value.id===7&&value.kind==='folderComplete'),false);
  assert.equal((await fs.lstat(f.copyCalls[0].contents)).isDirectory(),true);
  f.state.copyGate.resolve();await f.coordinator.cancelAndWait();
  assert.equal(f.inputs.find(value=>value.id===7&&value.kind==='folderComplete').result,'cancel');
  await assert.rejects(fs.access(f.copyCalls[0].contents));
});

test('page disposal cancels a delayed provider and cleans its late result without any completion enqueue',async t=>{
  const f=await fixture(t);f.state.copyGate=defer();f.coordinator.begin(f.request());await f.state.copyEntered.promise;
  f.coordinator.dispose();await tick();assert.equal(f.state.cancelCount,1);
  await fs.access(f.copyCalls[0].contents);
  f.state.copyGate.resolve();await f.coordinator.cancelAndWait();
  assert.equal(f.inputs.length,0);await assert.rejects(fs.access(f.copyCalls[0].contents));
});

test('progress is coalesced, retries backpressure, and cannot be emitted after terminal transfer',async t=>{
  const f=await fixture(t);f.state.copyGate=defer();f.coordinator.begin(f.request());await f.state.copyEntered.promise;
  f.state.progressAccepted=false;f.state.progressListener({processedSize:1,totalSize:10});f.coordinator.poll();
  f.state.progressListener({processedSize:2,totalSize:10});f.state.progressAccepted=true;f.coordinator.poll();
  assert.deepEqual(f.inputs.filter(value=>value.kind==='folderProgress').map(value=>value.processedBytes),[2]);
  f.state.copyGate.resolve();await until(()=>!f.coordinator.hasPending());
  const count=f.inputs.length;f.state.progressListener({processedSize:3,totalSize:10});f.coordinator.poll();assert.equal(f.inputs.length,count);
});

test('concurrent file/paste operation rejects a folder request before any picker',async t=>{
  const f=await fixture(t);f.state.available=false;f.coordinator.begin(f.request());
  assert.equal(f.inputs[0].result,'error');assert.match(f.inputs[0].error,/Another file operation/);assert.equal(f.copyCalls.length,0);
});

test('invalid scope, layout, intent and limits are terminal errors rather than executing new commands',async t=>{
  const f=await fixture(t);
  for(const [id,patch] of [[7,{scope:'recursive'}],[8,{copyLayout:'guess'}],[9,{intent:'file.scripts.batch'}],[10,{limits:{maxFiles:501,maxBytes:1}}]])
    f.coordinator.begin({...f.request(id),...patch});
  assert.equal(f.inputs.length,4);assert(f.inputs.every(value=>value.result==='error'));assert.equal(f.copyCalls.length,0);
});

test('PlatformBridge continues taking folderCancel while the independent copy is unfinished',async t=>{
  const f=await fixture(t);f.state.copyGate=defer();const platform=await f.makePlatform();
  f.outputs.push(JSON.stringify({events:[f.request()],closeState:null}));platform.poll();await f.state.copyEntered.promise;
  assert.equal(platform.hasFolderWork(),true);assert.equal(platform.running,false);
  f.outputs.push(JSON.stringify({events:[{kind:'folderCancel',id:7,target:TARGET}],closeState:null}));platform.poll();
  assert.equal(f.state.cancelCount,1);assert.equal(f.inputs.some(value=>value.kind==='folderComplete'),false);
  f.state.copyGate.resolve();await until(()=>!platform.hasFolderWork());
  assert.equal(f.inputs.find(value=>value.kind==='folderComplete').result,'cancel');
});

test('suspending IME for expected picker background does not cancel folder import',async t=>{
  const f=await fixture(t);f.state.copyGate=defer();const platform=await f.makePlatform();
  f.outputs.push(JSON.stringify({events:[f.request()],closeState:null}));platform.poll();await f.state.copyEntered.promise;
  platform.suspend();assert.equal(f.state.cancelCount,0);
  f.state.copyGate.resolve();await until(()=>!platform.hasFolderWork());
  assert.equal(f.inputs.find(value=>value.kind==='folderComplete').result,'success');
});

test('page hide cancels its folder job while control polling skips hidden component geometry',async t=>{
  const f=await fixture(t);f.state.copyGate=defer();const platform=await f.makePlatform();
  f.outputs.push(JSON.stringify({events:[f.request()],closeState:null}));platform.poll();await f.state.copyEntered.promise;
  platform.setPageVisible(false);assert.equal(f.state.cancelCount,1);
  const applied=f.trace.filter(value=>value==='apply-platform').length;
  f.outputs.push(JSON.stringify({events:[],closeState:null}));platform.poll();
  assert.equal(f.trace.filter(value=>value==='apply-platform').length,applied);
  f.state.copyGate.resolve();await until(()=>!platform.hasFolderWork());
  assert.equal(f.inputs.find(value=>value.kind==='folderComplete').result,'cancel');
});

test('cleanup failure retains material and sends an error with its owned root instead of a false success',async t=>{
  const f=await fixture(t);f.state.completionAccepted=false;f.coordinator.begin(f.request());
  await until(()=>f.attempts.some(value=>value.kind==='folderComplete'));
  const done=f.attempts.find(value=>value.kind==='folderComplete');f.state.cleanupError=true;
  f.coordinator.cancel(7,TARGET);
  await until(()=>f.attempts.some(value=>value.kind==='folderComplete'&&value.result==='error'));
  await fs.access(done.ownerRoot);assert(f.reports.some(message=>message.includes('Files have been retained')));
  f.state.completionAccepted=true;f.coordinator.poll();
  const accepted=f.inputs.find(value=>value.kind==='folderComplete');
  assert.equal(accepted.result,'error');assert.equal(accepted.ownerRoot,done.ownerRoot);
});

test('invalid SDK progress becomes terminal input error and cleans only the private copy after settlement',async t=>{
  const f=await fixture(t);f.state.copyGate=defer();f.coordinator.begin(f.request());await f.state.copyEntered.promise;
  f.state.progressListener({processedSize:NaN,totalSize:10});assert.equal(f.state.cancelCount,1);
  assert.equal(f.inputs.some(value=>value.kind==='folderComplete'),false);
  f.state.copyGate.resolve();await until(()=>!f.coordinator.hasPending());
  assert.equal(f.inputs.find(value=>value.kind==='folderComplete').result,'error');
  await assert.rejects(fs.access(f.copyCalls[0].contents));
  assert.equal(await fs.readFile(path.join(f.source,'工程.psd'),'utf8'),'8BPS');
});

test('authorized close waits for provider settlement before terminating the ability',async t=>{
  const f=await fixture(t);f.state.copyGate=defer();const platform=await f.makePlatform();
  f.outputs.push(JSON.stringify({events:[f.request()],closeState:null}));platform.poll();await f.state.copyEntered.promise;
  f.outputs.push(JSON.stringify({events:[{kind:'close',id:99}],closeState:null}));platform.poll();
  await tick();assert.equal(f.state.cancelCount,1);assert.equal(f.state.terminated,false);
  await fs.access(f.copyCalls[0].contents);f.state.copyGate.resolve();await until(()=>f.state.terminated);
  await assert.rejects(fs.access(f.copyCalls[0].contents));
});

for(const intent of ['file.automate.batch','file.automate.lensCorrection']) test(`original ${intent} input Browse imports the strict SDK root without executing the command`,async t=>{
  const f=await fixture(t);
  const request={...f.request(),intent};
  f.coordinator.begin(request);
  await until(()=>!f.coordinator.hasPending());
  const done=f.inputs.find(value=>value.kind==='folderComplete');
  assert.equal(done.result,'success');assert.equal(done.id,request.id);assert.equal(done.target,request.target);
  assert.equal(done.resolvedRoot,path.join(done.ownerRoot,'contents','用户输入-例'));
  const manifest=JSON.parse(await fs.readFile(done.manifestPath,'utf8'));
  assert(manifest.entries.some(entry=>entry.relativePath==='工程.psd'));
  assert(manifest.entries.some(entry=>entry.relativePath==='sub/deep/nested.png'));
  assert.equal(f.copyCalls.length,1);assert.equal(f.reports.length,0);
  assert(!f.inputs.some(value=>value.kind==='folderPublishComplete'));
});

test('Batch source refresh reactivates its bound folder and reads a fresh exact relative file',async t=>{
  const f=await fixture(t);const original=await f.importNow();
  const {FolderSourceBridge}=f.load('FolderSourceBridge');const reader=new FolderSourceBridge({filesDir:f.filesDir});
  const cached=path.join(original.resolvedRoot,'sub/deep/nested.png');
  const binding=await reader.discover(cached);
  assert.deepEqual(binding,{folderUri:URI,relativePath:'sub/deep/nested.png'});
  await fs.writeFile(path.join(f.source,'sub/deep/nested.png'),'fresh external pixels');
  const fresh=await reader.fresh(binding,123,async location=>fs.readFile(location,'utf8'));
  assert.equal(fresh,'fresh external pixels');
  assert.equal(await fs.readFile(cached,'utf8'),'nestedPNG','old input remains available to original engine history');
  assert.equal(f.copyCalls.length,2);
  assert.equal(f.trace.filter(value=>value==='persist-read').length,1,'fresh read never consumes another temporary picker grant');
  assert.deepEqual(await fs.readdir(path.dirname(original.ownerRoot)),[original.bindingId],'fresh whole-folder copy is cleaned after exact file consumption');
});

test('revoked bound folder permission or deleted relative file never falls back to old Batch input',async t=>{
  const f=await fixture(t);const original=await f.importNow();
  const {FolderSourceBridge}=f.load('FolderSourceBridge');const reader=new FolderSourceBridge({filesDir:f.filesDir});
  const cached=path.join(original.resolvedRoot,'顶层.png');const binding=await reader.discover(cached);
  f.state.permission=false;
  await assert.rejects(reader.fresh(binding,124,async()=>{throw new Error('must not consume');}),/permission|import/i);
  assert.equal(f.copyCalls.length,1);
  f.state.permission=true;await fs.unlink(path.join(f.source,'顶层.png'));
  await assert.rejects(reader.fresh(binding,125,async()=>{throw new Error('must not consume');}),/deleted or renamed/);
  assert.equal(await fs.readFile(cached,'utf8'),'PNG');
  assert.deepEqual(await fs.readdir(path.dirname(original.ownerRoot)),[original.bindingId]);
});

test('folder provenance rejects marker mismatch, traversal, changed manifest and symlink before external copy',async t=>{
  const f=await fixture(t);const original=await f.importNow();
  const {FolderSourceBridge}=f.load('FolderSourceBridge');const reader=new FolderSourceBridge({filesDir:f.filesDir});
  const cached=path.join(original.resolvedRoot,'顶层.png');const sourcePath=path.join(original.ownerRoot,'source.json');
  const marker=JSON.parse(await fs.readFile(sourcePath,'utf8'));
  await fs.writeFile(sourcePath,JSON.stringify({...marker,requestId:8}));
  await assert.rejects(reader.discover(cached),/does not match/);
  await fs.writeFile(sourcePath,JSON.stringify(marker));
  assert.throws(()=>reader.validate({folderUri:URI,relativePath:'../outside.png'}),/unsafe/);
  await fs.writeFile(cached,'changed owned bytes invalidates manifest');
  await assert.rejects(reader.discover(cached),/does not match the source manifest/);
  await fs.unlink(cached);await fs.symlink(path.join(f.source,'顶层.png'),cached);
  await assert.rejects(reader.discover(cached),/links/);
  assert.equal(f.copyCalls.length,1);
});

test('explicit safe close waits for single-file ACK binding finally as well as folder workers',async t=>{
  const f=await fixture(t);const {FileTransactionDrain}=f.load('FileTransactionDrain');
  const drain=new FileTransactionDrain();const token=drain.tryBegin();assert.ok(token);
  const platform=await f.makePlatform(drain);
  // Rust has accepted a Save-before-close. The Index transaction still owns delayed binding/fsync work.
  f.trace.push('rust-save-ack');f.outputs.push(JSON.stringify({events:[{kind:'close',id:333}],closeState:null}));platform.poll();
  for(let n=0;n<200;n++)await tick();
  assert.equal(f.state.terminated,false);assert.equal(drain.tryBegin(),undefined);
  f.trace.push('binding-fsync-finished');drain.end(token);await until(()=>f.state.terminated);
  assert(f.trace.indexOf('binding-fsync-finished')>f.trace.indexOf('rust-save-ack'));
});

test('folder-origin Linked/Recent imports persist provenance across rebuild and refresh exact external bytes',async t=>{
  const f=await fixture(t);const original=await f.importNow();
  const {DocumentBridge}=f.load('DocumentBridge');const bridge=new DocumentBridge({filesDir:f.filesDir});
  const cached=path.join(original.resolvedRoot,'工程.psd');
  await fs.writeFile(path.join(f.source,'工程.psd'),'8BPS fresh layer document');
  const first=await bridge.reopenSource(cached,401);
  assert.equal(await fs.readFile(first.path,'utf8'),'8BPS fresh layer document');
  assert.equal(first.sourceUri,'');assert.equal(first.originalName,'工程.psd');
  await bridge.bindImportedSource(first.path,first);
  const rebuilt=new DocumentBridge({filesDir:f.filesDir});await rebuilt.restoreBindings();
  await fs.writeFile(path.join(f.source,'工程.psd'),'8BPS externally edited again');
  const second=await rebuilt.reopenSource(first.path,402);
  assert.equal(await fs.readFile(second.path,'utf8'),'8BPS externally edited again');
  assert.equal(await fs.readFile(first.path,'utf8'),'8BPS fresh layer document');
  assert.equal(rebuilt.destinationFor(first.path),undefined,'folder authority cannot masquerade as a writable file URI');
  const records=JSON.parse(await fs.readFile(path.join(f.filesDir,'PhotoCraft/Documents/source-uris.json'),'utf8'));
  assert.equal(records[0].folder.relativePath,'工程.psd');assert.equal(records[0].uri,'');
});

test('production Index Save-before-close ACK must finish binding before production PlatformBridge terminate',async t=>{
  const f=await fixture(t);const {FileTransactionDrain}=f.load('FileTransactionDrain');
  const drain=new FileTransactionDrain();const token=drain.tryBegin();assert.ok(token);
  const platform=await f.makePlatform(drain);const bound=defer(),bindingEntered=defer();
  const indexPath=path.resolve(platformRoot,'../pages/Index.ets');
  const indexSource=syncFs.readFileSync(indexPath,'utf8');
  const begin=indexSource.indexOf('  private async runFileRequest('),end=indexSource.indexOf('  private notifyRecentSources(',begin);
  assert(begin>=0&&end>begin);
  const method=indexSource.slice(begin,end);
  const moduleShim=new Module(indexPath,module);
  const driverSource=`const nativePhotoCraft=globalThis.__photocraftFileNative;const hilog={error(){},warn(){}};
    class Driver{constructor(bridge){this.documentBridge=bridge;this.isBusy=true;this.recentSourcesChanged=false;}
      notifyRecentSources(){} focusEditor(){} reportFileError(message){throw new Error(message);} errorMessage(error){return error.message;}
      ${method}
    }module.exports={Driver};`;
  f.native.prepareFileSave=()=>'/private-stage.psd';
  f.native.completeFileRequest=()=>{f.trace.push('rust-save-ack');f.outputs.push(JSON.stringify({events:[{kind:'close',id:403}],closeState:null}));platform.poll();return true;};
  globalThis.__photocraftFileNative=f.native;
  moduleShim._compile(ts.transpileModule(driverSource,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText,indexPath);
  delete globalThis.__photocraftFileNative;
  const driver=new moduleShim.exports.Driver({destinationFor:()=>URI,destinationName:()=> 'saved.psd',
    publishDocument:async()=> 'saved.psd',bindSource:async()=>{bindingEntered.resolve();await bound.promise;f.trace.push('binding-fsync-finished');},takePublicationWarning:()=>''});
  const work=driver.runFileRequest({id:403,kind:'save',intent:'file.save',chooseDestination:false,previousPath:'/oldstage',suggestedName:'saved.psd'},drain,token);
  await bindingEntered.promise;
  for(let n=0;n<20;n++)await tick();
  assert.equal(f.state.terminated,false);assert.equal(driver.isBusy,true);assert.equal(drain.tryBegin(),undefined);
  bound.resolve();await work;await until(()=>f.state.terminated);
  assert.equal(driver.isBusy,false);assert.deepEqual(f.trace.filter(item=>typeof item==='string'&&['rust-save-ack','binding-fsync-finished'].includes(item)),['rust-save-ack','binding-fsync-finished']);
});

test('disappeared Index settles its old import and cannot ACK a replacement native app',async t=>{
  const f=await fixture(t);const {FileTransactionDrain}=f.load('FileTransactionDrain');
  const drain=new FileTransactionDrain(),token=drain.tryBegin();const picker=defer(),entered=defer();let removed=0,acks=0;
  const indexPath=path.resolve(platformRoot,'../pages/Index.ets');const source=syncFs.readFileSync(indexPath,'utf8');
  const begin=source.indexOf('  private async runFileRequest('),end=source.indexOf('  private notifyRecentSources(',begin);assert(begin>=0&&end>begin);
  const moduleShim=new Module(indexPath,module);globalThis.__photocraftOldNative={completeFileRequest(){acks++;return true;}};
  moduleShim._compile(ts.transpileModule(`const nativePhotoCraft=globalThis.__photocraftOldNative;const hilog={error(){},warn(){}};
    class Driver{constructor(bridge){this.documentBridge=bridge;this.isBusy=true;this.disposed=false;}
      notifyRecentSources(){} focusEditor(){} reportFileError(message){throw new Error(message);} errorMessage(error){return error.message;}
      ${source.slice(begin,end)}}module.exports={Driver};`,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText,indexPath);
  delete globalThis.__photocraftOldNative;
  const driver=new moduleShim.exports.Driver({openRequestedDocument:async()=>{entered.resolve();return await picker.promise;},
    removeTemporaryFile:async path=>{assert.equal(path,'old-owned-import.png');removed++;}});
  const work=driver.runFileRequest({id:404,kind:'open'},drain,token);await entered.promise;
  driver.disposed=true;drain.dispose();picker.resolve({path:'old-owned-import.png',originalName:'old.png'});await work;
  assert.equal(acks,0);assert.equal(removed,1);assert.equal(driver.isBusy,false);assert.equal(drain.end(token),false);
});

test('production Index cancelled paste keeps shutdown behind provider read and late PNG cleanup',async t=>{
  const f=await fixture(t);const {FileTransactionDrain}=f.load('FileTransactionDrain');
  const drain=new FileTransactionDrain(),platform=await f.makePlatform(drain);
  const paste=f.load('PasteAuthorization');
  const read=defer(),cleanup=defer(),cleanupEntered=defer();
  const indexPath=path.resolve(platformRoot,'../pages/Index.ets');const source=syncFs.readFileSync(indexPath,'utf8');
  const begin=source.indexOf('  private async authorizePaste('),end=source.indexOf('  private pollPlatform(',begin);
  assert(begin>=0&&end>begin);
  globalThis.__photocraftPaste={...paste,drain};
  const moduleShim=new Module(indexPath,module);
  moduleShim._compile(ts.transpileModule(`const {PasteAccessRequest,ClipboardReadCancelled,drain}=globalThis.__photocraftPaste;
    const PasteAccessDialog=value=>value;
    class CustomDialogController{constructor(options){this.options=options;}open(){this.options.builder.approve();}close(){}}
    class Driver{constructor(){this.fileTransactions=drain;this.pageVisible=true;this.foreground=true;}
      reportFileError(message){throw new Error(message);}errorMessage(error){return error.message;}
      ${source.slice(begin,end)}}module.exports={Driver};`,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText,indexPath);
  delete globalThis.__photocraftPaste;
  const driver=new moduleShim.exports.Driver();
  const work=driver.authorizePaste(()=>read.promise,async content=>{
    assert.equal(content.imagePath,'owned-late.png');cleanupEntered.resolve();await cleanup.promise;f.trace.push('late-png-removed');
  });
  const rejected=assert.rejects(work,paste.ClipboardReadCancelled);
  // Cancellation rejects this completion while tryDirect still awaits the provider gate.
  driver.pasteAccess.completion.catch(()=>{});
  driver.cancelPasteAccess();
  f.outputs.push(JSON.stringify({events:[{kind:'close',id:405}],closeState:null}));platform.poll();
  for(let n=0;n<20;n++)await tick();
  assert.equal(f.state.terminated,false);assert.ok(driver.pasteAccess);assert.equal(drain.tryBegin(),undefined);
  read.resolve({text:'',imagePath:'owned-late.png'});await cleanupEntered.promise;
  for(let n=0;n<20;n++)await tick();assert.equal(f.state.terminated,false);
  cleanup.resolve();await rejected;await until(()=>f.state.terminated);
  assert.equal(driver.pasteAccess,undefined);assert(f.trace.includes('late-png-removed'));
});

test('production PlatformBridge rejected PNG enqueue holds close until its owned PNG cleanup settles',async t=>{
  const f=await fixture(t);const {FileTransactionDrain}=f.load('FileTransactionDrain');
  const drain=new FileTransactionDrain(),platform=await f.makePlatform(drain),cleanup=defer(),entered=defer();
  platform.authorizePaste=async()=>({text:'',imagePath:'rejected-owned-paste.png'});
  platform.cleanupImage=async local=>{assert.equal(local,'rejected-owned-paste.png');entered.resolve();await cleanup.promise;};
  const original=f.native.platformInput;
  f.native.platformInput=serialized=>JSON.parse(serialized).kind==='clipboard'&&JSON.parse(serialized).imagePath.length>0?false:original(serialized);
  f.outputs.push(JSON.stringify({events:[{kind:'paste',id:406},{kind:'close',id:407}],closeState:null}));platform.poll();
  await entered.promise;
  for(let n=0;n<20;n++)await tick();assert.equal(f.state.terminated,false);assert.equal(drain.tryBegin(),undefined);
  cleanup.resolve();await until(()=>f.state.terminated);
  assert.equal(f.inputs.some(value=>value.kind==='clipboard'&&value.id===406&&value.result==='error'),true);
});
