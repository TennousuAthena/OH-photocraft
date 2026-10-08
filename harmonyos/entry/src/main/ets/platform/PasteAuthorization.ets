import { shellString } from './ShellLocalization';
import { ClipboardContent } from './ClipboardBridge';
import { BusinessError } from '@kit.BasicServicesKit';

export class ClipboardReadCancelled extends Error {
  constructor() { super(shellString("shell_paste_canceled", "Paste canceled")); }
}

/** One explicit paste request. Cancellation never publishes a late clipboard read. */
export class PasteAccessRequest {
  public completion: Promise<ClipboardContent>;
  public settled: Promise<void>;
  public cleanupError: Error | undefined = undefined;
  private active: boolean = true;
  private reading: boolean = false;
  private resolve: (content: ClipboardContent) => void = () => {};
  private reject: (error: Error) => void = () => {};
  private resolveSettled: () => void = () => {};
  private read: () => Promise<ClipboardContent>;
  private discard: (content: ClipboardContent) => Promise<void>;

  constructor(read: () => Promise<ClipboardContent>, discard: (content: ClipboardContent) => Promise<void>) {
    this.read = read;
    this.discard = discard;
    this.completion = new Promise<ClipboardContent>((resolve, reject) => {
      this.resolve = resolve;
      this.reject = reject;
    });
    this.settled = new Promise<void>((resolve) => { this.resolveSettled = resolve; });
  }

  async approve(): Promise<void> {
    await this.readOnce(false);
  }

  /** A physical Ctrl+V can already carry a short-lived OS grant. Only permission denial needs the button. */
  async tryDirect(): Promise<boolean> {
    return await this.readOnce(true);
  }

  private async readOnce(allowAuthorization: boolean): Promise<boolean> {
    if (!this.active || this.reading) {
      return true;
    }
    this.reading = true;
    let completed = true;
    try {
      const content = await this.read();
      if (this.active) {
        this.active = false;
        this.resolve(content);
      } else {
        try {
          await this.discard(content);
        } catch (error) {
          this.cleanupError = error as Error;
        }
      }
    } catch (error) {
      if (this.active && allowAuthorization && (error as BusinessError).code === 201) {
        // Keep the same request/owner pending, without rereading until the user grants access.
        completed = false;
      } else if (this.active) {
        this.fail(error as Error);
      } else {
        this.cleanupError = error as Error;
      }
    } finally {
      this.reading = false;
      this.finishIfIdle();
    }
    return completed;
  }

  fail(error: Error): void {
    if (this.active) {
      this.active = false;
      this.reject(error);
      this.finishIfIdle();
    }
  }

  cancel(): void {
    this.fail(new ClipboardReadCancelled() as Error);
  }

  private finishIfIdle(): void {
    if (!this.active && !this.reading) {
      this.resolveSettled();
    }
  }
}
