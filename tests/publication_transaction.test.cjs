'use strict';

// Exercise the production state machine with real temporary files and destructive injected failures.
// The helper is a pure TypeScript subset of ArkTS; use the installed SDK compiler to load it in Node.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const syncFs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const loadPureEts = require('./load_pure_ets.cjs');
const helperPath = path.resolve(__dirname,
  '../harmonyos/entry/src/main/ets/platform/PublicationTransaction.ets');
const { PublicationTransaction, PublicationFailure, publicationRecoveryNotice } = loadPureEts(helperPath);

const ORIGINAL = Buffer.from('original document bytes, including old tail');
const ENCODED = Buffer.from('fresh encoded document');
const URI = 'content://test-provider/authorized-target';

class FileStorage {
  constructor(directory, faults = []) {
    this.directory = directory;
    this.target = path.join(directory, 'provider-target');
    this.stage = path.join(directory, 'stage.pcraft');
    this.backup = path.join(directory, 'old.pcraft');
    this.retained = path.join(directory, 'new.pcraft');
    this.journal = path.join(directory, 'journal.json');
    this.faults = new Set(faults);
    this.handles = new Map();
    this.roles = new Map();
    this.trace = [];
    this.phase = '';
  }
  location(value) { return value === URI ? this.target : value; }
  role(value) { return value === URI ? 'target' : value === this.stage ? 'stage' : 'backup'; }
  hit(name) {
    this.trace.push(name);
    if (this.faults.delete(name)) throw new Error(`injected ${name}`);
  }
  async openRead(value) {
    this.hit(`read:${this.role(value)}`);
    const handle = await fs.open(this.location(value), 'r');
    this.handles.set(handle.fd, handle);
    this.roles.set(handle.fd, this.role(value));
    return handle.fd;
  }
  async openWrite(value, create) {
    this.hit(`write:${this.role(value)}`);
    // r+ here is Node's existing-file no-TRUNC mode; production uses WRITE_ONLY without TRUNC.
    const handle = await fs.open(this.location(value), create ? 'wx' : 'r+');
    this.handles.set(handle.fd, handle);
    this.roles.set(handle.fd, this.role(value));
    return handle.fd;
  }
  async close(fd) {
    await this.handles.get(fd).close();
    this.handles.delete(fd);
    this.roles.delete(fd);
  }
  async prepareWrite(fd) { this.hit('prepare:target'); }
  async truncate(fd) {
    const operation = `truncate:${this.phase === 'rollingBack' ? 'rollback' : 'publish'}`;
    await this.handles.get(fd).truncate(0);
    this.hit(operation); // Fail after truncating, proving that a rejected API may already mutate bytes.
  }
  async copy(source, destination) {
    const sourceFile = this.handles.get(source);
    const destinationFile = this.handles.get(destination);
    const operation = this.roles.get(destination) === 'backup' ? 'copy:backup' :
      `copy:${this.phase === 'rollingBack' ? 'rollback' : 'publish'}`;
    await destinationFile.truncate(0); // Mirrors the SDK's FD copy overwrite semantics.
    const bytes = Buffer.alloc(Number((await sourceFile.stat()).size));
    await sourceFile.read(bytes, 0, bytes.length, 0);
    if (this.faults.has(operation)) {
      await destinationFile.write(bytes.subarray(0, 3), 0, Math.min(bytes.length, 3), 0);
      this.hit(operation); // Fail after partial destination writes.
    }
    this.hit(operation);
    await destinationFile.write(bytes, 0, bytes.length, 0);
  }
  async sync(fd) {
    const operation = this.roles.get(fd) === 'backup' ? 'sync:backup' :
      `sync:${this.phase === 'rollingBack' ? 'rollback' : 'publish'}`;
    this.hit(operation);
    await this.handles.get(fd).sync();
  }
  async move(source, destination) {
    this.hit('move:recovery');
    await fs.rename(source, destination);
  }
  async remove(location) {
    this.hit('remove:backup');
    await fs.rm(location, { force: true });
  }
  async writeJournal(journal) {
    this.phase = journal.phase;
    this.hit(`journal:${journal.phase}`);
    const temporary = `${this.journal}.tmp`;
    const handle = await fs.open(temporary, 'w');
    try {
      await handle.writeFile(JSON.stringify(journal));
      await handle.sync();
    } finally { await handle.close(); }
    await fs.rename(temporary, this.journal);
  }
  async clearJournal() {
    this.hit('clear:journal');
    await fs.rm(this.journal, { force: true });
    await fs.rm(`${this.journal}.tmp`, { force: true });
  }
}

