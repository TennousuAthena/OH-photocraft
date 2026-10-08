'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const syncFs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const loadPureEts = require('./load_pure_ets.cjs');
const source = process.env.FOLDER_HELPER_PATH || path.resolve(__dirname, '../harmonyos/entry/src/main/ets/platform/SandboxFolderManifest.ets');
const {inspectSandboxFolder, removeOwnedSandboxFolder, validateFolderRelativePath, validateFolderLimits, FolderImportCancelled, resolveSourceBasenameChild} = loadPureEts(source);
const LIMITS = {maxFiles:500, maxBytes:1024*1024};
class Storage {
  constructor() {this.inspected=[];}
  async list(directory) {return fs.readdir(directory);}
  async inspect(file) {
    this.inspected.push(file);
    const stat = await fs.lstat(file);
    return {kind:stat.isSymbolicLink()?'symlink':stat.isDirectory()?'directory':stat.isFile()?'file':'other', bytes:stat.size};
  }
  async unlink(file) {await fs.unlink(file);}
  async removeEmptyDirectory(directory) {assert.equal((await fs.readdir(directory)).length,0); await fs.rmdir(directory);}
}
async function fixture(action) {
  const base = await fs.mkdtemp(path.join(os.tmpdir(), 'photocraft-folder-'));
  const root = path.join(base,'owned');
  await fs.mkdir(root);
  try {await action({base,root,storage:new Storage()});} finally {await fs.rm(base,{recursive:true,force:true});}
}

test('real nested Unicode files and empty folders produce a deterministic sandbox manifest',()=>fixture(async ({root,storage})=>{
  await fs.mkdir(path.join(root,'空目录'));
  await fs.mkdir(path.join(root,'工程'));
  await fs.writeFile(path.join(root,'工程','中文.pcraft'),'project');
  await fs.writeFile(path.join(root,'零字节.png'),'');
  const result=await inspectSandboxFolder(storage,root,LIMITS,()=>false);
  assert.equal(result.fileCount,2);
  assert.equal(result.totalBytes,7);
  assert.deepEqual(result.entries.map(entry=>entry.relativePath), ['工程','工程/中文.pcraft','空目录','零字节.png']);
  assert.ok(storage.inspected.every(file=>file===root||file.startsWith(root+'/')));
}));

test('500 real files are accepted; a 501st file rejects the whole manifest',()=>fixture(async ({root,storage})=>{
  await Promise.all(Array.from({length:500},(_,i)=>fs.writeFile(path.join(root,`${i}.png`),'a')));
  assert.equal((await inspectSandboxFolder(storage,root,LIMITS,()=>false)).fileCount,500);
  await fs.writeFile(path.join(root,'overflow.png'),'b');
  await assert.rejects(inspectSandboxFolder(storage,root,LIMITS,()=>false),/file count or size limit/);
}));

test('aggregate real file size is checked, including an exact byte boundary',()=>fixture(async ({root,storage})=>{
  await fs.writeFile(path.join(root,'a'),'123');
  await fs.writeFile(path.join(root,'b'),'4567');
  assert.equal((await inspectSandboxFolder(storage,root,{maxFiles:2,maxBytes:7},()=>false)).totalBytes,7);
  await assert.rejects(inspectSandboxFolder(storage,root,{maxFiles:2,maxBytes:6},()=>false),/file count or size limit/);
}));

for (const directory of [false,true]) {
  test(`copied ${directory?'directory':'file'} symlink is rejected and cleanup never follows its outside target`,()=>fixture(async ({base,root,storage})=>{
    const outside=path.join(base,'outside');
    if(directory){await fs.mkdir(outside);await fs.writeFile(path.join(outside,'keep.txt'),'unchanged');}
    else await fs.writeFile(outside,'unchanged');
    await fs.symlink(outside,path.join(root,'link'));
    await assert.rejects(inspectSandboxFolder(storage,root,LIMITS,()=>false),/links or special files/);
    await removeOwnedSandboxFolder(storage,root);
    assert.equal(await fs.readFile(directory?path.join(outside,'keep.txt'):outside,'utf8'),'unchanged');
    assert.ok(!storage.inspected.includes(outside));
    await assert.rejects(fs.access(root));
  }));
}

test('a symlink root is rejected without reading its target',()=>fixture(async ({base,root,storage})=>{
  const alias=path.join(base,'alias');
  await fs.symlink(root,alias);
  await assert.rejects(inspectSandboxFolder(storage,alias,LIMITS,()=>false),/Links will not be followed/);
  assert.deepEqual(storage.inspected,[alias]);
  await removeOwnedSandboxFolder(storage,alias);
  assert.deepEqual(await fs.readdir(root),[]);
}));

for(const child of ['../outside','/absolute','a/b','a\\b','.','..','\u0000bad']) {
  test(`untrusted listing rejects ${JSON.stringify(child)} before resolving or inspecting it`,()=>fixture(async ({root,storage})=>{
    storage.list=async()=>[child];
    await assert.rejects(inspectSandboxFolder(storage,root,LIMITS,()=>false),/unsafe|outside/);
    assert.deepEqual(storage.inspected,[root]);
  }));
}

