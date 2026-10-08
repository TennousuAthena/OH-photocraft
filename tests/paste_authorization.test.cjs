"use strict";
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const loadPureEts = require('./load_pure_ets.cjs');
const source = path.resolve(__dirname, '../harmonyos/entry/src/main/ets/platform/PasteAuthorization.ets');
const { PasteAccessRequest, ClipboardReadCancelled } = loadPureEts(source);
const CONTENT = {text:'explicit paste', imagePath:''};

test('no clipboard read occurs until approval; Cancel settles the request', async () => {
  let reads = 0;
  const request = new PasteAccessRequest(async () => { reads++; return CONTENT; }, async () => {});
  const result = assert.rejects(request.completion, ClipboardReadCancelled);
  request.cancel();
  await result;
  await request.approve();
  assert.equal(reads, 0);
});

test('approval reads once even when the user clicks twice', async () => {
  let reads = 0;
  const request = new PasteAccessRequest(async () => { reads++; return CONTENT; }, async () => {});
  await Promise.all([request.approve(), request.approve()]);
  assert.equal(await request.completion, CONTENT);
  assert.equal(reads, 1);
});

test('explicit paste uses the existing system grant without a second approval', async () => {
  let reads = 0;
  const request = new PasteAccessRequest(async () => { reads++; return CONTENT; }, async () => {});
  assert.equal(await request.tryDirect(), true);
  assert.equal(await request.completion, CONTENT);
  await request.approve();
  assert.equal(reads, 1);
});

test('only permission denial keeps the same request pending for the real security button', async () => {
  let reads = 0;
  const request = new PasteAccessRequest(async () => {
    if (++reads === 1) throw Object.assign(new Error('Permission verification failed'), {code: 201});
    return CONTENT;
  }, async () => {});
  let delivered = false, settled = false;
  request.completion.then(() => { delivered = true; });
  request.settled.then(() => { settled = true; });
  assert.equal(await request.tryDirect(), false);
  assert.equal(delivered, false);
  assert.equal(settled, false);
  assert.equal(reads, 1, 'permission denial must not trigger repeated clipboard reads');
  await request.approve();
  assert.equal(await request.completion, CONTENT);
  await request.settled;
  assert.equal(reads, 2);
});

test('direct read preserves non-permission errors without requesting extra authorization', async () => {
  const failure = Object.assign(new Error('Another copy or paste is in progress'), {code: 27787277});
  let reads = 0;
  const request = new PasteAccessRequest(async () => { reads++; throw failure; }, async () => {});
  const result = assert.rejects(request.completion, error => error === failure);
  assert.equal(await request.tryDirect(), true);
  await result;
  await request.approve();
  assert.equal(reads, 1);
});

test('cancel after permission denial does not reread clipboard or deliver to a new editor', async () => {
  let reads = 0;
  const request = new PasteAccessRequest(async () => {
    reads++;
    throw Object.assign(new Error('Permission verification failed'), {code: 201});
  }, async () => {});
  assert.equal(await request.tryDirect(), false);
  const result = assert.rejects(request.completion, ClipboardReadCancelled);
  request.cancel();
  await result;
  await request.approve();
  await request.settled;
  assert.equal(reads, 1);
});

test('page hide during image read cancels completion and discards the late PNG', async () => {
  let finish;
  const late = new Promise(resolve => { finish = resolve; });
  const discarded = [];
  const request = new PasteAccessRequest(() => late, async content => { discarded.push(content.imagePath); });
  const result = assert.rejects(request.completion, ClipboardReadCancelled);
  const reading = request.approve();
  request.cancel();
  finish({text:'', imagePath:'/sandbox/cache/Clipboard/late.png'});
  await Promise.all([result, reading]);
  assert.deepEqual(discarded, ['/sandbox/cache/Clipboard/late.png']);
});

test('destroy/cancel during text read never delivers late text to an editor', async () => {
  let finish;
  let discarded = 0;
  const request = new PasteAccessRequest(() => new Promise(resolve => { finish = resolve; }), async () => { discarded++; });
  const result = assert.rejects(request.completion, ClipboardReadCancelled);
  const reading = request.approve();
  request.cancel();
  request.cancel();
  finish(CONTENT);
  await Promise.all([result, reading]);
  assert.equal(discarded, 1);
});

test('security authorization failure rejects without reading clipboard', async () => {
  let reads = 0;
  const failure = new Error('security authorization failed');
  const request = new PasteAccessRequest(async () => { reads++; return CONTENT; }, async () => {});
  const result = assert.rejects(request.completion, error => error === failure);
  request.fail(failure);
  await result;
  await request.approve();
  assert.equal(reads, 0);
});

test('authorized read failure preserves its error instead of pretending to paste', async () => {
  const failure = new Error('provider read failed');
  const request = new PasteAccessRequest(async () => { throw failure; }, async () => {});
  const result = assert.rejects(request.completion, error => error === failure);
  await request.approve();
  await result;
});

test('cancelled completion cannot settle shutdown before both provider read and late PNG discard finish', async () => {
  let finishRead, finishDiscard, discardEntered;
  const reading = new Promise(resolve => { finishRead = resolve; });
  const discarded = new Promise(resolve => { finishDiscard = resolve; });
  const entered = new Promise(resolve => { discardEntered = resolve; });
  const request = new PasteAccessRequest(() => reading, async () => { discardEntered(); await discarded; });
  let settled = false;
  request.settled.then(() => { settled = true; });
  const rejected = assert.rejects(request.completion, ClipboardReadCancelled);
  const approved = request.approve();
  request.cancel();
  await rejected;
  assert.equal(settled, false);
  finishRead({text:'', imagePath:'/sandbox/cache/Clipboard/late.png'});
  await entered;
  assert.equal(settled, false);
  finishDiscard();
  await Promise.all([approved, request.settled]);
  assert.equal(settled, true);
});

test('failed late PNG cleanup preserves cancellation and reports the cleanup failure after actual settlement', async () => {
  let finish;
  const cleanupFailure = new Error('cache removal failed');
  const request = new PasteAccessRequest(() => new Promise(resolve => { finish = resolve; }), async () => { throw cleanupFailure; });
  const rejected = assert.rejects(request.completion, ClipboardReadCancelled);
  const approved = request.approve();
  request.cancel();
  finish({text:'', imagePath:'/sandbox/cache/Clipboard/late.png'});
  await Promise.all([rejected, approved, request.settled]);
  assert.equal(request.cleanupError, cleanupFailure);
});
