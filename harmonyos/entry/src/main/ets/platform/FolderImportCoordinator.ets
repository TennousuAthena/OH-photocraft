import { shellString } from './ShellLocalization';
import { FolderLimits, validateFolderLimits } from './SandboxFolderManifest';
import { FolderImportCompletion, FolderImportProgress, FolderImportRequest, FolderImportService } from './FolderImportBridge';

export interface FolderImportEvent {
  kind: 'folderImport';
  id: number;
  target: string;
  intent: string;
  scope: 'topLevel';
  copyLayout: 'sourceBasenameChild';
  limits: FolderLimits;
}

class FolderImportJob {
  request: FolderImportEvent;
  cancelled: boolean = false;
  cleaning: boolean = false;
  transferred: boolean = false;
  pending: FolderImportCompletion | undefined = undefined;
  progress: FolderImportProgress | undefined = undefined;
  lastProgressTime: number = 0;
  operation: Promise<void> = Promise.resolve();

  constructor(request: FolderImportEvent) { this.request = request; }
}

/** A single independent job slot. A true enqueue transfers ownership; false retains it for retry. */
export class FolderImportCoordinator {
  private service: FolderImportService;
  private enqueue: (serialized: string) => boolean;
  private available: () => boolean;
  private report: (message: string) => void;
  private activity: (active: boolean) => void;
  private active: FolderImportJob | undefined = undefined;
  private rejected: FolderImportCompletion[] = [];
  private seen: Set<number> = new Set<number>();
  private disposed: boolean = false;

  constructor(service: FolderImportService, enqueue: (serialized: string) => boolean,
    available: () => boolean, report: (message: string) => void, activity: (active: boolean) => void) {
    this.service = service;
    this.enqueue = enqueue;
    this.available = available;
    this.report = report;
    this.activity = activity;
  }

  hasPending(): boolean { return this.active !== undefined || this.rejected.length > 0; }

  begin(request: FolderImportEvent): void {
    if (this.disposed) { return; }
    if (!Number.isSafeInteger(request.id) || request.id < 0 || typeof request.target !== 'string' ||
      request.target.length < 1 || request.target.length > 512 || request.target.includes('\x00')) {
      this.report(shellString("shell_invalid_folder_input_request_identity", "Invalid folder input request identity"));
      return;
    }
    // A repeated id is never a fresh resource request, even if its target was changed.
    if (this.seen.has(request.id)) { return; }
    if (this.seen.size >= 4096 || this.rejected.length >= 64) {
      this.report(shellString("shell_the_folder_input_queue_is_full_cancel_and_try", "The folder input queue is full. Cancel and try again"));
      return;
    }
    this.seen.add(request.id);
    try {
      validateFolderLimits(request.limits);
      if (request.kind !== 'folderImport' || request.scope !== 'topLevel' ||
        request.copyLayout !== 'sourceBasenameChild' ||
        !['file.scripts.loadFilesIntoStack', 'file.automate.contactSheetII', 'file.scripts.statistics', 'file.scripts.imageProcessor', 'file.automate.batch', 'file.automate.lensCorrection']
          .includes(request.intent)) {
        throw new Error(shellString("shell_unsupported_folder_input_request", "Unsupported folder input request"));
      }
      if (this.active !== undefined || !this.available()) {
        throw new Error(shellString("shell_another_file_operation_is_in_progress_complete_or_cancel", "Another file operation is in progress. Complete or cancel it before selecting a folder"));
      }
    } catch (error) {
      this.rejected.push(this.emptyCompletion(request, 'error', (error as Error).message));
      this.activity(true);
      this.poll();
      return;
    }
    const job = new FolderImportJob(request);
    this.active = job;
    this.activity(true);
    job.operation = this.execute(job);
  }

  cancel(id: number, target: string): boolean {
    const job = this.active;
    if (job === undefined || job.request.id !== id || job.request.target !== target || job.transferred) { return false; }
    job.cancelled = true;
    job.progress = undefined;
    try { this.service.cancel(id, target); } catch (_) { this.report(shellString("shell_could_not_cancel_folder_copying_immediately_waiting_for_the", "Could not cancel folder copying immediately. Waiting for the task to finish")); }
    if (job.pending !== undefined && !job.cleaning) { this.cancelRetained(job); }
    return true;
  }

  cancelCurrent(): void {
    if (this.active !== undefined) { this.cancel(this.active.request.id, this.active.request.target); }
  }

  async cancelAndWait(): Promise<void> {
    const job = this.active;
    if (job !== undefined) {
      this.cancel(job.request.id, job.request.target);
      await job.operation;
    }
  }

