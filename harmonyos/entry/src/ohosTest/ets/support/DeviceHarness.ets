import { abilityDelegatorRegistry, Driver } from '@kit.TestKit';
import { DeviceComponent as Component, DeviceOn as ON, DeviceUiDriver } from './DeviceUiDriver';
import { application, common, UIAbility, Want } from '@kit.AbilityKit';
import { KeyCode } from '@kit.InputKit';
import { cryptoFramework } from '@kit.CryptoArchitectureKit';
import { fileIo } from '@kit.CoreFileKit';
import { util } from '@kit.ArkTS';
import nativePhotoCraft from 'libphotocraft.so';
import process from '@ohos.process';
import { AppStorageRoots } from '../../../main/ets/platform/AppStorageRoots';
import { PublishedFileEvidence } from '../../../main/ets/platform/DeviceTestEvidence';

export interface TestDocument {
  index: number;
  name: string;
  path: string | null;
  width: number;
  height: number;
  dirty: boolean;
  revision: number;
}

export interface TestPixels {
  width: number;
  height: number;
  rgba8: number[];
  blake3: string;
  omitted?: boolean;
}

export interface TestDialog {
  id: number;
  kind: string;
  fields: Record<string, Object>;
}

export interface TestSession { active: number; documents: TestDocument[]; }
export interface TestText { text: string; }
export interface TestLayer { name: string; kind: string; text?: TestText; }
export interface TestActiveDocument {
  width: number; height: number; revision: number; canUndo: boolean; canRedo: boolean; layers: TestLayer[];
}
export interface TestUi {
  tool: string;
  dialogs: TestDialog[];
  session: TestSession;
  document: TestActiveDocument | null;
  statusError: string | null;
}
export interface TestFileCompletion { id: number; result: string; displayName: string; accepted: boolean; }
export interface TestInput { nativeKeyEvents: number; nativePointerEvents: number; nativeTextEvents: number; lastNativeKeyCode: number; }
export interface TestStorageRoots { files: string; cache: string; }
export interface TestRuntime {
  frames: number; uiPresented: boolean; sizePixels: number[]; density: number; backend: string;
  storageRoots: TestStorageRoots;
  fileOperationPending: boolean; closePending: boolean; lastFileCompletion: TestFileCompletion | null; input: TestInput;
}
export interface DeviceSnapshot {
  runId: string;
  runtime: TestRuntime;
  canvasRect: number[];
  ui: TestUi;
  documentPixels: TestPixels | null;
}
export interface TestPointerEvent { kind: string; x: number; y: number; }
export interface TestFileHandlingPreferences { autosave: boolean; autosaveMinutes: number; recoverOnLaunch: boolean; }
export interface TestPreferencesValues { fileHandling: TestFileHandlingPreferences; }
export interface TestDocumentActivation { document: number; }
export interface ControlParams {
  id?: string;
  params?: TestDocumentActivation;
  dialog?: number;
  field?: string;
  value?: string | number | TestPreferencesValues;
  key?: string;
  text?: string;
  tool?: string;
  ctrl?: boolean;
  command?: boolean;
  shift?: boolean;
  events?: TestPointerEvent[];
}
interface ControlRequest { method: string; params: ControlParams; }
interface FailureEvidence { runId: string; suite: string; label: string; screenPath?: string; layoutPath?: string; }
export interface CloseLifecycle { windowStageDestroyed: boolean; abilityDestroyed: boolean; }

interface ControlReply {
  ok: boolean;
  error?: string;
  result: ControlResult;
}

export interface ControlResult {
  dialog?: number;
}

export interface StorageRootFingerprint {
  labels: string[]; pathDigest: string; digest: string; entries: number; bytes: number;
}
export interface EntryStorageDirectories { filesDir: string; cacheDir: string; }
interface StorageRootGroup { path: string; labels: string[]; }
interface NamedStorageRoot { label: string; path: string; }
export interface StorageFingerprint {
  digest: string;
  entries: number;
  bytes: number;
  roots: StorageRootFingerprint[];
}

interface FingerprintEntry {
  root: string;
  rootLabels: string[];
  relativePath: string;
  kind: string;
  size: number;
  hash: string;
}
interface FingerprintDifference {
  root: string;
  relativePath: string;
  pathDigest: string;
  change: string;
  beforeSize?: number;
  afterSize?: number;
  beforeHash?: string;
  afterHash?: string;
}
const fingerprintEntries: Map<string, FingerprintEntry[]> = new Map<string, FingerprintEntry[]>();

async function sha256Text(text: string): Promise<string> {
  const digest = cryptoFramework.createMd('SHA256');
  await digest.update({ data: new util.TextEncoder().encodeInto(text) });
  const result = await digest.digest();
  return Array.from(result.data).map((value: number) => value.toString(16).padStart(2, '0')).join('');
}

export function requireCondition(condition: boolean, message: string): void {
  if (!condition) { throw new Error(message); }
}

