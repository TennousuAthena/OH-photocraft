import { shellString } from './ShellLocalization';
/** An instance identity, never a native mailbox ID or a durable success receipt. */
export class FileTransactionToken {
  readonly sequence: number;

  constructor(sequence: number) {
    this.sequence = sequence;
  }
}

/** Keeps an authorized close behind provider work and its binding/fsync finally block. */
export class FileTransactionDrain {
  private active: Set<FileTransactionToken> = new Set<FileTransactionToken>();
  private sequence: number = 0;
  private accepting: boolean = true;
  private disposed: boolean = false;
  private pending: Promise<void> | undefined = undefined;
  private resolveClose: (() => void) | undefined = undefined;
  private rejectClose: ((reason: Error) => void) | undefined = undefined;

  /** Acquire before consuming a mailbox request; a closed gate must leave that request queued. */
  tryBegin(): FileTransactionToken | undefined {
    if (!this.accepting || this.disposed || this.active.size >= 64 ||
      this.sequence >= Number.MAX_SAFE_INTEGER) {
      return undefined;
    }
    const token = new FileTransactionToken(++this.sequence);
    this.active.add(token);
    return token;
  }

  /** Only the original transaction's finally block can release its instance token. */
  end(token: FileTransactionToken): boolean {
    if (!this.active.delete(token)) {
      return false;
    }
    if (this.active.size === 0 && this.resolveClose !== undefined) {
      const resolve = this.resolveClose;
      this.resolveClose = undefined;
      this.rejectClose = undefined;
      resolve();
    }
    return true;
  }

  /** Closing the admission gate is synchronous; every waiter shares one bounded promise. */
  waitForIdle(): Promise<void> {
    this.accepting = false;
    if (this.disposed) {
      return Promise.reject(new Error(shellString("shell_file_transaction_service_destroyed", "The file transaction service has been destroyed")));
    }
    if (this.active.size === 0) {
      return Promise.resolve();
    }
    if (this.pending === undefined) {
      this.pending = new Promise<void>((resolve, reject) => {
        this.resolveClose = resolve;
        this.rejectClose = reject;
      });
    }
    return this.pending;
  }

  /** A failed shutdown does not permanently disable the editor's file service. */
  resumeAfterFailedClose(): void {
    if (this.disposed) {
      return;
    }
    if (this.rejectClose !== undefined) {
      this.rejectClose(new Error(shellString("shell_file_transaction_close_canceled", "File transaction closure was canceled")));
    }
    this.pending = undefined;
    this.resolveClose = undefined;
    this.rejectClose = undefined;
    this.accepting = true;
  }

  /** Destroyed UI state cannot satisfy an outstanding close with a false idle result. */
  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.accepting = false;
    if (this.rejectClose !== undefined) {
      this.rejectClose(new Error(shellString("shell_file_transaction_service_destroyed", "The file transaction service has been destroyed")));
    }
    this.pending = undefined;
    this.resolveClose = undefined;
    this.rejectClose = undefined;
  }
}
