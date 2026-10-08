import { shellString } from './ShellLocalization';
import { FolderDestinationCompletion, FolderDestinationRequest, FolderOutputIntent, isFolderDestinationHandle, isFolderOutputIntent } from './FolderDestinationBridge';
import { validateFolderPublicationIdentity } from './FolderPublicationTransaction';

export interface FolderStageRequest {
  kind: 'folderStageAllocate';
  id: number;
  target: string;
  intent: FolderOutputIntent;
  destinationHandle: string;
}

export interface FolderStageCompletion {
  kind: 'folderStageComplete';
  id: number;
  target: string;
  result: 'success' | 'cancel' | 'error';
  destinationHandle: string;
  ownerRoot: string;
  stageRoot: string;
  journalPath: string;
  error: string;
}

type FolderOutputRequest = FolderDestinationRequest | FolderStageRequest;
type FolderOutputCompletion = FolderDestinationCompletion | FolderStageCompletion;

class FolderOutputJob {
  request: FolderOutputRequest;
  cancelled: boolean = false;
  cleaning: boolean = false;
  pending: FolderOutputCompletion | undefined = undefined;
  operation: Promise<void> = Promise.resolve();
  constructor(request: FolderOutputRequest) { this.request = request; }
}

/** Short authorization/allocation jobs have the same bounded queue and identity rules as long copy jobs. */
export class FolderOutputCoordinator {
  private select: (request: FolderDestinationRequest) => Promise<FolderDestinationCompletion>;
  private allocate: (request: FolderStageRequest) => Promise<FolderStageCompletion>;
  private cancelPicker: (id: number, target: string) => boolean;
  private releaseHandle: (handle: string, target: string) => Promise<void>;
  private enqueue: (serialized: string) => boolean;
  private available: () => boolean;
  private report: (message: string) => void;
  private activity: (active: boolean) => void;
  private active: FolderOutputJob | undefined = undefined;
  private rejected: FolderOutputCompletion[] = [];
  private seen: Set<number> = new Set<number>();
  private disposed: boolean = false;

  constructor(select: (request: FolderDestinationRequest) => Promise<FolderDestinationCompletion>,
    allocate: (request: FolderStageRequest) => Promise<FolderStageCompletion>,
    cancelPicker: (id: number, target: string) => boolean,
    releaseHandle: (handle: string, target: string) => Promise<void>, enqueue: (serialized: string) => boolean,
    available: () => boolean, report: (message: string) => void, activity: (active: boolean) => void) {
    this.select = select;
    this.allocate = allocate;
    this.cancelPicker = cancelPicker;
    this.releaseHandle = releaseHandle;
    this.enqueue = enqueue;
    this.available = available;
    this.report = report;
    this.activity = activity;
  }

  hasPending(): boolean { return this.active !== undefined || this.rejected.length > 0; }

  begin(request: FolderOutputRequest): void {
    if (this.disposed) { return; }
    try { validateFolderPublicationIdentity(request.id, request.target); } catch (_) {
      this.report(shellString("shell_invalid_output_folder_request_identity", "Invalid output folder request identity")); return;
    }
    if (this.seen.has(request.id)) { return; }
    if (this.seen.size >= 4096 || this.rejected.length >= 64) {
      this.report(shellString("shell_too_many_output_folder_requests_cancel_and_try_again", "Too many output folder requests. Cancel and try again")); return;
    }
    this.seen.add(request.id);
    try {
      if (!isFolderOutputIntent(request.intent) ||
        !['folderDestination', 'folderStageAllocate'].includes(request.kind) ||
        (request.kind === 'folderStageAllocate' &&
          !isFolderDestinationHandle((request as FolderStageRequest).destinationHandle))) {
        throw new Error(shellString("shell_invalid_output_folder_request_format", "Invalid output folder request format"));
      }
      if (this.active !== undefined || !this.available()) { throw new Error(shellString("shell_another_file_operation_is_in_progress_complete_or_cancel_91", "Another file operation is in progress. Complete or cancel it and try again")); }
    } catch (error) {
      this.rejected.push(this.empty(request, 'error', (error as Error).message));
      this.activity(true); this.poll(); return;
    }
    const job = new FolderOutputJob(request);
    this.active = job;
    this.activity(true);
    job.operation = this.execute(job);
  }