export async function delay(milliseconds: number): Promise<void> {
  await new Promise<void>((resolve: () => void) => { setTimeout(resolve, milliseconds); });
}

export function genuineApplicationContext(): common.ApplicationContext {
  return application.getApplicationContext();
}

/** The SDK module-context factory returns application roots. Derive the documented module sandbox
 * before launch, then start() must compare these paths against the real EntryAbility.context. */
export async function entryStorageDirectories(context: common.Context): Promise<EntryStorageDirectories> {
  requireCondition(context.filesDir.startsWith('/') && context.filesDir.endsWith('/files') &&
    context.cacheDir.endsWith('/cache'), 'Application storage roots do not match the documented sandbox layout');
  const base = context.filesDir.slice(0, -'/files'.length);
  requireCondition(context.cacheDir === `${base}/cache` && !base.includes('//') &&
    !base.split('/').some((part: string) => part === '.' || part === '..'),
    'Application files/cache roots do not share a safe sandbox base');
  const directories = [base, context.filesDir, context.cacheDir, `${base}/haps`, `${base}/haps/entry`,
    `${base}/haps/entry/files`, `${base}/haps/entry/cache`];
  for (const directory of directories) {
    const stat = await fileIo.lstat(directory);
    requireCondition(stat.isDirectory() && !stat.isSymbolicLink(),
      'Prelaunch entry storage must already exist as real sandbox directories');
  }
  return { filesDir: `${base}/haps/entry/files`, cacheDir: `${base}/haps/entry/cache` };
}

async function ordinaryRootGroups(context: common.Context): Promise<StorageRootGroup[]> {
  const entry = await entryStorageDirectories(context);
  const candidates: NamedStorageRoot[] = [
    { label: 'application.files', path: context.filesDir }, { label: 'application.cache', path: context.cacheDir },
    { label: 'entry.files', path: entry.filesDir }, { label: 'entry.cache', path: entry.cacheDir } ];
  const paths: Map<string, StorageRootGroup> = new Map<string, StorageRootGroup>();
  for (const candidate of candidates) {
    const existing = paths.get(candidate.path);
    if (existing === undefined) { paths.set(candidate.path, { path: candidate.path, labels: [candidate.label] }); }
    else { existing.labels.push(candidate.label); }
  }
  const groups = Array.from(paths.values());
  for (const group of groups) { group.labels.sort(); }
  return groups.sort((first: StorageRootGroup, second: StorageRootGroup) => first.labels.join('+').localeCompare(second.labels.join('+')));
}

/** Cover the genuine application and entry-module directories, deduplicated by actual path. */
export async function fingerprintOrdinaryStorage(context: common.Context): Promise<StorageFingerprint> {
  const aggregate = cryptoFramework.createMd('SHA256');
  const encoder = new util.TextEncoder();
  const fingerprint: StorageFingerprint = { digest: '', entries: 0, bytes: 0, roots: [] };
  const records: FingerprintEntry[] = [];
  for (const group of await ordinaryRootGroups(context)) {
    const digest = cryptoFramework.createMd('SHA256');
    const root: StorageRootFingerprint = { labels: group.labels, pathDigest: await sha256Text(group.path),
      digest: '', entries: 0, bytes: 0 };
    const rootName = group.labels.join('+');
    const scan = async (relative: string): Promise<void> => {
      const directory = relative.length === 0 ? group.path : `${group.path}/${relative}`;
      const names = (await fileIo.listFile(directory)).sort();
      for (const name of names) {
        if (relative.length === 0 && name === 'PhotoCraftTestRuns') { continue; }
        requireCondition(name !== '.' && name !== '..' && !name.includes('/'), 'Storage listing contains invalid entry');
        const child = relative.length === 0 ? name : `${relative}/${name}`;
        const path = `${group.path}/${child}`;
        const stat = await fileIo.lstat(path);
        requireCondition(!stat.isSymbolicLink(), 'Ordinary storage contains a link; cannot safely fingerprint');
        root.entries++;
        fingerprint.entries++;
        requireCondition(fingerprint.entries <= 10000, 'Ordinary storage exceeds fingerprint entry limit');
        await digest.update({ data: encoder.encodeInto(`${child}\0${stat.size}\0${stat.isDirectory() ? 'dir' : 'file'}\0`) });
        if (stat.isDirectory()) {
          records.push({ root: rootName, rootLabels: group.labels, relativePath: child, kind: 'directory', size: stat.size, hash: '' });
          await scan(child); continue;
        }
        requireCondition(stat.isFile(), 'Ordinary storage contains a special file');
        const fileDigest = cryptoFramework.createMd('SHA256');
        const handle = await fileIo.open(path, fileIo.OpenMode.READ_ONLY | fileIo.OpenMode.NOFOLLOW);
        try {
          let offset = 0;
          while (offset < stat.size) {
            const bytes = new ArrayBuffer(Math.min(65536, stat.size - offset));
            const count = await fileIo.read(handle.fd, bytes, { offset: offset });
            requireCondition(count > 0, 'Ordinary storage changed during fingerprint');
            await digest.update({ data: new Uint8Array(bytes, 0, count) });
            await fileDigest.update({ data: new Uint8Array(bytes, 0, count) });
            offset += count;
            root.bytes += count;
            fingerprint.bytes += count;
          }
        } finally { await fileIo.close(handle); }
        const fileResult = await fileDigest.digest();
        records.push({ root: rootName, rootLabels: group.labels, relativePath: child, kind: 'file', size: stat.size,
          hash: Array.from(fileResult.data).map((value: number) => value.toString(16).padStart(2, '0')).join('') });
      }
    };
    await scan('');
    const result = await digest.digest();
    root.digest = Array.from(result.data).map((value: number) => value.toString(16).padStart(2, '0')).join('');
    fingerprint.roots.push(root);
    await aggregate.update({ data: encoder.encodeInto(`${rootName}\0${root.pathDigest}\0${root.digest}\0`) });
  }
  const result = await aggregate.digest();
  fingerprint.digest = Array.from(result.data).map((value: number) => value.toString(16).padStart(2, '0')).join('');
  fingerprintEntries.set(fingerprint.digest, records);
  return fingerprint;
}

