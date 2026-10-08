import { afterAll, beforeAll, describe, expect, it, Level, Size, TestType } from '@ohos/hypium';
import { abilityDelegatorRegistry } from '@kit.TestKit';
import { common } from '@kit.AbilityKit';
import { KeyCode } from '@kit.InputKit';
import { fileIo } from '@kit.CoreFileKit';
import process from '@ohos.process';
import { DeviceHarness, DeviceSnapshot, StorageFingerprint, delay, fingerprintOrdinaryStorage, guardedCase,
  entryStorageDirectories, genuineApplicationContext, reportDeviceRuntime, reportOrdinaryCacheInventory, reportOrdinaryStorageRoots, reportStorageIsolation, requireCondition } from '../support/DeviceHarness';

const CASE = TestType.FUNCTION | Size.MEDIUMTEST | Level.LEVEL0;

interface RecoverySeed {
  processId: number;
  runId: string;
  name: string;
  blake3: string;
  revision: number;
  elapsedMilliseconds: number;
}

interface RecoveryInfo {
  key: string;
  document_name: string;
  original_path: string | null;
  revision: number;
}

async function runFiles(context: common.Context, harness: DeviceHarness): Promise<string> {
  const entry = await entryStorageDirectories(context);
  return `${entry.filesDir}/PhotoCraftTestRuns/${harness.runId}/files`;
}

async function assertIsolation(context: common.Context, before: StorageFingerprint, runId: string, suite: string): Promise<void> {
  const after = await reportStorageIsolation(context, before, runId, suite);
  expect(after.digest).assertEqual(before.digest);
  expect(after.entries).assertEqual(before.entries);
  expect(after.bytes).assertEqual(before.bytes);
}

