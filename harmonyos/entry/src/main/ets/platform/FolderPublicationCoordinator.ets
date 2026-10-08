import { shellString } from './ShellLocalization';
import { FolderLimits, validateFolderLimits } from './SandboxFolderManifest';
import { FolderPublicationCompletion, FolderPublicationProgress, FolderPublicationRequest,
  validateFolderPublicationIdentity } from './FolderPublicationTransaction';

export type FolderPublishOperation = (request: FolderPublicationRequest,
  progress: (value: FolderPublicationProgress) => void) => Promise<FolderPublicationCompletion>;

class FolderPublicationJob {
  request: FolderPublicationRequest;
  cancelled: boolean = false;
  pending: FolderPublicationCompletion | undefined = undefined;
  progress: FolderPublicationProgress | undefined = undefined;
  lastProgressTime: number = 0;
  operation: Promise<void> = Promise.resolve();
  constructor(request: FolderPublicationRequest) { this.request = request; }
}

/** Publication borrows an immutable Rust-owned stage; this coordinator never deletes a stage or external URI. */
export class FolderPublicationCoordinator {
  private publish: FolderPublishOperation;
  private cancelCopy: (id: number, target: string) => boolean;
  private enqueue: (serialized: string) => boolean;
  private available: () => boolean;
  private report: (message: string) => void;
  private activity: (active: boolean) => void;
  private active: FolderPublicationJob | undefined = undefined;
  private rejected: FolderPublicationCompletion[] = [];
  private seen: Set<number> = new Set<number>();
  private disposed: boolean = false;

  constructor(publish: FolderPublishOperation, cancelCopy: (id: number, target: string) => boolean,
    enqueue: (serialized: string) => boolean, available: () => boolean, report: (message: string) => void,
    activity: (active: boolean) => void) {
    this.publish = publish;
    this.cancelCopy = cancelCopy;
    this.enqueue = enqueue;
    this.available = available;
    this.report = report;
    this.activity = activity;
  }

  hasPending(): boolean { return this.active !== undefined || this.rejected.length > 0; }

  begin(request: FolderPublicationRequest): void {
    if (this.disposed) { return; }
    try { validateFolderPublicationIdentity(request.id, request.target); } catch (_) {
      this.report(shellString("shell_invalid_folder_output_request_identity", "Invalid folder output request identity"));
      return;
    }
    if (this.seen.has(request.id)) { return; }
    if (this.seen.size >= 4096 || this.rejected.length >= 64) {
      this.report(shellString("shell_the_folder_output_queue_is_full_cancel_and_try", "The folder output queue is full. Cancel and try again"));
      return;
    }
    this.seen.add(request.id);
    try {
      validateFolderLimits(request.limits);
      if (request.kind !== 'folderPublish' || request.policy !== 'freshSubdirectory' ||
        typeof request.ownerRoot !== 'string' || typeof request.stageRoot !== 'string' ||
        request.ownerRoot.length === 0 || request.stageRoot.length === 0) {
        throw new Error(shellString("shell_invalid_folder_output_request", "Invalid folder output request"));
      }
      if (this.active !== undefined || !this.available()) {
        throw new Error(shellString("shell_another_file_operation_is_in_progress_complete_or_cancel_122", "Another file operation is in progress. Complete or cancel it before exporting"));
      }
    } catch (error) {
      this.rejected.push(this.emptyCompletion(request, (error as Error).message));
      this.activity(true);
      this.poll();
      return;
    }
    const job = new FolderPublicationJob(request);
    this.active = job;
    this.activity(true);
    job.operation = this.execute(job);
  }

  cancel(id: number, target: string): boolean {
    const job = this.active;
    if (job === undefined || job.request.id !== id || job.request.target !== target || job.pending !== undefined) {
      // A completed provider copy cannot be undone while its actual receipt waits in a full mailbox.
      return false;
    }
    job.cancelled = true;
    job.progress = undefined;
    try { this.cancelCopy(id, target); } catch (_) {
      this.report(shellString("shell_waiting_for_folder_output_to_finish_keep_the_system", "Waiting for folder output to finish. Keep the system file window open"));
    }
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
    if (job === undefined) { this.activity(false); return; }
    if (job.pending !== undefined) {
      if (!this.enqueue(JSON.stringify(job.pending))) { return; }
      // Only the matching Rust registry may accept a receipt and clean a successful stage.
      this.active = undefined;
      job.progress = undefined;
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
    this.cancelCurrent();
    this.disposed = true;
    this.rejected = [];
    this.activity(false);
    // Even success remains retained if the application ends before Rust accepts the receipt.
  }

  private async execute(job: FolderPublicationJob): Promise<void> {
    let completion: FolderPublicationCompletion;
    try {
      completion = await this.publish(job.request, (value: FolderPublicationProgress) => {
        if (this.active !== job || this.disposed || job.cancelled || job.pending !== undefined ||
          value.id !== job.request.id || value.target !== job.request.target) { return; }
        this.validateProgress(value, job.request.limits);
        job.progress = value;
      });
    } catch (_) {
      // An escaped SDK/helper failure cannot prove that provider mutation did not begin.
      completion = this.emptyCompletion(job.request,
        shellString("shell_the_folder_export_result_could_not_be_confirmed_local", "The folder export result could not be confirmed. Local output has been retained. Inspect the new output folder and export again"));
      completion.partialPossible = true;
    }
    job.progress = undefined;
    if (completion.kind !== 'folderPublishComplete' || completion.id !== job.request.id ||
      completion.target !== job.request.target || completion.retainStage !== true ||
      !['success', 'cancel', 'error'].includes(completion.result) ||
      !Number.isSafeInteger(completion.publishedCount) || completion.publishedCount < 0 ||
      completion.publishedCount > job.request.limits.maxFiles ||
      (completion.result === 'success' && (completion.publishedCount === 0 || completion.partialPossible))) {
      completion = this.emptyCompletion(job.request,
        shellString("shell_invalid_folder_output_receipt_local_output_has_been_retained", "Invalid folder output receipt. Local output has been retained. Inspect the output folder and retry"));
      completion.partialPossible = true;
    }
    if (this.active !== job) { return; }
    job.pending = completion;
    if (this.disposed) { this.active = undefined; return; }
    this.poll();
  }

  private validateProgress(progress: FolderPublicationProgress, limits: FolderLimits): void {
    if (progress.kind !== 'folderPublishProgress' || !Number.isSafeInteger(progress.processedBytes) ||
      !Number.isSafeInteger(progress.totalBytes) || !Number.isSafeInteger(progress.filesDone) ||
      progress.processedBytes < 0 || progress.totalBytes < 0 || progress.processedBytes > progress.totalBytes ||
      progress.totalBytes > limits.maxBytes || progress.filesDone < 0 || progress.filesDone > limits.maxFiles) {
      throw new Error(shellString("shell_invalid_folder_output_progress", "Invalid folder output progress"));
    }
  }

  private emptyCompletion(request: FolderPublicationRequest, error: string): FolderPublicationCompletion {
    return { kind: 'folderPublishComplete', id: request.id, target: request.target, result: 'error',
      ownerRoot: '', stageRoot: '', journalPath: '', publicationName: '', publishedCount: 0,
      partialPossible: false, retainStage: true, error: error };
  }
}