async function fixture(t, faults, original = ORIGINAL) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'photocraft-publication-test-'));
  const storage = new FileStorage(directory, faults);
  await fs.writeFile(storage.target, original);
  await fs.writeFile(storage.stage, ENCODED);
  t.after(async () => {
    await Promise.allSettled([...storage.handles.values()].map(handle => handle.close()));
    await fs.rm(directory, { force: true, recursive: true });
  });
  return storage;
}

function publish(storage) {
  return new PublicationTransaction(storage).publish(storage.stage, URI, storage.backup, storage.retained);
}

for (const fault of ['read:target', 'write:backup', 'copy:backup', 'sync:backup',
  'journal:prepared', 'write:target', 'prepare:target', 'journal:writing']) {
  test(`${fault}: failure before mutation preserves the original`, async t => {
    const storage = await fixture(t, [fault]);
    await assert.rejects(publish(storage), error => error instanceof PublicationFailure &&
      error.message.includes('original file is unchanged'));
    assert.deepEqual(await fs.readFile(storage.target), ORIGINAL);
    assert.deepEqual(await fs.readFile(storage.stage), ENCODED);
    assert(!storage.trace.includes('truncate:publish'));
    if (['read:target', 'write:backup', 'copy:backup', 'sync:backup', 'journal:prepared'].includes(fault)) {
      assert(!storage.trace.includes('write:target'));
    }
    assert.equal(storage.handles.size, 0);
  });
}

for (const fault of ['truncate:publish', 'copy:publish', 'sync:publish', 'journal:completed']) {
  test(`${fault}: destructive failure restores exact old bytes and still rejects saving`, async t => {
    const storage = await fixture(t, [fault]);
    await assert.rejects(publish(storage), error => error instanceof PublicationFailure &&
      error.message.includes('original file was restored') && error.message.includes(fault));
    assert.deepEqual(await fs.readFile(storage.target), ORIGINAL);
    assert.deepEqual(await fs.readFile(storage.stage), ENCODED);
    assert.equal(storage.trace.filter(value => value === 'write:target').length, 1,
      'rollback reuses the live target fd');
    await assert.rejects(fs.access(storage.backup));
    await assert.rejects(fs.access(storage.journal));
    assert.equal(storage.handles.size, 0);
  });
}

for (const fault of ['truncate:rollback', 'copy:rollback', 'sync:rollback', 'read:backup']) {
  test(`${fault}: failed rollback retains both complete recovery files`, async t => {
    const storage = await fixture(t, ['copy:publish', fault]);
    await assert.rejects(publish(storage), error => error instanceof PublicationFailure &&
      error.message.includes('original file could not be restored') && error.message.includes('copy:publish') && error.message.includes(fault));
    assert.deepEqual(await fs.readFile(storage.backup), ORIGINAL);
    assert.deepEqual(await fs.readFile(storage.retained), ENCODED);
    await assert.rejects(fs.access(storage.stage));
    const journal = JSON.parse(await fs.readFile(storage.journal, 'utf8'));
    assert.equal(journal.phase, 'recoveryRequired');
    assert.equal(journal.destinationUri, URI);
    assert.equal(journal.retainedStagePath, storage.retained);
    assert(!storage.trace.includes('clear:journal'));
    assert.equal(storage.handles.size, 0);
  });
}