export default function photoCraftPhaseTests(): void {
  describe('PhotoCraftRecovery', () => {
    let harness: DeviceHarness;
    let context: common.Context;
    let before: StorageFingerprint;
    let baselineReady: boolean = false;

    beforeAll(async () => {
      harness = new DeviceHarness('PhotoCraftRecovery');
      context = genuineApplicationContext();
      before = await fingerprintOrdinaryStorage(context);
      baselineReady = true;
      await reportOrdinaryStorageRoots(context, harness.runId, 'PhotoCraftRecovery');
      await reportOrdinaryCacheInventory(before, harness.runId, 'PhotoCraftRecovery');
      await reportDeviceRuntime(harness.runId, 'PhotoCraftRecovery');
      await harness.start();
    });

    afterAll(async () => { if (!baselineReady) { return; } await harness.finish(); await assertIsolation(context, before, harness.runId, 'PhotoCraftRecovery'); });

    it('seed_autosave', CASE, guardedCase(() => harness, 'seed_autosave', async () => {
      requireCondition(harness.snapshot().ui.session.documents.length === 0,
        'Recovery seed requires a fresh run ID and cold process');
      const root = await runFiles(context, harness);
      const markerPath = `${root}/recovery-test-seed.json`;
      requireCondition(!fileIo.accessSync(markerPath), 'Recovery marker already exists; use a fresh recovery run ID');
      const dialog = await harness.menu('edit.preferences.fileHandling');
      requireCondition(dialog.dialog !== undefined, 'Original File Handling preferences did not open');
      await harness.call('ui.dialog.set', { dialog: dialog.dialog as number, field: 'values',
        value: { fileHandling: { autosave: true, autosaveMinutes: 1, recoverOnLaunch: true } } });
      await harness.call('ui.dialog.confirm', { dialog: dialog.dialog as number });
      const name = `recovery-${harness.runId}`;
      await harness.newDocument(name);
      const started = Date.now();
      await harness.menu('image.adjustments.invert');
      const edited = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.document?.canUndo === true,
        'Recovery document edit did not commit');
      harness.assertPixel(edited, 0);
      requireCondition(edited.documentPixels !== null, 'Recovery composite missing');
      const recoveryDir = `${root}/PhotoCraft/Recovery`;
      let saved: RecoveryInfo | undefined = undefined;
      const deadline = Date.now() + 95000;
      while (Date.now() < deadline && saved === undefined) {
        if (fileIo.accessSync(recoveryDir)) {
          const entries = await fileIo.listFile(recoveryDir);
          for (const entry of entries) {
            if (!/^doc-[0-9]+\.json$/.test(entry)) { continue; }
            const info = JSON.parse(await fileIo.readText(`${recoveryDir}/${entry}`)) as RecoveryInfo;
            if (info.document_name === name && info.revision === edited.ui.document?.revision &&
              fileIo.accessSync(`${recoveryDir}/${info.key}.pcraft/manifest.json`)) {
              saved = info;
            }
          }
        }
        if (saved === undefined) { await delay(1000); }
      }
      requireCondition(saved !== undefined, 'Original one-minute autosave did not publish a recoverable bundle');
      const elapsed = Date.now() - started;
      requireCondition(elapsed >= 50000, 'Autosave fired before the original minute interval');
      requireCondition(saved?.original_path === null, 'Unsaved recovery fixture unexpectedly has a file binding');
      const marker: RecoverySeed = { processId: process.pid, runId: harness.runId, name: name,
        blake3: edited.documentPixels?.blake3 ?? '', revision: edited.ui.document?.revision ?? 0,
        elapsedMilliseconds: elapsed };
      const handle = await fileIo.open(markerPath, fileIo.OpenMode.CREATE | fileIo.OpenMode.NOFOLLOW | fileIo.OpenMode.WRITE_ONLY);
      try { await fileIo.write(handle.fd, JSON.stringify(marker)); await fileIo.fsync(handle.fd); }
      finally { await fileIo.close(handle); }
      await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_RECOVERY ${JSON.stringify({ stage: 'seed', runId: harness.runId,
        processId: process.pid, elapsedMilliseconds: elapsed, pixels: marker.blake3 })}\n`);
    }));

    it('verify_autosave', CASE, guardedCase(() => harness, 'verify_autosave', async () => {
      const marker = JSON.parse(await fileIo.readText(`${await runFiles(context, harness)}/recovery-test-seed.json`)) as RecoverySeed;
      requireCondition(marker.runId === harness.runId && marker.processId !== process.pid,
        'Recovery verification requires force-stop and a new process with the same run ID');
      const recovered = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.session.documents.length === 1 &&
        snapshot.ui.session.documents[0].name === marker.name && snapshot.documentPixels?.blake3 === marker.blake3,
        'Original startup did not recover the exact autosaved document', 20000);
      harness.assertPixel(recovered, 0);
      expect(recovered.ui.session.documents[0].dirty).assertTrue();
      expect(recovered.ui.session.documents[0].path).assertNull();
      await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_RECOVERY ${JSON.stringify({ stage: 'verify', runId: harness.runId,
        seedProcessId: marker.processId, processId: process.pid, pixels: recovered.documentPixels?.blake3 })}\n`);
    }));
  });

  describe('PhotoCraftClose', () => {
    let harness: DeviceHarness;
    let context: common.Context;
    let before: StorageFingerprint;
    let baselineReady: boolean = false;

    beforeAll(async () => {
      harness = new DeviceHarness('PhotoCraftClose');
      context = genuineApplicationContext();
      before = await fingerprintOrdinaryStorage(context);
      baselineReady = true;
      await reportOrdinaryStorageRoots(context, harness.runId, 'PhotoCraftClose');
      await reportOrdinaryCacheInventory(before, harness.runId, 'PhotoCraftClose');
      await reportDeviceRuntime(harness.runId, 'PhotoCraftClose');
      await harness.start();
      requireCondition(harness.snapshot().ui.session.documents.length === 0,
        'Close tests require a fresh run ID and cold process');
    });

    afterAll(async () => { if (!baselineReady) { return; } await harness.finish(); await assertIsolation(context, before, harness.runId, 'PhotoCraftClose'); });

    it('cancel_window_close', CASE, guardedCase(() => harness, 'cancel_window_close', async () => {
      await harness.newDocument(`close-${harness.runId}`);
      await harness.menu('image.adjustments.invert');
      const edited = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.document?.canUndo === true,
        'Close fixture edit did not commit');
      harness.assertPixel(edited, 0);
      const window = await harness.driver.findWindow({ bundleName: abilityDelegatorRegistry.getArguments().bundleName });
      if (window === undefined) { throw new Error('Actual PhotoCraft window was not found'); }
      await harness.armCloseMonitor();
      await window.close();
      await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.runtime.closePending,
        'Actual window close did not reach original unsaved-changes handling');
      await harness.driver.triggerKey(KeyCode.KEYCODE_ESCAPE);
      const cancelled = await harness.waitFor((snapshot: DeviceSnapshot) => !snapshot.runtime.closePending,
        'Actual Escape did not cancel the original close prompt');
      expect(cancelled.ui.session.documents.length).assertEqual(1);
      expect(cancelled.documentPixels?.blake3).assertEqual(edited.documentPixels?.blake3);
      expect(cancelled.ui.session.documents[0].dirty).assertTrue();
      await delay(200);
      harness.assertCloseWasCancelled();
      requireCondition(await harness.driver.findWindow({ bundleName: abilityDelegatorRegistry.getArguments().bundleName }) !== undefined,
        'Cancelled close did not preserve the actual application window');
      await harness.disarmCloseMonitor();
    }));

    it('save_window_close', CASE, guardedCase(() => harness, 'save_window_close', async () => {
      // This must be the final close phase: the real production window is destroyed.
      if (harness.snapshot().ui.session.documents.length === 0) {
        await harness.newDocument(`close-${harness.runId}`);
      }
      requireCondition(harness.snapshot().ui.session.documents.length === 1,
        'Successful close requires exactly one owned test document');
      const filename = `close-${harness.runId}.pcraft`;
      await harness.saveWithSystemPicker(filename);
      const externalBefore = await harness.publishedDigest(filename);
      await harness.menu('image.adjustments.invert');
      const edited = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.session.documents[0].dirty,
        'Saved close fixture did not become dirty');
      const expected = edited.documentPixels?.blake3;
      const bundle = abilityDelegatorRegistry.getArguments().bundleName;
      const window = await harness.driver.findWindow({ bundleName: bundle });
      if (window === undefined) { throw new Error('Actual PhotoCraft window was not found'); }
      await harness.armCloseMonitor();
      await window.close();
      await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.runtime.closePending,
        'Actual window close did not open the original save prompt');
      await harness.call('ui.close.save');
      const destroyed = await harness.awaitCloseDestruction();
      let closed = false;
      const deadline = Date.now() + 20000;
      while (Date.now() < deadline) {
        const remaining = await harness.driver.findWindow({ bundleName: bundle });
        if (remaining === null || remaining === undefined) { closed = true; break; }
        await delay(100);
      }
      requireCondition(closed, 'Successful original save did not close the actual window');
      await harness.disarmCloseMonitor();
      const externalAfter = await harness.publishedDigest(filename);
      expect(externalAfter !== externalBefore).assertTrue();
      await harness.assertPublishedFile(filename, 'pcraft');
      // Recreate the original ability and reopen through the real picker to prove saved content.
      await harness.start();
      const reopened = await harness.reopenWithSystemPicker(filename);
      expect(reopened.documentPixels?.blake3).assertEqual(expected);
      expect(reopened.ui.session.documents[reopened.ui.session.active].dirty).assertFalse();
      await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_CLOSE ${JSON.stringify({ actualWindowClosed: closed,
        windowStageDestroyed: destroyed.windowStageDestroyed, abilityDestroyed: destroyed.abilityDestroyed,
        externalBytesChanged: externalAfter !== externalBefore, reopenedPixels: expected })}\n`);
    }));
  });
}
