'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const loadPureEts = require('./load_pure_ets.cjs');
const source = path.resolve(__dirname, '../harmonyos/entry/src/main/ets/platform/FileTransactionDrain.ets');
const { FileTransactionDrain, FileTransactionToken } = loadPureEts(source);
const turn = () => new Promise(resolve => setImmediate(resolve));

test('external ACK cannot release close before both provider/binding finally blocks end', async () => {
  const drain = new FileTransactionDrain();
  const save = drain.tryBegin();
  const dropBinding = drain.tryBegin();
  let closed = false;
  const waiter = drain.waitForIdle();
  waiter.then(() => { closed = true; });
  assert.equal(drain.tryBegin(), undefined, 'gate closes before a new mailbox read');
  assert.equal(drain.waitForIdle(), waiter, 'repeated requests share one waiter');
  for (let n = 0; n < 200; n++) await turn();
  assert.equal(closed, false, 'there is no timeout or synthetic ACK');
  assert.equal(drain.end(save), true);
  await turn();
  assert.equal(closed, false, 'drop import/bind still owns provider work');
  assert.equal(drain.end(dropBinding), true);
  await waiter;
  assert.equal(closed, true);
  assert.equal(drain.tryBegin(), undefined, 'terminal close keeps admission closed');
});

test('duplicate, forged, and replaced-owner tokens cannot release another transaction', async () => {
  const previous = new FileTransactionDrain();
  const stale = previous.tryBegin();
  const next = new FileTransactionDrain();
  const owner = next.tryBegin();
  assert.equal(stale.sequence, owner.sequence);
  assert.equal(next.end(stale), false);
  assert.equal(next.end(new FileTransactionToken(owner.sequence)), false);
  let resolved = false;
  const waiter = next.waitForIdle();
  waiter.then(() => { resolved = true; });
  await turn();
  assert.equal(resolved, false);
  assert.equal(next.end(owner), true);
  assert.equal(next.end(owner), false);
  await waiter;
  previous.dispose();
  assert.equal(previous.end(stale), true, 'late original finally remains harmless');
});

test('failed close cancels its waiter and reopens admission with independent ownership', async () => {
  const drain = new FileTransactionDrain();
  const active = drain.tryBegin();
  const failed = drain.waitForIdle();
  const rejection = assert.rejects(failed, /closure was canceled/);
  drain.resumeAfterFailedClose();
  await rejection;
  const next = drain.tryBegin();
  assert.ok(next);
  const waiting = drain.waitForIdle();
  let resolved = false;
  waiting.then(() => { resolved = true; });
  drain.end(active);
  await turn();
  assert.equal(resolved, false);
  drain.end(next);
  await waiting;
  drain.resumeAfterFailedClose();
  assert.ok(drain.tryBegin());
});

test('disposal rejects close and retains active ownership until finally without admitting new work', async () => {
  const drain = new FileTransactionDrain();
  const active = drain.tryBegin();
  const waiting = drain.waitForIdle();
  const rejection = assert.rejects(waiting, /has been destroyed/);
  drain.dispose();
  drain.dispose();
  await rejection;
  assert.equal(drain.tryBegin(), undefined);
  drain.resumeAfterFailedClose();
  assert.equal(drain.tryBegin(), undefined);
  assert.equal(drain.end(active), true);
  assert.equal(drain.end(active), false);
  await assert.rejects(drain.waitForIdle(), /has been destroyed/);
});

test('admission is bounded and empty close can reopen only after explicit failure', async () => {
  const drain = new FileTransactionDrain();
  const tokens = Array.from({ length: 64 }, () => drain.tryBegin());
  assert.ok(tokens.every(Boolean));
  assert.equal(drain.tryBegin(), undefined);
  drain.end(tokens[0]);
  const replacement = drain.tryBegin();
  assert.ok(replacement);
  const waiting = drain.waitForIdle();
  tokens.slice(1).forEach(token => drain.end(token));
  drain.end(replacement);
  await waiting;
  drain.resumeAfterFailedClose();
  await drain.waitForIdle();
  assert.equal(drain.tryBegin(), undefined);
  drain.resumeAfterFailedClose();
  assert.ok(drain.tryBegin());
});