test('failed recovery rename requests exact original-stage retention from Rust', async t => {
  const storage = await fixture(t, ['copy:publish', 'copy:rollback', 'move:recovery']);
  await assert.rejects(publish(storage), error => error instanceof PublicationFailure &&
    error.retainStagePath === storage.stage);
  assert.deepEqual(await fs.readFile(storage.backup), ORIGINAL);
  assert.deepEqual(await fs.readFile(storage.stage), ENCODED);
  assert.equal(JSON.parse(await fs.readFile(storage.journal, 'utf8')).retainedStagePath, storage.stage);
});

test('empty new picker target is actually read and backed up before publishing', async t => {
  const storage = await fixture(t, [], Buffer.alloc(0));
  assert.deepEqual(await publish(storage), { warning: '' });
  assert.deepEqual(await fs.readFile(storage.target), ENCODED);
  assert(storage.trace.indexOf('sync:backup') < storage.trace.indexOf('write:target'));
  assert(storage.trace.indexOf('journal:prepared') < storage.trace.indexOf('truncate:publish'));
});

test('successful publication syncs new bytes and removes backup/journal, retaining document stage', async t => {
  const storage = await fixture(t, []);
  assert.deepEqual(await publish(storage), { warning: '' });
  assert.deepEqual(await fs.readFile(storage.target), ENCODED);
  assert.deepEqual(await fs.readFile(storage.stage), ENCODED);
  assert(storage.trace.indexOf('sync:publish') < storage.trace.indexOf('remove:backup'));
  await assert.rejects(fs.access(storage.backup));
  await assert.rejects(fs.access(storage.journal));
  assert.equal(storage.handles.size, 0);
});

test('cleanup failure reports a warning after committed bytes, without rollback', async t => {
  const storage = await fixture(t, ['remove:backup']);
  const outcome = await publish(storage);
  assert(outcome.warning.includes('file was written'));
  assert.deepEqual(await fs.readFile(storage.target), ENCODED);
  assert.deepEqual(await fs.readFile(storage.backup), ORIGINAL);
  assert.equal(JSON.parse(await fs.readFile(storage.journal, 'utf8')).phase, 'completed');
  assert(!storage.trace.includes('copy:rollback'));
});

test('a failed final recovery journal update still leaves both paths in the earlier durable record', async t => {
  const storage = await fixture(t, ['copy:publish', 'copy:rollback', 'journal:recoveryRequired']);
  await assert.rejects(publish(storage), PublicationFailure);
  const journal = JSON.parse(await fs.readFile(storage.journal, 'utf8'));
  assert.equal(journal.phase, 'rollingBack');
  assert.equal(journal.retainedStagePath, storage.retained);
  assert.deepEqual(await fs.readFile(journal.backupPath), ORIGINAL);
  assert.deepEqual(await fs.readFile(journal.retainedStagePath), ENCODED);
});

test('restart notice preserves target and both recovery files without automatically restoring', async t => {
  const storage = await fixture(t, ['copy:publish', 'copy:rollback']);
  await assert.rejects(publish(storage), PublicationFailure);
  const before = await Promise.all([storage.target, storage.backup, storage.retained].map(file => fs.readFile(file)));
  const journal = JSON.parse(await fs.readFile(storage.journal, 'utf8'));
  const trace = storage.trace.slice();
  assert(publicationRecoveryNotice([journal.phase], 0).includes('will not be overwritten automatically'));
  assert.deepEqual(storage.trace, trace);
  assert.deepEqual(await Promise.all([storage.target, storage.backup, storage.retained].map(file => fs.readFile(file))), before);
  assert(publicationRecoveryNotice(['completed'], 0).includes('still needs cleanup'));
  assert(publicationRecoveryNotice([], 1).includes('recovery record'));
  assert.equal(publicationRecoveryNotice([], 0), '');
});