  cancel(kind: 'folderDestination' | 'folderStageAllocate', id: number, target: string): boolean {
    const job = this.active;
    if (job === undefined || job.request.kind !== kind || job.request.id !== id || job.request.target !== target) {
      return false;
    }
    job.cancelled = true;
    if (kind === 'folderDestination') {
      try { this.cancelPicker(id, target); } catch (_) { this.report(shellString("shell_waiting_for_system_folder_selection_to_finish", "Waiting for system folder selection to finish")); }
    }
    if (job.pending !== undefined && !job.cleaning) { job.operation = this.cancelRetained(job); }
    return true;
  }

  cancelCurrent(): void {
    if (this.active !== undefined) {
      this.cancel(this.active.request.kind, this.active.request.id, this.active.request.target);
    }
  }

  async cancelAndWait(): Promise<void> {
    const job = this.active;
    if (job !== undefined) { this.cancelCurrent(); await job.operation; }
  }

  poll(): void {
    if (this.disposed) { return; }
    while (this.rejected.length > 0) {
      if (!this.enqueue(JSON.stringify(this.rejected[0]))) { return; }
      this.rejected.shift();
    }
    const job = this.active;
    if (job === undefined) { this.activity(false); return; }
    if (job.cleaning || job.pending === undefined) { return; }
    if (job.cancelled) { job.operation = this.cancelRetained(job); return; }
    if (!this.enqueue(JSON.stringify(job.pending))) { return; }
    this.active = undefined;
    // true transfers the handle or stage; no late shell cancellation may release it afterward.
    this.activity(this.hasPending());
  }

  dispose(): void {
    if (this.disposed) { return; }
    this.cancelCurrent(); this.disposed = true; this.rejected = []; this.activity(false);
  }

  private async execute(job: FolderOutputJob): Promise<void> {
    let completion: FolderOutputCompletion;
    try {
      completion = job.request.kind === 'folderDestination' ?
        await this.select(job.request as FolderDestinationRequest) : await this.allocate(job.request as FolderStageRequest);
    } catch (_) {
      completion = this.empty(job.request, 'error', shellString("shell_the_output_folder_operation_is_incomplete_select_the_folder", "The output folder operation is incomplete. Select the folder again and retry"));
    }
    const expected = job.request.kind === 'folderDestination' ? 'folderDestinationComplete' : 'folderStageComplete';
    if (completion.kind !== expected || completion.id !== job.request.id || completion.target !== job.request.target ||
      !['success', 'cancel', 'error'].includes(completion.result) ||
      (completion.result === 'success' && !isFolderDestinationHandle(completion.destinationHandle)) ||
      (completion.kind === 'folderStageComplete' &&
        completion.destinationHandle !== (job.request as FolderStageRequest).destinationHandle)) {
      completion = this.empty(job.request, 'error', shellString("shell_invalid_output_folder_result_identity_select_the_folder_again", "Invalid output folder result identity. Select the folder again"));
    }
    job.pending = completion;
    if (this.disposed || job.cancelled) { await this.cancelRetained(job); }
    else { this.poll(); }
  }

  private async cancelRetained(job: FolderOutputJob): Promise<void> {
    if (job.cleaning || job.pending === undefined) { return; }
    job.cleaning = true;
    const pending = job.pending;
    if (pending.kind === 'folderDestinationComplete' && pending.destinationHandle.length > 0) {
      try {
        await this.releaseHandle(pending.destinationHandle, job.request.target);
        job.pending = this.empty(job.request, 'cancel', '');
      } catch (_) {
        pending.result = 'error';
        pending.error = shellString("shell_folder_selection_was_canceled_but_the_local_permission_record", "Folder selection was canceled, but the local permission record was retained. Select the folder again and retry");
      }
    } else {
      // A newly allocated empty stage is transferred with its cancel receipt for Rust owner validation/cleanup.
      pending.result = 'cancel'; pending.error = '';
    }
    job.cancelled = false; job.cleaning = false;
    if (this.disposed) { this.active = undefined; return; }
    this.poll();
  }

  private empty(request: FolderOutputRequest, result: 'success' | 'cancel' | 'error', error: string): FolderOutputCompletion {
    if (request.kind === 'folderDestination') {
      return { kind: 'folderDestinationComplete', id: request.id, target: request.target, result: result,
        destinationHandle: '', error: error };
    }
    return { kind: 'folderStageComplete', id: request.id, target: request.target, result: result,
      destinationHandle: (request as FolderStageRequest).destinationHandle || '', ownerRoot: '', stageRoot: '',
      journalPath: '', error: error };
  }
}
