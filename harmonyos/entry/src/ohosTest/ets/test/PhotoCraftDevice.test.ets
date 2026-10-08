import { afterAll, beforeAll, describe, expect, it, Level, Size, TestType } from '@ohos/hypium';
import { abilityDelegatorRegistry } from '@kit.TestKit';
import { DeviceOn as ON } from '../support/DeviceUiDriver';
import { common } from '@kit.AbilityKit';
import { KeyCode } from '@kit.InputKit';
import { DeviceHarness, DeviceSnapshot, StorageFingerprint, delay, fingerprintOrdinaryStorage, guardedCase,
  entryStorageDirectories, genuineApplicationContext, reportDeviceRuntime, reportOrdinaryCacheInventory, reportOrdinaryStorageRoots, reportStorageIsolation, requireCondition } from '../support/DeviceHarness';

const CASE = TestType.FUNCTION | Size.MEDIUMTEST | Level.LEVEL0;

export default function photoCraftDeviceTests(): void {
  describe('PhotoCraftCore', () => {
    let harness: DeviceHarness;
    let ordinaryContext: common.Context;
    let before: StorageFingerprint;
    let baselineReady: boolean = false;

    beforeAll(async () => {
      harness = new DeviceHarness('PhotoCraftCore');
      ordinaryContext = genuineApplicationContext();
      before = await fingerprintOrdinaryStorage(ordinaryContext);
      baselineReady = true;
      await reportOrdinaryStorageRoots(ordinaryContext, harness.runId, 'PhotoCraftCore');
      await reportOrdinaryCacheInventory(before, harness.runId, 'PhotoCraftCore');
      await reportDeviceRuntime(harness.runId, 'PhotoCraftCore');
      await harness.start();
    });

    afterAll(async () => {
      if (!baselineReady) { return; }
      await harness.finish();
      const after = await reportStorageIsolation(ordinaryContext, before, harness.runId, 'PhotoCraftCore');
      expect(after.digest).assertEqual(before.digest);
      expect(after.entries).assertEqual(before.entries);
      expect(after.bytes).assertEqual(before.bytes);
    });

    it('new_edit_undo_redo', CASE, guardedCase(() => harness, 'new_edit_undo_redo', async () => {
      const created = await harness.newDocument(`core-${harness.runId}`);
      harness.assertPixel(created, 255);
      const whiteHash = created.documentPixels?.blake3;
      await harness.menu('image.adjustments.invert');
      const edited = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.document?.canUndo === true &&
        snapshot.documentPixels?.blake3 !== whiteHash, 'Original Invert did not change pixels');
      harness.assertPixel(edited, 0);
      const blackHash = edited.documentPixels?.blake3;
      await harness.menu('edit.undo');
      const undone = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.documentPixels?.blake3 === whiteHash,
        'Original Undo did not restore the white composite');
      harness.assertPixel(undone, 255);
      expect(undone.ui.document?.canRedo).assertTrue();
      await harness.menu('edit.redo');
      const redone = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.documentPixels?.blake3 === blackHash,
        'Original Redo did not restore the black composite');
      harness.assertPixel(redone, 0);
    }));

    it('dialog_cancel_and_two_documents', CASE, guardedCase(() => harness, 'dialog_cancel_and_two_documents', async () => {
      const first = await harness.newDocument(`first-${harness.runId}`);
      const firstIndex = first.ui.session.active;
      const firstRevision = first.ui.session.documents[firstIndex].revision;
      const firstCount = first.ui.session.documents.length;
      const dialog = await harness.menu('file.new');
      requireCondition(dialog.dialog !== undefined, 'New dialog not opened');
      await harness.call('ui.dialog.cancel', { dialog: dialog.dialog as number });
      const cancelled = harness.snapshot();
      expect(cancelled.ui.session.documents.length).assertEqual(firstCount);
      expect(cancelled.ui.session.active).assertEqual(firstIndex);
      const second = await harness.newDocument(`second-${harness.runId}`);
      expect(second.ui.session.active !== firstIndex).assertTrue();
      await harness.menu('image.adjustments.invert');
      const edited = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.document?.canUndo === true,
        'Second document edit not committed');
      harness.assertPixel(edited, 0);
      expect(edited.ui.session.documents[firstIndex].revision).assertEqual(firstRevision);
    }));

    it('synthetic_and_actual_keyboard', CASE, guardedCase(() => harness, 'synthetic_and_actual_keyboard', async () => {
      await harness.newDocument(`input-${harness.runId}`);
      await harness.call('ui.pointer', { tool: 'move', events: [] });
      await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.tool === 'Move',
        'Shortcut precondition did not select the original Move tool');
      const beforeInput = harness.snapshot().runtime.input.nativeKeyEvents;
      try {
        await harness.call('ui.key', { key: 'B' });
        const synthetic = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.tool === 'Brush',
          'Synthetic shortcut did not select Brush');
        expect(synthetic.runtime.input.nativeKeyEvents).assertEqual(beforeInput);
        const surface = await harness.driver.waitForComponent(ON.id('photocraft-surface'), 10000);
        await surface.click();
        await harness.driver.triggerKey(KeyCode.KEYCODE_M);
        const actual = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.tool === 'RectMarquee' &&
          snapshot.runtime.input.nativeKeyEvents > beforeInput, 'UITest keyboard event did not traverse XComponent input');
        expect(actual.runtime.input.lastNativeKeyCode).assertEqual(KeyCode.KEYCODE_M);
        await harness.reportInputDiagnostics('actual-keyboard');
      } catch (error) {
        try { await harness.reportInputDiagnostics('failed-keyboard'); } catch (_diagnosticError) {}
        throw error as Error;
      }
    }));

    it('synthetic_and_actual_chinese_text', CASE, guardedCase(() => harness, 'synthetic_and_actual_chinese_text', async () => {
      await harness.newDocument(`chinese-${harness.runId}`);
      await harness.call('ui.pointer', { tool: 'type', events: [
        { kind: 'down', x: 2, y: 8 }, { kind: 'up', x: 2, y: 8 } ] });
      const nativeTextBefore = harness.snapshot().runtime.input.nativeTextEvents;
      await harness.call('ui.type', { text: '合成😀' });
      const synthetic = await harness.waitFor((snapshot: DeviceSnapshot) =>
        snapshot.ui.document?.layers.some((layer) => layer.text?.text === '合成😀') === true,
        'Synthetic Chinese text did not reach the original Type tool');
      expect(synthetic.runtime.input.nativeTextEvents).assertEqual(nativeTextBefore);
      const delegator = abilityDelegatorRegistry.getAbilityDelegator();
      const nativeKeysBefore = harness.snapshot().runtime.input.nativeKeyEvents;
      const injected = await delegator.executeShellCommand('uitest uiInput text 真机中文😀');
      await harness.reportInputDiagnostics('uitest-chinese-clipboard', injected.stdResult, injected.exitCode);
      requireCondition(injected.exitCode === 0 && injected.stdResult.trim().length === 0,
        'Official UITest reported an error while injecting the Chinese clipboard fixture');
      // Official UiTest implements non-ASCII text through the clipboard and real Ctrl+V.
      // A rejected direct read may require the production security component; neither path is a direct IME commit.
      const bundle = abilityDelegatorRegistry.getArguments().bundleName;
      try {
        const pasteArrived = (snapshot: DeviceSnapshot): boolean => snapshot.runtime.input.nativeKeyEvents > nativeKeysBefore &&
          snapshot.ui.document?.layers.some((layer) => layer.text?.text === '合成😀真机中文😀') === true;
        let pasteAuthorized = false;
        let actual = harness.snapshot();
        const until = Date.now() + 10000;
        while (!pasteArrived(actual) && Date.now() < until) {
          if (!pasteAuthorized) {
            const approve = await harness.driver.findComponent(ON.id('photocraft-paste-authorize').inWindow(bundle));
            actual = harness.snapshot();
            if (!pasteArrived(actual) && approve !== null && approve !== undefined) {
              await approve.click();
              pasteAuthorized = true;
            }
          }
          await delay(100);
          actual = harness.snapshot();
        }
        requireCondition(pasteArrived(actual),
          'Actual Chinese clipboard paste did not traverse the production input bridge');
        expect(actual.runtime.input.nativeTextEvents).assertEqual(nativeTextBefore);
        expect(actual.ui.document?.layers.some((layer) => layer.text?.text === '合成😀真机中文😀')).assertTrue();
        await harness.reportInputDiagnostics(pasteAuthorized ? 'authorized-chinese-clipboard' : 'direct-chinese-clipboard',
          injected.stdResult, injected.exitCode);
      } catch (error) {
        try { await harness.reportInputDiagnostics('failed-chinese-clipboard', injected.stdResult, injected.exitCode); } catch (_diagnosticError) {}
        throw error as Error;
      }
      await harness.driver.triggerKey(KeyCode.KEYCODE_ESCAPE);
    }));

    it('background_foreground', CASE, guardedCase(() => harness, 'background_foreground', async () => {
      const delegator = abilityDelegatorRegistry.getAbilityDelegator();
      const ability = await delegator.getCurrentTopAbility();
      const beforeBackground = harness.snapshot();
      await delegator.doAbilityBackground(ability);
      await delay(700);
      const hidden = harness.snapshot();
      await delay(700);
      const stillHidden = harness.snapshot();
      expect(stillHidden.runtime.frames).assertEqual(hidden.runtime.frames);
      await delegator.doAbilityForeground(ability);
      const resumed = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.runtime.frames > hidden.runtime.frames,
        'Native rendering did not resume after foreground');
      expect(resumed.runId).assertEqual(beforeBackground.runId);
      expect(resumed.ui.session.documents.length).assertEqual(beforeBackground.ui.session.documents.length);
      expect(resumed.documentPixels?.blake3).assertEqual(beforeBackground.documentPixels?.blake3);
    }));
  });

  describe('PhotoCraftFiles', () => {
    let harness: DeviceHarness;
    let ordinaryContext: common.Context;
    let before: StorageFingerprint;
    let baselineReady: boolean = false;

    beforeAll(async () => {
      harness = new DeviceHarness('PhotoCraftFiles');
      ordinaryContext = genuineApplicationContext();
      before = await fingerprintOrdinaryStorage(ordinaryContext);
      baselineReady = true;
      await reportOrdinaryStorageRoots(ordinaryContext, harness.runId, 'PhotoCraftFiles');
      await reportOrdinaryCacheInventory(before, harness.runId, 'PhotoCraftFiles');
      await reportDeviceRuntime(harness.runId, 'PhotoCraftFiles');
      await harness.start();
    });

    afterAll(async () => {
      if (!baselineReady) { return; }
      await harness.finish();
      const after = await reportStorageIsolation(ordinaryContext, before, harness.runId, 'PhotoCraftFiles');
      expect(after.digest).assertEqual(before.digest);
    });

    it('pcraft_system_roundtrip', CASE, guardedCase(() => harness, 'pcraft_system_roundtrip', async () => { await formatRoundtrip(harness, 'pcraft'); }));
    it('psd_system_roundtrip', CASE, guardedCase(() => harness, 'psd_system_roundtrip', async () => { await formatRoundtrip(harness, 'psd'); }));
    it('png_system_roundtrip', CASE, guardedCase(() => harness, 'png_system_roundtrip', async () => { await formatRoundtrip(harness, 'png'); }));

    it('system_picker_cancel', CASE, guardedCase(() => harness, 'system_picker_cancel', async () => {
      const created = await harness.newDocument(`cancel-${harness.runId}`);
      const previousDocuments = created.ui.session.documents.length;
      const digest = created.documentPixels?.blake3;
      await harness.menu('file.open');
      await harness.anyText(['打开', 'Open', '取消', 'Cancel']);
      await harness.driver.pressBack();
      const cancelled = await harness.waitFor((snapshot: DeviceSnapshot) => !snapshot.runtime.fileOperationPending &&
        snapshot.runtime.lastFileCompletion?.result === 'cancel', 'Picker cancel did not acknowledge the file request');
      expect(cancelled.ui.session.documents.length).assertEqual(previousDocuments);
      expect(cancelled.documentPixels?.blake3).assertEqual(digest);
      // Prove cancellation cleared the original file request instead of leaving it busy.
      const next = await harness.newDocument(`after-cancel-${harness.runId}`);
      expect(next.ui.session.documents.length).assertEqual(previousDocuments + 1);
    }));

    it('two_documents_alternate_system_save', CASE, guardedCase(() => harness, 'two_documents_alternate_system_save', async () => {
      const first = await harness.newDocument(`pairA-${harness.runId}`);
      const firstIndex = first.ui.session.active;
      const firstName = `pairA-${harness.runId}.pcraft`;
      await harness.saveWithSystemPicker(firstName);
      const firstBefore = await harness.publishedDigest(firstName);
      const second = await harness.newDocument(`pairB-${harness.runId}`);
      const secondIndex = second.ui.session.active;
      const secondName = `pairB-${harness.runId}.pcraft`;
      await harness.menu('image.adjustments.invert');
      const secondBlack = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.document?.canUndo === true,
        'Second document edit did not commit');
      harness.assertPixel(secondBlack, 0);
      await harness.saveWithSystemPicker(secondName);
      const secondBefore = await harness.publishedDigest(secondName);
      await harness.activateDocument(firstIndex);
      await harness.menu('image.adjustments.invert');
      const firstBlack = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.session.documents[firstIndex].dirty,
        'First document edit did not commit');
      harness.assertPixel(firstBlack, 0);
      await harness.menu('file.save');
      const savedFirst = await harness.awaitFileSuccess(firstName, firstBlack.runtime.lastFileCompletion?.id);
      expect(savedFirst.ui.session.active).assertEqual(firstIndex);
      expect(savedFirst.ui.session.documents[secondIndex].path?.endsWith(`/${secondName}`)).assertTrue();
      expect(savedFirst.ui.session.documents[secondIndex].dirty).assertFalse();
      const firstAfter = await harness.publishedDigest(firstName);
      expect(firstAfter !== firstBefore).assertTrue();
      expect(await harness.publishedDigest(secondName)).assertEqual(secondBefore);
      await harness.activateDocument(secondIndex);
      harness.assertPixel(harness.snapshot(), 0);
      await harness.menu('image.adjustments.invert');
      const secondWhite = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.session.documents[secondIndex].dirty,
        'Second alternating edit did not commit');
      harness.assertPixel(secondWhite, 255);
      await harness.menu('file.save');
      const savedSecond = await harness.awaitFileSuccess(secondName, secondWhite.runtime.lastFileCompletion?.id);
      expect(savedSecond.ui.session.active).assertEqual(secondIndex);
      expect(savedSecond.ui.session.documents[firstIndex].path?.endsWith(`/${firstName}`)).assertTrue();
      expect(savedSecond.ui.session.documents[firstIndex].dirty).assertFalse();
      expect(await harness.publishedDigest(secondName) !== secondBefore).assertTrue();
      expect(await harness.publishedDigest(firstName)).assertEqual(firstAfter);
      await harness.closeDocument();
      harness.assertPixel(await harness.reopenWithSystemPicker(secondName), 255);
      await harness.activateDocument(firstIndex);
      await harness.closeDocument();
      harness.assertPixel(await harness.reopenWithSystemPicker(firstName), 0);
    }));

    it('recent_reimports_fresh_external_bytes', CASE, guardedCase(() => harness, 'recent_reimports_fresh_external_bytes', async () => {
      const replacementName = `replacement-${harness.runId}.pcraft`;
      await harness.newDocument(`replacement-${harness.runId}`);
      await harness.menu('image.adjustments.invert');
      const black = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.document?.canUndo === true,
        'External replacement fixture edit did not commit');
      harness.assertPixel(black, 0);
      await harness.saveWithSystemPicker(replacementName);
      const replacementDigest = await harness.publishedDigest(replacementName);
      await harness.newDocument(`recent-${harness.runId}`);
      const filename = `recent-${harness.runId}.pcraft`;
      await harness.saveWithSystemPicker(filename);
      const originalExternal = await harness.publishedDigest(filename);
      const imported = await harness.reopenWithSystemPicker(filename);
      harness.assertPixel(imported, 255);
      const importedIndex = imported.ui.session.active;
      const importedPath = imported.ui.session.documents[importedIndex].path;
      const whiteHash = imported.documentPixels?.blake3;
      await harness.replaceOwnedPublishedFile(replacementName, filename);
      expect(await harness.publishedDigest(filename)).assertEqual(replacementDigest);
      expect(replacementDigest !== originalExternal).assertTrue();
      // This independent provider mutation must leave the existing imported white cache unchanged.
      const stillWhite = harness.snapshot();
      harness.assertPixel(stillWhite, 255);
      expect(stillWhite.documentPixels?.blake3).assertEqual(whiteHash);
      expect(stillWhite.ui.session.documents[importedIndex].path).assertEqual(importedPath);
      const unrelated = await harness.newDocument(`unrelated-${harness.runId}`);
      const count = unrelated.ui.session.documents.length;
      await harness.menu('file.openRecent.0');
      const recent = await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.session.documents.length > count &&
        snapshot.documentPixels?.blake3 === black.documentPixels?.blake3,
        'Open Recent did not reimport the independently replaced provider bytes', 20000);
      harness.assertPixel(recent, 0);
      expect(recent.ui.session.documents[recent.ui.session.active].path !== importedPath).assertTrue();
    }));
  });
}

