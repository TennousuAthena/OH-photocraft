import { shellString } from './ShellLocalization';
import { AppStorageRoots } from './AppStorageRoots';
import { common } from '@kit.AbilityKit';
import { BusinessError, print } from '@kit.BasicServicesKit';
import { fileIo, fileUri } from '@kit.CoreFileKit';

export type PrintingState = 'submitted' | 'blocked' | 'succeeded' | 'failed' |
  'cancelled' | 'rejected' | 'monitoringError';

export interface PrintingUpdate {
  id: number;
  state: PrintingState;
  terminal: boolean;
  error: string;
}

class TrackedPrintTask {
  id: number;
  task: print.PrintTask | undefined = undefined;
  blockAttached: boolean = false;
  successAttached: boolean = false;
  failureAttached: boolean = false;
  cancelAttached: boolean = false;
  onBlock: () => void = () => {};
  onSuccess: () => void = () => {};
  onFailure: () => void = () => {};
  onCancel: () => void = () => {};

  constructor(id: number) {
    this.id = id;
  }
}

/**
 * UI-thread printing of an already rendered sandbox PDF. Requires the normal
 * ohos.permission.PRINT permission. PlatformBridge forwards typed task updates to Rust.
 *
 * The caller owns the PDF and must retain it while the task is outstanding.
 * Resolving submitPdf(true) means submission was accepted, never print success.
 * Only SDK succeed/fail/cancel events produce a terminal task result. A blocked
 * task can resume. Missing callbacks or a monitoring error remain indeterminate.
 */
export class PrintingBridge {
  private context: common.UIAbilityContext;
  private roots: AppStorageRoots;
  private observer: (update: PrintingUpdate) => void;
  private tasks: Map<number, TrackedPrintTask> = new Map();
  private disposed: boolean = false;

  constructor(context: common.UIAbilityContext, observer: (update: PrintingUpdate) => void,
    roots: AppStorageRoots = new AppStorageRoots(context)) {
    this.context = context;
    this.roots = roots;
    this.observer = observer;
  }

  async submitPdf(id: number, pdfPath: string): Promise<boolean> {
    // IDs must be unique while live. Reject duplicate delivery without reporting
    // a terminal result for the already submitted task using the same ID.
    if (!Number.isSafeInteger(id) || id < 0 || this.disposed || this.tasks.has(id)) {
      return false;
    }
    if (this.tasks.size >= 8) {
      this.notify(id, 'rejected', true, shellString("shell_too_many_print_jobs_wait_for_existing_jobs_to", "Too many print jobs. Wait for existing jobs to finish"));
      return false;
    }
    const record = new TrackedPrintTask(id);
    this.tasks.set(id, record);
    try {
      if (!canIUse('SystemCapability.Print.PrintFramework')) {
        throw new Error(shellString("shell_system_printing_is_unavailable_on_this_device", "System printing is unavailable on this device"));
      }
      if (!this.isSandboxPdf(pdfPath)) {
        throw new Error(shellString("shell_the_print_file_must_be_a_pdf_in_the", "The print file must be a PDF in the app sandbox"));
      }
      const info = await fileIo.lstat(pdfPath);
      if (!info.isFile() || info.isSymbolicLink() || info.size === 0) {
        throw new Error(shellString("shell_the_print_pdf_file_is_unavailable", "The print PDF file is unavailable"));
      }
      if (!this.isLive(record)) {
        return false;
      }
      const task = await print.print([fileUri.getUriFromPath(pdfPath)], this.context);
      // Disposal during the system call cannot cancel a task already submitted.
      if (!this.isLive(record)) {
        return true;
      }
      record.task = task;
    } catch (error) {
      if (this.isLive(record)) {
        this.tasks.delete(id);
        this.notify(id, 'rejected', true, this.errorMessage(error as BusinessError));
      }
      return false;
    }

    record.onBlock = () => { this.taskEvent(record, 'blocked', false); };
    record.onSuccess = () => { this.taskEvent(record, 'succeeded', true); };
    record.onFailure = () => { this.taskEvent(record, 'failed', true); };
    record.onCancel = () => { this.taskEvent(record, 'cancelled', true); };
    // The SDK has separate literal overloads for each event. Keep stable callback
    // references so disposal only removes this bridge's own subscriptions.
    try {
      const task = record.task;
      if (task !== undefined) {
        task.on('succeed', record.onSuccess);
        record.successAttached = true;
        task.on('fail', record.onFailure);
        record.failureAttached = true;
        task.on('cancel', record.onCancel);
        record.cancelAttached = true;
        task.on('block', record.onBlock);
        record.blockAttached = true;
      }
    } catch (error) {
      // The task exists even if listener registration fails. Keep any successful
      // subscriptions and never convert this failure into a guessed print result.
      if (this.isLive(record)) {
        this.notify(id, 'monitoringError', false, this.errorMessage(error as BusinessError));
      }
      return true;
    }
    if (this.isLive(record)) {
      this.notify(id, 'submitted', false, '');
    }
    return true;
  }

  /** Detaches listeners only. The public PrintTask API has no cancel operation. */
  dispose(): void {
    this.disposed = true;
    for (const record of this.tasks.values()) {
      this.detach(record);
    }
    this.tasks.clear();
  }

  private isLive(record: TrackedPrintTask): boolean {
    return !this.disposed && this.tasks.get(record.id) === record;
  }

  private taskEvent(record: TrackedPrintTask, state: PrintingState, terminal: boolean): void {
    if (!this.isLive(record)) {
      return;
    }
    if (terminal) {
      this.tasks.delete(record.id);
      this.detach(record);
    }
    this.notify(record.id, state, terminal, '');
  }

  private detach(record: TrackedPrintTask): void {
    const task = record.task;
    if (task === undefined) {
      return;
    }
    // Each unsubscribe is independent; one SDK failure must not skip the others.
    try { if (record.blockAttached) { task.off('block', record.onBlock); } } catch (_) {}
    try { if (record.successAttached) { task.off('succeed', record.onSuccess); } } catch (_) {}
    try { if (record.failureAttached) { task.off('fail', record.onFailure); } } catch (_) {}
    try { if (record.cancelAttached) { task.off('cancel', record.onCancel); } } catch (_) {}
    record.task = undefined;
  }

  private notify(id: number, state: PrintingState, terminal: boolean, error: string): void {
    if (this.disposed) {
      return;
    }
    try {
      this.observer({ id: id, state: state, terminal: terminal, error: error });
    } catch (_) {
      // Application callbacks must not escape into PrintKit event dispatch.
    }
  }

  private isSandboxPdf(path: string): boolean {
    if (!path.toLowerCase().endsWith('.pdf') || path.includes('\0') ||
      path.split('/').some((part: string) => part === '..' || part === '.')) {
      return false;
    }
    return path.startsWith(this.roots.cacheDir + '/') || path.startsWith(this.roots.filesDir + '/');
  }

  private errorMessage(error: BusinessError): string {
    return error.message || shellString("shell_system_print_request_failed", "System print request failed");
  }
}