export async function reportOrdinaryCacheInventory(before: StorageFingerprint, runId: string, suite: string): Promise<void> {
  for (const entry of fingerprintEntries.get(before.digest) ?? []) {
    if (!entry.rootLabels.some((label: string) => label.endsWith('.cache')) || entry.kind !== 'file') { continue; }
    const sensitive = /(^|\/)(Documents|Clipboard)(\/|$)/.test(entry.relativePath);
    await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_CACHE_INVENTORY ${JSON.stringify({
      runId: runId, suite: suite, rootLabels: entry.rootLabels, relativePath: sensitive ? '<private-name-redacted>' : entry.relativePath,
      pathDigest: await sha256Text(`${entry.root}/${entry.relativePath}`), size: entry.size, hash: entry.hash })}\n`);
  }
}

export async function reportOrdinaryStorageRoots(context: common.Context, runId: string, suite: string): Promise<void> {
  const entry = await entryStorageDirectories(context);
  await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_STORAGE_ROOTS ${JSON.stringify({
    runId: runId, suite: suite, applicationFilesDir: context.filesDir, applicationCacheDir: context.cacheDir,
    entryFilesDir: entry.filesDir, entryCacheDir: entry.cacheDir })}\n`);
}

export async function reportDeviceRuntime(runId: string, suite: string): Promise<void> {
  await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_RUNTIME ${JSON.stringify({
    runId: runId, suite: suite, pid: process.pid })}\n`);
}

export async function reportStorageIsolation(context: common.Context, before: StorageFingerprint,
  runId: string, suite: string): Promise<StorageFingerprint> {
  await delay(500);
  const after = await fingerprintOrdinaryStorage(context);
  const originals: Map<string, FingerprintEntry> = new Map<string, FingerprintEntry>();
  for (const entry of fingerprintEntries.get(before.digest) ?? []) {
    originals.set(`${entry.root}/${entry.relativePath}`, entry);
  }
  const differences: FingerprintDifference[] = [];
  for (const entry of fingerprintEntries.get(after.digest) ?? []) {
    const key = `${entry.root}/${entry.relativePath}`;
    const original = originals.get(key);
    originals.delete(key);
    if (original !== undefined && original.kind === entry.kind && original.size === entry.size && original.hash === entry.hash) { continue; }
    const sensitive = /(^|\/)(Documents|Clipboard)(\/|$)/.test(entry.relativePath);
    differences.push({ root: entry.root, relativePath: sensitive ? '<private-name-redacted>' : entry.relativePath,
      pathDigest: await sha256Text(key), change: original === undefined ? 'added' : 'changed',
      beforeSize: original?.size, afterSize: entry.size, beforeHash: original?.hash, afterHash: entry.hash });
  }
  for (const entry of originals.values()) {
    const key = `${entry.root}/${entry.relativePath}`;
    const sensitive = /(^|\/)(Documents|Clipboard)(\/|$)/.test(entry.relativePath);
    differences.push({ root: entry.root, relativePath: sensitive ? '<private-name-redacted>' : entry.relativePath,
      pathDigest: await sha256Text(key), change: 'removed', beforeSize: entry.size, beforeHash: entry.hash });
  }
  await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_ISOLATION ${JSON.stringify({
    runId: runId, suite: suite, before: before, after: after, unchanged: before.digest === after.digest,
    differences: differences })}\n`);
  return after;
}

export function guardedCase(harness: () => DeviceHarness, label: string,
  operation: () => Promise<void>): () => Promise<void> {
  return async (): Promise<void> => { await harness().guard(label, operation); };
}