test('duplicate provider names reject instead of overwriting manifest identity',()=>fixture(async ({root,storage})=>{
  await fs.writeFile(path.join(root,'a'),'safe');
  storage.list=async()=>['a','a'];
  await assert.rejects(inspectSandboxFolder(storage,root,LIMITS,()=>false),/duplicate paths/);
}));

test('pre-cancel does no metadata IO; cancel after directory listing stops before reading a child',()=>fixture(async ({root,storage})=>{
  await assert.rejects(inspectSandboxFolder(storage,root,LIMITS,()=>true),FolderImportCancelled);
  assert.deepEqual(storage.inspected,[]);
  await fs.writeFile(path.join(root,'a'),'safe');
  let cancelled=false;
  storage.list=async directory=>{const names=await fs.readdir(directory);cancelled=true;return names;};
  await assert.rejects(inspectSandboxFolder(storage,root,LIMITS,()=>cancelled),FolderImportCancelled);
  assert.deepEqual(storage.inspected,[root]);
}));

test('deep real folder hierarchy is bounded and can still be cleaned safely',()=>fixture(async ({root,storage})=>{
  let current=root;
  for(let i=0;i<33;i++){current=path.join(current,'d');await fs.mkdir(current);}
  await assert.rejects(inspectSandboxFolder(storage,root,LIMITS,()=>false),/unsafe/);
  await removeOwnedSandboxFolder(storage,root);
  await assert.rejects(fs.access(root));
}));

test('metadata IO errors propagate and the owned partial copy can be removed',()=>fixture(async ({root,storage})=>{
  await fs.writeFile(path.join(root,'partial'),'partial bytes');
  const list=storage.list.bind(storage);
  storage.list=async()=>{throw new Error('injected IO failure');};
  await assert.rejects(inspectSandboxFolder(storage,root,LIMITS,()=>false),/injected IO failure/);
  storage.list=list;
  await removeOwnedSandboxFolder(storage,root);
  await assert.rejects(fs.access(root));
}));

test('cleanup refuses an unsafe listing and preserves unrelated files',()=>fixture(async ({base,root,storage})=>{
  const outside=path.join(base,'keep');await fs.writeFile(outside,'outside');
  storage.list=async()=>['../keep'];
  await assert.rejects(removeOwnedSandboxFolder(storage,root),/invalid directory entry/);
  assert.equal(await fs.readFile(outside,'utf8'),'outside');
}));

test('invalid counts/bytes and absolute or dot-segment relative paths are never accepted',()=>{
  for(const limits of [{maxFiles:501,maxBytes:1},{maxFiles:0,maxBytes:1},{maxFiles:1,maxBytes:0},{maxFiles:1,maxBytes:Infinity},{maxFiles:1,maxBytes:1024*1024*1024+1}])
    assert.throws(()=>validateFolderLimits(limits));
  for(const relative of ['/a','a/../b','a/./b','a//b','']) assert.throws(()=>validateFolderRelativePath(relative));
});

test('sourceBasenameChild is an explicit container contract and manifest paths are relative to its actual root',()=>fixture(async ({root,storage})=>{
  const actual=path.join(root,'源目录');await fs.mkdir(actual);await fs.writeFile(path.join(actual,'顶层.png'),'image');
  assert.equal(await resolveSourceBasenameChild(storage,root,()=>false),actual);
  const manifest=await inspectSandboxFolder(storage,actual,LIMITS,()=>false);
  assert.deepEqual(manifest.entries.map(entry=>entry.relativePath),['顶层.png']);
}));

for(const shape of ['empty','flatFile','twoDirectories']) {
  test(`explicit source-basename contract rejects ${shape} instead of guessing a different provider layout`,()=>fixture(async ({root,storage})=>{
    if(shape==='flatFile') await fs.writeFile(path.join(root,'flat.png'),'flat');
    if(shape==='twoDirectories'){await fs.mkdir(path.join(root,'one'));await fs.mkdir(path.join(root,'two'));}
    await assert.rejects(resolveSourceBasenameChild(storage,root,()=>false),/copy hierarchy|unique regular/);
  }));
}

test('a symbolic link occupying the source child never becomes actualRoot',()=>fixture(async ({base,root,storage})=>{
  const outside=path.join(base,'outside');await fs.mkdir(outside);await fs.writeFile(path.join(outside,'keep'),'outside');
  await fs.symlink(outside,path.join(root,'source'));
  await assert.rejects(resolveSourceBasenameChild(storage,root,()=>false),/Links will not be followed/);
  assert.equal(await fs.readFile(path.join(outside,'keep'),'utf8'),'outside');assert(!storage.inspected.includes(outside));
}));

test('untrusted container listing cannot smuggle an outside actualRoot',()=>fixture(async ({root,storage})=>{
  storage.list=async()=>['../outside'];
  await assert.rejects(resolveSourceBasenameChild(storage,root,()=>false),/copy hierarchy/);
  assert.deepEqual(storage.inspected,[root]);
}));