async function formatRoundtrip(harness: DeviceHarness, extension: string): Promise<void> {
  await harness.newDocument(`roundtrip-${extension}-${harness.runId}`);
  if (extension !== 'png') {
    const layer = await harness.menu('layer.new.layer');
    requireCondition(layer.dialog !== undefined, 'Original New Layer did not open its dialog');
    await harness.call('ui.dialog.set', { dialog: layer.dialog as number, field: 'name', value: 'Roundtrip Layer' });
    await harness.call('ui.dialog.confirm', { dialog: layer.dialog as number });
    await harness.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.document?.layers.length === 2,
      'Original New Layer did not commit the second layer');
  }
  const expected = harness.snapshot().documentPixels?.blake3;
  const filename = `roundtrip-${harness.runId}.${extension}`;
  await harness.saveWithSystemPicker(filename);
  await harness.assertPublishedFile(filename, extension);
  await harness.closeDocument();
  const reopened = await harness.reopenWithSystemPicker(filename);
  expect(reopened.documentPixels?.blake3).assertEqual(expected);
  harness.assertPixel(reopened, 255);
  if (extension !== 'png') {
    expect(reopened.ui.document?.layers.length).assertEqual(2);
    expect(reopened.ui.document?.layers.some((layer) => layer.name === 'Roundtrip Layer')).assertTrue();
  }
}