export class DeviceHarness {
  readonly runId: string;
  private readonly suite: string;
  private lastFailureEvidenceAt: number = 0;
  private uiDriver: Driver | undefined = undefined;
  private uiFacade: DeviceUiDriver | undefined = undefined;
  private closeMonitor: abilityDelegatorRegistry.AbilityMonitor | undefined = undefined;
  private closeLifecycle: CloseLifecycle = { windowStageDestroyed: false, abilityDestroyed: false };
  private testFolderCreated: boolean = false;
  private publicationUris: Map<string, string> = new Map<string, string>();
  readonly folderName: string;

  constructor(suite: string = 'PhotoCraftCore') {
    requireCondition(/^PhotoCraft(Core|Files|Recovery|Close)$/.test(suite), 'Unknown device test suite');
    this.suite = suite;
    const parameters = abilityDelegatorRegistry.getArguments().parameters;
    this.runId = parameters['-s photocraftTestRunId'] ?? parameters['photocraftTestRunId'] ?? '';
    requireCondition(AppStorageRoots.validRunId(this.runId), 'Run with -s photocraftTestRunId <fresh run ID>');
    this.folderName = `PhotoCraftTest-${this.runId}`;
  }

  get driver(): DeviceUiDriver {
    requireCondition(this.uiFacade !== undefined, 'UITest driver must be initialized after the real ability started');
    return this.uiFacade as DeviceUiDriver;
  }