  poll(): void {
    if (this.disposed) { return; }
    while (this.rejected.length > 0) {
      if (!this.enqueue(JSON.stringify(this.rejected[0]))) { return; }
      this.rejected.shift();
    }
    const job = this.active;
    if (job === undefined) {
      this.activity(false);
      return;
    }
    if (job.cleaning) { return; }
    if (job.pending !== undefined) {
      if (job.cancelled) { this.cancelRetained(job); return; }
      // Do not put an owned directory into a generic string queue with no ownership callback.
      if (!this.enqueue(JSON.stringify(job.pending))) { return; }
      job.transferred = true;
      job.progress = undefined;
      this.active = undefined;
      this.activity(this.hasPending());
      return;
    }
    if (job.progress !== undefined && Date.now() - job.lastProgressTime >= 200) {
      if (this.enqueue(JSON.stringify(job.progress))) {
        job.lastProgressTime = Date.now();
        job.progress = undefined;
      }
    }
  }

  dispose(): void {
    if (this.disposed) { return; }
    this.disposed = true;
    this.rejected = [];
    this.cancelCurrent();
    this.activity(false);
  }

  private async execute(job: FolderImportJob): Promise<void> {
    let completion: FolderImportCompletion;
    try {
      const request: FolderImportRequest = { kind: 'folderImport', id: job.request.id, target: job.request.target,
        intent: job.request.intent, limits: job.request.limits, recursive: true,
        copyLayout: job.request.copyLayout };
      completion = await this.service.importFolder(request, (progress: FolderImportProgress) => {
        if (this.active !== job || this.disposed || job.cancelled || job.pending !== undefined ||
          progress.id !== job.request.id || progress.target !== job.request.target) { return; }
        if (!Number.isSafeInteger(progress.processedBytes) || !Number.isSafeInteger(progress.totalBytes) ||
          !Number.isSafeInteger(progress.filesDone) || progress.processedBytes < 0 || progress.totalBytes < 0 ||
          progress.processedBytes > progress.totalBytes || progress.totalBytes > job.request.limits.maxBytes ||
          progress.filesDone < 0 || progress.filesDone > job.request.limits.maxFiles) {
          throw new Error(shellString("shell_invalid_folder_input_progress", "Invalid folder input progress"));
        }
        job.progress = progress;
      });
    } catch (error) {
      completion = this.emptyCompletion(job.request, job.cancelled ? 'cancel' : 'error',
        job.cancelled ? '' : (error as Error).message || shellString("shell_folder_import_failed", "Folder import failed"));
    }
    job.progress = undefined;
    if (completion.id !== job.request.id || completion.target !== job.request.target ||
      completion.copyLayout !== job.request.copyLayout) {
      if (completion.ownerRoot.length > 0) {
        await this.cleanup(job, completion);
      }
      completion = this.emptyCompletion(job.request, 'error', shellString("shell_the_folder_input_result_does_not_match_the_request", "The folder input result does not match the request. The original dialog was not updated"));
    }
    job.pending = completion;
    if (this.disposed || job.cancelled) { await this.cancelRetained(job); }
    else { this.poll(); }
  }

  private async cancelRetained(job: FolderImportJob): Promise<void> {
    if (job.cleaning || job.pending === undefined || job.transferred) { return; }
    job.cleaning = true;
    const completion = job.pending;
    if (completion.ownerRoot.length > 0 && !await this.cleanup(job, completion)) {
      completion.result = 'error';
      completion.error = shellString("shell_import_was_canceled_but_the_local_copy_could_not", "Import was canceled, but the local copy could not be removed. Files have been retained. Try again later");
      job.pending = completion;
    } else {
      job.pending = this.emptyCompletion(job.request, 'cancel', '');
    }
    job.cancelled = false;
    job.cleaning = false;
    if (this.disposed) {
      this.active = undefined;
      return;
    }
    this.poll();
  }

  private async cleanup(job: FolderImportJob, completion: FolderImportCompletion): Promise<boolean> {
    try {
      await this.service.releaseRetained(completion, job.request.id, job.request.target);
      return true;
    } catch (_) {
      this.report(shellString("shell_the_local_imported_copy_could_not_be_removed_files", "The local imported copy could not be removed. Files have been retained"));
      return false;
    }
  }

  private emptyCompletion(request: FolderImportEvent, result: 'cancel' | 'error', error: string): FolderImportCompletion {
    return { kind: 'folderComplete', id: request.id, target: request.target, result: result,
      copyLayout: 'sourceBasenameChild', ownerRoot: '', resolvedRoot: '', manifestPath: '', bindingId: '', error: error };
  }
}
