import { common } from '@kit.AbilityKit';
import { fileIo } from '@kit.CoreFileKit';

/** Storage paths are separate from the genuine context used by system services. */
export class AppStorageRoots {
  readonly filesDir: string;
  readonly cacheDir: string;
  readonly runId: string;
  private static active: AppStorageRoots | undefined = undefined;

  constructor(context: common.UIAbilityContext, runId: string = '') {
    if (runId.length > 0 && !AppStorageRoots.validRunId(runId)) {
      throw new Error('PhotoCraft device test run ID is invalid');
    }
    this.runId = runId;
    this.filesDir = runId.length === 0 ? context.filesDir : `${context.filesDir}/PhotoCraftTestRuns/${runId}/files`;
    this.cacheDir = runId.length === 0 ? context.cacheDir : `${context.cacheDir}/PhotoCraftTestRuns/${runId}/cache`;
  }

  static validRunId(runId: string): boolean {
    return /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(runId);
  }

  /** Called once by onCreate; onNewWant never selects another storage namespace. */
  static activate(context: common.UIAbilityContext, enabled: boolean, requestedRunId?: string): AppStorageRoots {
    const runId = enabled ? (requestedRunId ?? '') : '';
    if (enabled && requestedRunId !== undefined && !AppStorageRoots.validRunId(runId)) {
      throw new Error('PhotoCraft device test run ID is invalid');
    }
    const roots = new AppStorageRoots(context, runId);
    const active = AppStorageRoots.active;
    if (active !== undefined && (active.filesDir !== roots.filesDir || active.cacheDir !== roots.cacheDir ||
      active.runId !== roots.runId)) {
      throw new Error('PhotoCraft storage roots cannot change in a live process; start a cold test process');
    }
    if (runId.length > 0) {
      AppStorageRoots.ensureDirectory(context.filesDir, ['PhotoCraftTestRuns', runId, 'files']);
      AppStorageRoots.ensureDirectory(context.cacheDir, ['PhotoCraftTestRuns', runId, 'cache']);
    }
    AppStorageRoots.active = roots;
    return roots;
  }

  static current(context: common.UIAbilityContext): AppStorageRoots {
    return AppStorageRoots.active ?? new AppStorageRoots(context);
  }

  private static ensureDirectory(base: string, names: string[]): void {
    let directory = base;
    for (const name of ['', ...names]) {
      if (name.length > 0) {
        directory += `/${name}`;
        if (!fileIo.accessSync(directory)) { fileIo.mkdirSync(directory); }
      }
      const stat = fileIo.lstatSync(directory);
      if (!stat.isDirectory() || stat.isSymbolicLink()) {
        throw new Error('PhotoCraft device test storage must contain real directories');
      }
    }
  }
}