  async initializeDriver(): Promise<void> {
    if (this.uiFacade !== undefined) { await this.uiFacade.close(); }
    let errorMessage = '';
    try { this.uiDriver = Driver.create(); }
    catch (error) { errorMessage = (error as Error).message; }
    this.uiFacade = new DeviceUiDriver(abilityDelegatorRegistry.getArguments().bundleName, this.runId,
      this.uiDriver === null ? undefined : this.uiDriver, this.snapshot().runtime.storageRoots.cache);
    await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_UI_DRIVER ${JSON.stringify({
      runId: this.runId, suite: this.suite, phase: 'after-ability-start',
      available: this.uiDriver !== undefined && this.uiDriver !== null, backend: this.uiFacade.backend, error: errorMessage })}\n`);
  }
  async finish(): Promise<void> {
    await this.guard('test-support teardown', async (): Promise<void> => {
      await this.disarmCloseMonitor();
      if (this.uiFacade !== undefined) { await this.uiFacade.close(); }
    });
  }

  async start(): Promise<void> {
    const args = abilityDelegatorRegistry.getArguments();
    const want: Want = { bundleName: args.bundleName, abilityName: 'EntryAbility',
      parameters: { 'photocraft.testRunId': this.runId } };
    await abilityDelegatorRegistry.getAbilityDelegator().startAbility(want);
    const snapshot = await this.waitFor((snapshot: DeviceSnapshot) => snapshot.runtime.frames > 1 && snapshot.runtime.uiPresented,
      'Native editor did not present a test-enabled frame', 20000, true);
    const appContext = genuineApplicationContext();
    const entry = await entryStorageDirectories(appContext);
    const ability = await abilityDelegatorRegistry.getAbilityDelegator().getCurrentTopAbility();
    await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_NATIVE_ROOTS ${JSON.stringify({
      runId: this.runId, pid: process.pid, nativeFiles: snapshot.runtime.storageRoots.files,
      nativeCache: snapshot.runtime.storageRoots.cache,
      expectedFiles: `${entry.filesDir}/PhotoCraftTestRuns/${this.runId}/files`,
      expectedCache: `${entry.cacheDir}/PhotoCraftTestRuns/${this.runId}/cache`,
      abilityFilesDir: ability.context.filesDir, abilityCacheDir: ability.context.cacheDir })}\n`);
    requireCondition(ability.context.filesDir === entry.filesDir && ability.context.cacheDir === entry.cacheDir,
      'Real EntryAbility context does not match the entry roots fingerprinted before launch');
    requireCondition(snapshot.runtime.storageRoots.files === `${entry.filesDir}/PhotoCraftTestRuns/${this.runId}/files` &&
      snapshot.runtime.storageRoots.cache === `${entry.cacheDir}/PhotoCraftTestRuns/${this.runId}/cache`,
      'Actual native storage roots do not match the genuine entry-module test namespace');
    await this.initializeDriver();
  }

  snapshot(): DeviceSnapshot {
    const serialized = nativePhotoCraft.testSnapshot(this.runId);
    requireCondition(serialized.length > 0, nativePhotoCraft.lastError() || 'Native test snapshot rejected');
    const snapshot = JSON.parse(serialized) as DeviceSnapshot;
    requireCondition(snapshot.runId === this.runId, 'Native snapshot belongs to another run');
    return snapshot;
  }

  async waitFor(predicate: (snapshot: DeviceSnapshot) => boolean, message: string, timeout: number = 10000,
    allowInitialization: boolean = false): Promise<DeviceSnapshot> {
    const until = Date.now() + timeout;
    while (Date.now() < until) {
      let snapshot: DeviceSnapshot | undefined = undefined;
      try { snapshot = this.snapshot(); }
      catch (error) { if (!allowInitialization) { throw error as Error; } }
      // Only the first start tolerates an uninitialized snapshot. Predicate/runtime failures remain authoritative.
      if (snapshot !== undefined && predicate(snapshot)) { return snapshot; }
      await delay(50);
    }
    try { await this.captureFailureEvidence(message); } catch (_captureError) { /* Preserve the timeout reason. */ }
    throw new Error(message);
  }

  async armCloseMonitor(): Promise<void> {
    requireCondition(this.closeMonitor === undefined, 'A close lifecycle monitor is already armed');
    const delegator = abilityDelegatorRegistry.getAbilityDelegator();
    const target = await delegator.getCurrentTopAbility();
    const roots = await entryStorageDirectories(genuineApplicationContext());
    requireCondition(target.context.filesDir === roots.filesDir && target.context.cacheDir === roots.cacheDir,
      'Close lifecycle target is not the genuine tested EntryAbility');
    this.closeLifecycle = { windowStageDestroyed: false, abilityDestroyed: false };
    const matchesTarget = (ability: UIAbility): boolean => ability === target &&
      ability.context === target.context && ability.context.filesDir === roots.filesDir && ability.context.cacheDir === roots.cacheDir;
    const monitor: abilityDelegatorRegistry.AbilityMonitor = { abilityName: 'EntryAbility', moduleName: 'entry',
      onWindowStageDestroy: (ability: UIAbility): void => {
        if (matchesTarget(ability)) { this.closeLifecycle.windowStageDestroyed = true; }
      }, onAbilityDestroy: (ability: UIAbility): void => {
        if (matchesTarget(ability)) { this.closeLifecycle.abilityDestroyed = true; }
      } };
    await delegator.addAbilityMonitor(monitor);
    this.closeMonitor = monitor;
  }

  assertCloseWasCancelled(): void {
    requireCondition(!this.closeLifecycle.windowStageDestroyed && !this.closeLifecycle.abilityDestroyed,
      'Cancelled window close destroyed the monitored EntryAbility or WindowStage');
  }

  async awaitCloseDestruction(): Promise<CloseLifecycle> {
    const until = Date.now() + 20000;
    while (Date.now() < until) {
      if (this.closeLifecycle.windowStageDestroyed && this.closeLifecycle.abilityDestroyed) {
        await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_CLOSE_LIFECYCLE ${JSON.stringify({
          runId: this.runId, suite: this.suite, windowStageDestroyed: true, abilityDestroyed: true })}\n`);
        return this.closeLifecycle;
      }
      await delay(50);
    }
    throw new Error(`Original EntryAbility close did not emit both destroy callbacks: ${JSON.stringify(this.closeLifecycle)}`);
  }

  async disarmCloseMonitor(): Promise<void> {
    const monitor = this.closeMonitor;
    this.closeMonitor = undefined;
    if (monitor !== undefined) { await abilityDelegatorRegistry.getAbilityDelegator().removeAbilityMonitor(monitor); }
  }

  private checkFileCompletion(snapshot: DeviceSnapshot, previousId?: number): void {
    const completion = snapshot.runtime.lastFileCompletion;
    if (completion !== null && completion.id !== previousId && (completion.result !== 'success' || !completion.accepted)) {
      throw new Error(`Original file request ${completion.id} failed (${completion.result}, accepted=${completion.accepted}): ${snapshot.ui.statusError ?? 'No status error supplied'}`);
    }
  }

  async call(method: string, params: ControlParams = {}): Promise<ControlResult> {
    const request: ControlRequest = { method: method, params: params };
    const ticket = nativePhotoCraft.testSubmit(this.runId, JSON.stringify(request));
    requireCondition(ticket.length > 0, nativePhotoCraft.lastError() || `Native test request rejected: ${method}`);
    const until = Date.now() + 15000;
    while (Date.now() < until) {
      const serialized = nativePhotoCraft.testPoll(this.runId, ticket);
      if (serialized.length > 0) {
        const reply = JSON.parse(serialized) as ControlReply;
        requireCondition(reply.ok, reply.error ?? `Original UI command failed: ${method}`);
        return reply.result;
      }
      await delay(20);
    }
    throw new Error(`Native test request timed out: ${method}`);
  }

  async guard(label: string, operation: () => Promise<void>): Promise<void> {
    try {
      await operation();
    } catch (error) {
      try { await this.captureFailureEvidence(label); }
      catch (_captureError) { /* A failed capture must not replace the original test error. */ }
      try { await this.disarmCloseMonitor(); }
      catch (_monitorError) { /* Preserve the operation failure if lifecycle-monitor cleanup also fails. */ }
      throw error as Error;
    }
  }

  async captureFailureEvidence(label: string): Promise<void> {
    const now = Date.now();
    if (now - this.lastFailureEvidenceAt < 2000) { return; }
    this.lastFailureEvidenceAt = now;
    const bundle = abilityDelegatorRegistry.getArguments().bundleName;
    requireCondition(/^[A-Za-z0-9_.]+$/.test(bundle), 'Test bundle name cannot be used for scoped failure evidence');
    const prefix = `/data/local/tmp/PhotoCraftTest-${this.runId}-${now}-failure`;
    const evidence: FailureEvidence = { runId: this.runId, suite: this.suite, label: label };
    const delegator = abilityDelegatorRegistry.getAbilityDelegator();
    try {
      const layout = await delegator.executeShellCommand(`uitest dumpLayout -b ${bundle} -p ${prefix}.json`);
      if (layout.exitCode === 0) { evidence.layoutPath = `${prefix}.json`; }
    } catch (_error) { /* The original test failure remains authoritative if evidence capture fails. */ }
    try {
      if (this.uiDriver !== undefined && this.uiDriver !== null && await this.uiDriver.screenCap(`${prefix}.png`)) {
        evidence.screenPath = `${prefix}.png`;
      }
    } catch (_error) { /* The app UID may be unable to create files in the shell's temporary directory. */ }
    if (evidence.screenPath === undefined) {
      try {
        const screen = await delegator.executeShellCommand(`uitest screenCap -p ${prefix}.png`);
        if (screen.exitCode === 0) { evidence.screenPath = `${prefix}.png`; }
      } catch (_error) { /* Some targets cannot capture during lifecycle transitions. */ }
    }
    if (evidence.layoutPath !== undefined || evidence.screenPath !== undefined) {
      await delegator.print(`PHOTOCRAFT_FAILURE_EVIDENCE ${JSON.stringify(evidence)}\n`);
    }
  }

  async reportInputDiagnostics(stage: string, shellOutput: string = '', shellExitCode: number = -1): Promise<void> {
    const snapshot = this.snapshot();
    const bundle = abilityDelegatorRegistry.getArguments().bundleName;
    let pasteDialogVisible = false;
    if (this.uiFacade !== undefined) {
      try {
        const pasteDialog = await this.uiFacade.findComponent(ON.id('photocraft-paste-authorize').inWindow(bundle));
        pasteDialogVisible = pasteDialog !== null && pasteDialog !== undefined;
      } catch (_error) { /* Input diagnostics cannot hide the original operation error. */ }
    }
    await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_INPUT_DIAGNOSTIC ${JSON.stringify({
      runId: this.runId, stage: stage, input: snapshot.runtime.input, tool: snapshot.ui.tool,
      activeDocument: snapshot.ui.session.documents[snapshot.ui.session.active]?.name,
      textLayers: snapshot.ui.document?.layers.filter((layer: TestLayer) => layer.text !== undefined)
        .map((layer: TestLayer) => layer.text?.text), pasteDialogVisible: pasteDialogVisible, driverAvailable: this.uiDriver !== undefined && this.uiDriver !== null, driverBackend: this.uiFacade?.backend,
      shellExitCode: shellExitCode, shellOutput: shellOutput })}\n`);
    if (stage.startsWith('failed-')) { await this.captureFailureEvidence(stage); }
  }

  async menu(id: string): Promise<ControlResult> { return await this.call('ui.menu.invoke', { id: id }); }

  async newDocument(name: string): Promise<DeviceSnapshot> {
    const previousCount = this.snapshot().ui.session.documents.length;
    const result = await this.menu('file.new');
    requireCondition(result.dialog !== undefined, 'Original File/New did not open its dialog');
    const dialog = result.dialog as number;
    await this.call('ui.dialog.set', { dialog: dialog, field: 'name', value: name });
    await this.call('ui.dialog.set', { dialog: dialog, field: 'width', value: 16 });
    await this.call('ui.dialog.set', { dialog: dialog, field: 'height', value: 16 });
    await this.call('ui.dialog.set', { dialog: dialog, field: 'background', value: 'white' });
    await this.call('ui.dialog.confirm', { dialog: dialog });
    return await this.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.session.documents.length === previousCount + 1 &&
      snapshot.ui.document?.width === 16 && snapshot.ui.document?.height === 16 && snapshot.documentPixels !== null, 'New document was not committed');
  }

  async activateDocument(index: number): Promise<DeviceSnapshot> {
    await this.call('ui.menu.invoke', { id: 'document.activate', params: { document: index } });
    return await this.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.session.active === index,
      'Original document activation did not switch the active document');
  }

  async closeDocument(): Promise<DeviceSnapshot> {
    const previous = this.snapshot().ui.session.documents.length;
    await this.menu('file.close');
    return await this.waitFor((snapshot: DeviceSnapshot) => snapshot.ui.session.documents.length === previous - 1,
      'Original File/Close did not remove the saved document');
  }

  async anyText(labels: string[], timeout: number = 10000): Promise<Component> {
    const until = Date.now() + timeout;
    while (Date.now() < until) {
      for (const label of labels) {
        const component = await this.driver.findComponent(ON.text(label).inWindow('com.huawei.hmos.filemanager'));
        if (component !== null && component !== undefined) { return component; }
      }
      await delay(100);
    }
    throw new Error(`System picker prerequisite missing: ${labels.join(' / ')}`);
  }

  private async folderNameField(): Promise<Component> {
    const fields = await this.driver.findComponents(ON.type('TextInput').focused(true).inWindow('com.huawei.hmos.filemanager'));
    requireCondition(fields.length === 1, 'New-folder editor must expose exactly one focused text field');
    return fields[0];
  }

  private async desktop(): Promise<void> {
    const desktop = await this.driver.waitForComponent(
      ON.id('FileManager_LeftBarColItemComp_key_file://docs/storage/Users/currentUser/Desktop').inWindow('com.huawei.hmos.filemanager'), 10000);
    await desktop.click();
  }

  private async enterTestFolder(): Promise<void> {
    await this.desktop();
    const folderId = `FileManager_FileListViewComp_0file://docs/storage/Users/currentUser/Desktop/${this.folderName}`;
    const folderSelector = ON.id(folderId).inWindow('com.huawei.hmos.filemanager');
    const folder = await this.driver.waitForComponent(folderSelector, 10000);
    await folder.doubleClick();
    let desktopTileGone = false;
    const deadline = Date.now() + 10000;
    while (Date.now() < deadline) {
      const tile = await this.driver.findComponent(folderSelector);
      if (tile === null || tile === undefined) { desktopTileGone = true; break; }
      await delay(100);
    }
    requireCondition(desktopTileGone, 'Picker did not leave Desktop for the owned test folder');
    await this.anyText([this.folderName]);
  }

  async saveWithSystemPicker(filename: string, menu: string = 'file.saveAs'): Promise<DeviceSnapshot> {
    requireCondition(/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(filename), 'Test output filename must be a plain owned filename');
    const previousCompletion = this.snapshot().runtime.lastFileCompletion?.id;
    await this.menu(menu);
    await this.anyText(['保存', 'Save']);
    if (!this.testFolderCreated) {
      await this.desktop();
      const existingFolder = await this.driver.findComponent(ON.text(this.folderName).inWindow('com.huawei.hmos.filemanager'));
      requireCondition(existingFolder === null || existingFolder === undefined,
        'Test output folder already exists; use a fresh run ID');
      await (await this.anyText(['新建文件夹', 'New folder', 'New Folder'])).click();
      await (await this.folderNameField()).inputText(this.folderName);
      await this.driver.triggerKey(KeyCode.KEYCODE_ENTER);
      this.testFolderCreated = true;
    }
    await this.enterTestFolder();
    const existingFile = await this.driver.findComponent(ON.text(filename).inWindow('com.huawei.hmos.filemanager'));
    requireCondition(existingFile === null || existingFile === undefined,
      'Refusing to overwrite an existing output in the system picker');
    const filenameField = await this.driver.waitForComponent(ON.id('pickerFileNameTextInput').inWindow('com.huawei.hmos.filemanager'), 10000);
    await filenameField.inputText(filename);
    await (await this.anyText(['保存', 'Save'])).click();
    return await this.awaitFileSuccess(filename, previousCompletion);
  }

  private publishedUri(filename: string): string {
    const evidence = AppStorage.get<PublishedFileEvidence>('photocraft.test.publishedFile');
    if (evidence !== undefined && evidence.runId === this.runId && evidence.filename === filename) {
      const pathname = decodeURIComponent(evidence.uri.split(/[?#]/)[0]);
      requireCondition(pathname.startsWith('file://docs/') && pathname.endsWith(`/${this.folderName}/${filename}`) &&
        !pathname.split('/').includes('..'), 'Provider URI does not match the verified owned test folder and filename');
      this.publicationUris.set(filename, evidence.uri);
    }
    const uri = this.publicationUris.get(filename);
    requireCondition(uri !== undefined && uri.length > 0, 'Missing genuine system picker publication evidence');
    return uri ?? '';
  }

  async assertPublishedFile(filename: string, extension: string): Promise<void> {
    const uri = this.publishedUri(filename);
    const handle = await fileIo.open(uri, fileIo.OpenMode.READ_ONLY);
    try {
      const stat = await fileIo.stat(handle.fd);
      requireCondition(stat.size > 0, 'System picker published an empty file');
      const bytes = new ArrayBuffer(16);
      const count = await fileIo.read(handle.fd, bytes);
      const header = new Uint8Array(bytes, 0, count);
      if (extension === 'png') {
        const signature: number[] = [137, 80, 78, 71, 13, 10, 26, 10];
        requireCondition(signature.every((value: number, index: number) => header[index] === value),
          'External PNG signature differs');
      } else if (extension === 'pcraft') {
        requireCondition(header[0] === 80 && header[1] === 75 && header[2] === 3 && header[3] === 4,
          'External pcraft ZIP signature differs');
      } else if (extension === 'psd') {
        requireCondition(header[0] === 56 && header[1] === 66 && header[2] === 80 && header[3] === 83,
          'External PSD signature differs');
      }
    } finally { await fileIo.close(handle); }
  }

  async publishedDigest(filename: string): Promise<string> {
    const uri = this.publishedUri(filename);
    const digest = cryptoFramework.createMd('SHA256');
    const handle = await fileIo.open(uri, fileIo.OpenMode.READ_ONLY);
    try {
      const stat = await fileIo.stat(handle.fd);
      let offset = 0;
      while (offset < stat.size) {
        const bytes = new ArrayBuffer(Math.min(65536, stat.size - offset));
        const count = await fileIo.read(handle.fd, bytes, { offset: offset });
        requireCondition(count > 0, 'External test file changed during hashing');
        await digest.update({ data: new Uint8Array(bytes, 0, count) });
        offset += count;
      }
    } finally { await fileIo.close(handle); }
    const result = await digest.digest();
    return Array.from(result.data).map((value: number) => value.toString(16).padStart(2, '0')).join('');
  }

  /** Independently change only two genuine picker-authorized fixtures, leaving the native import cache untouched. */
  async replaceOwnedPublishedFile(sourceFilename: string, targetFilename: string): Promise<void> {
    requireCondition(sourceFilename !== targetFilename &&
      /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}\.pcraft$/.test(sourceFilename) &&
      /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}\.pcraft$/.test(targetFilename),
      'External replacement requires distinct owned pcraft fixture names');
    const sourceUri = this.publishedUri(sourceFilename);
    const targetUri = this.publishedUri(targetFilename);
    requireCondition(sourceUri !== targetUri, 'External replacement cannot overwrite its source');
    const source = await fileIo.open(sourceUri, fileIo.OpenMode.READ_ONLY);
    try {
      const stat = await fileIo.stat(source.fd);
      requireCondition(stat.size >= 4 && stat.size <= 4 * 1024 * 1024, 'Owned replacement fixture exceeds size limits');
      const bytes = new ArrayBuffer(stat.size);
      const read = await fileIo.read(source.fd, bytes);
      const header = new Uint8Array(bytes);
      requireCondition(read === stat.size && header[0] === 80 && header[1] === 75 && header[2] === 3 && header[3] === 4,
        'Owned replacement source is not a complete pcraft fixture');
      const target = await fileIo.open(targetUri, fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.TRUNC);
      try {
        requireCondition(await fileIo.write(target.fd, bytes) === bytes.byteLength, 'External fixture write was incomplete');
        await fileIo.fsync(target.fd);
      } finally { await fileIo.close(target); }
    } finally { await fileIo.close(source); }
  }

  async awaitFileSuccess(filename: string, previousId?: number): Promise<DeviceSnapshot> {
    return await this.waitFor((snapshot: DeviceSnapshot): boolean => {
      this.checkFileCompletion(snapshot, previousId);
      return !snapshot.runtime.fileOperationPending && snapshot.runtime.lastFileCompletion?.id !== previousId &&
        snapshot.runtime.lastFileCompletion?.accepted === true && snapshot.runtime.lastFileCompletion?.result === 'success' &&
        snapshot.runtime.lastFileCompletion?.displayName === filename;
    },
      'Original file request did not complete successfully', 20000);
  }

  async reopenWithSystemPicker(filename: string): Promise<DeviceSnapshot> {
    const before = this.snapshot();
    const previousCount = before.ui.session.documents.length;
    const previousId = before.runtime.lastFileCompletion?.id;
    await this.menu('file.open');
    await this.enterTestFolder();
    await (await this.anyText([filename])).click();
    const confirm = await this.driver.findComponent(ON.text('打开').inWindow('com.huawei.hmos.filemanager')) ?? await this.driver.findComponent(ON.text('Open').inWindow('com.huawei.hmos.filemanager'));
    if (confirm !== null && confirm !== undefined) { await confirm.click(); }
    return await this.waitFor((snapshot: DeviceSnapshot): boolean => {
      this.checkFileCompletion(snapshot, previousId);
      return snapshot.ui.session.documents.length > previousCount &&
        snapshot.ui.session.documents[snapshot.ui.session.active]?.path?.endsWith(`/${filename}`) === true;
    },
      'System open did not load the chosen file', 20000);
  }

  assertPixel(snapshot: DeviceSnapshot, gray: number): void {
    const pixels = snapshot.documentPixels;
    requireCondition(pixels !== null && pixels.width === 16 && pixels.height === 16 &&
      snapshot.ui.document?.width === 16 && snapshot.ui.document?.height === 16 &&
      pixels.rgba8.length === 16 * 16 * 4, 'Missing full 16x16 composite oracle');
    if (pixels === null) { return; }
    for (let index = 0; index < pixels.rgba8.length; index += 4) {
      requireCondition(pixels.rgba8[index] === gray && pixels.rgba8[index + 1] === gray &&
        pixels.rgba8[index + 2] === gray && pixels.rgba8[index + 3] === 255, 'Composite pixel content differs');
    }
  }
}
