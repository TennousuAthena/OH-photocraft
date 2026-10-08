import { shellString } from './ShellLocalization';
import { window } from '@kit.ArkUI';
import { BusinessError } from '@kit.BasicServicesKit';

export interface ViewportRequest {
  command: string;
  enabled?: boolean;
}

export interface ViewportState {
  fullscreen: boolean;
  maximized: boolean;
}

/** Main-window commands only. A viewport request never authorizes closing the editor. */
export class ViewportBridge {
  private mainWindow: window.Window;
  private emitState: (state: ViewportState) => void;
  private reportError: (message: string) => void;
  private beforeFullscreen: window.WindowStatusType | undefined = undefined;
  private lastState: string = '';
  private subscribed: boolean = false;
  private disposed: boolean = false;
  private statusHandler: (status: window.WindowStatusType) => void = (status: window.WindowStatusType) => {
    if (!this.disposed) {
      this.publishState(status);
    }
  };

  constructor(mainWindow: window.Window, emitState: (state: ViewportState) => void,
    reportError: (message: string) => void) {
    this.mainWindow = mainWindow;
    this.emitState = emitState;
    this.reportError = reportError;
  }

  initialize(): void {
    // API20 emits the final geometry/status once, unlike the older transition callback.
    this.mainWindow.on('windowStatusDidChange', this.statusHandler);
    this.subscribed = true;
    this.publishActualState();
  }

  async apply(request: ViewportRequest): Promise<void> {
    if (this.disposed) {
      throw new Error(shellString("shell_the_window_service_is_closed", "The window service is closed"));
    }
    try {
      if (request.command === 'fullscreen') {
        if (typeof request.enabled !== 'boolean') {
          throw new Error(shellString("shell_the_fullscreen_request_is_missing_its_state", "The fullscreen request is missing its state"));
        }
        const status = this.mainWindow.getWindowStatus();
        if (request.enabled) {
          if (status !== window.WindowStatusType.FULL_SCREEN) {
            this.beforeFullscreen = status;
            await this.mainWindow.maximize(window.MaximizePresentation.ENTER_IMMERSIVE);
          }
        } else {
          if (status === window.WindowStatusType.FULL_SCREEN) {
            if (this.beforeFullscreen === window.WindowStatusType.MAXIMIZE) {
              await this.mainWindow.maximize(window.MaximizePresentation.EXIT_IMMERSIVE);
            } else {
              await this.mainWindow.recover();
            }
          }
          this.beforeFullscreen = undefined;
        }
      } else if (request.command === 'maximized') {
        if (typeof request.enabled !== 'boolean') {
          throw new Error(shellString("shell_the_maximize_request_is_missing_its_state", "The maximize request is missing its state"));
        }
        const status = this.mainWindow.getWindowStatus();
        if (request.enabled && status !== window.WindowStatusType.MAXIMIZE) {
          await this.mainWindow.maximize(window.MaximizePresentation.EXIT_IMMERSIVE);
        } else if (!request.enabled && status !== window.WindowStatusType.FLOATING) {
          await this.mainWindow.recover();
        }
        this.beforeFullscreen = undefined;
      } else if (request.command === 'startDrag') {
        // API14 expects an active Down/Move gesture. A delayed mailbox request needs device verification.
        // Its promise acknowledges the system request; it does not claim a completed move.
        await this.mainWindow.startMoving();
      } else {
        throw new Error(shellString("shell_unsupported_window_request_detail", "Unsupported window request: {0}", request.command));
      }
    } finally {
      // Always report what the OS actually did, including a failed/partially applied command.
      this.publishActualState();
    }
  }

  dispose(): void {
    this.disposed = true;
    if (this.subscribed) {
      this.mainWindow.off('windowStatusDidChange', this.statusHandler);
      this.subscribed = false;
    }
  }

  private publishActualState(): void {
    try {
      this.publishState(this.mainWindow.getWindowStatus());
    } catch (error) {
      this.reportError(shellString("shell_could_not_read_window_state_detail", "Could not read window state: {0}", (error as BusinessError).message));
    }
  }

  private publishState(status: window.WindowStatusType): void {
    const state: ViewportState = { fullscreen: status === window.WindowStatusType.FULL_SCREEN,
      maximized: status === window.WindowStatusType.MAXIMIZE };
    const key = `${state.fullscreen}/${state.maximized}`;
    if (key !== this.lastState) {
      this.emitState(state);
      this.lastState = key;
    }
  }
}
