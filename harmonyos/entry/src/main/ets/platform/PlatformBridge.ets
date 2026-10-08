import { shellString } from './ShellLocalization';
import { AppStorageRoots } from './AppStorageRoots';
import { common } from '@kit.AbilityKit';
import { UIContext, window } from '@kit.ArkUI';
import { util } from '@kit.ArkTS';
import { BusinessError } from '@kit.BasicServicesKit';
import nativePhotoCraft from 'libphotocraft.so';
import { ClipboardBridge, ClipboardContent } from './ClipboardBridge';
import { ClipboardReadCancelled } from './PasteAuthorization';
import { ViewportBridge, ViewportRequest, ViewportState } from './ViewportBridge';
import { PrintingBridge, PrintingUpdate } from './PrintingBridge';
import { FileTransactionDrain } from './FileTransactionDrain';
import { FolderLimits } from './SandboxFolderManifest';
import { FolderImportBridge } from './FolderImportBridge';
import { FolderImportCoordinator, FolderImportEvent } from './FolderImportCoordinator';
import { FolderDestinationBridge, FolderDestinationRequest } from './FolderDestinationBridge';
import { FolderOutputCoordinator, FolderStageRequest } from './FolderOutputCoordinator';
import { FolderStageBridge } from './FolderStageBridge';
import { FolderPublicationBridge } from './FolderPublicationBridge';
import { FolderPublicationCoordinator } from './FolderPublicationCoordinator';
import { FolderPublicationRequest, FolderPublicationProgress } from './FolderPublicationTransaction';

interface PlatformEvent {
  id: number;
  kind: string;
  text?: string;
  path?: string;
  url?: string;
  textOnly?: boolean;
  command?: string;
  enabled?: boolean;
  pdfPath?: string;
  target?: string;
  intent?: string;
  scope?: string;
  copyLayout?: string;
  limits?: FolderLimits;
  destinationHandle?: string;
  ownerRoot?: string;
  stageRoot?: string;
  policy?: string;
}

interface PlatformOutput {
  events: PlatformEvent[];
  closeState: CloseState | null;
}

interface CloseState {
  id: number;
  pending: boolean;
}

interface PlatformCompletion {
  kind: string;
  id: number;
  result: string;
  text: string;
  imagePath: string;
  error: string;
}

/** UI-thread IME dispatch and asynchronous system operations, independent of rendering. */
export class PlatformBridge {
  private context: common.UIAbilityContext;
  private uiContext: UIContext;
  private clipboard: ClipboardBridge;
  private printing: PrintingBridge;
  private folders: FolderImportCoordinator;
  private destinations: FolderDestinationBridge;
  private outputFolders: FolderOutputCoordinator;
  private publications: FolderPublicationCoordinator;
  private folderPollTimer: number = -1;
  private reportError: (message: string) => void;
  private restoreFocus: () => void;
  private authorizePaste: (read: () => Promise<ClipboardContent>,
    discard: (content: ClipboardContent) => Promise<void>) => Promise<ClipboardContent>;
  private cancelPaste: () => void;
  private mainWindow: window.Window | undefined = undefined;
  private viewportBridge: ViewportBridge | undefined = undefined;
  private events: PlatformEvent[] = [];
  private completions: string[] = [];
  private running: boolean = false;
  private fileTransactions: FileTransactionDrain;
  private closing: boolean = false;
  private pendingClose: boolean = false;
  private disposed: boolean = false;
  private pageVisible: boolean = true;
  private geometryError: boolean = false;
  private closePollTimer: number = -1;
  private closeRequestId: number = -1;
  private closeSequence: number = 0;
  private closeHandler: () => Promise<boolean> = async (): Promise<boolean> => {
    this.cancelPaste();
    if (this.closeRequestId >= 0) {
      return true;
    }
    this.closeRequestId = Date.now() + this.closeSequence++;
    // An explicit title-bar close may target a visible background window.
    // Keep polling until this request is acknowledged as cancelled or safely closed.
    this.closePollTimer = setInterval(() => { this.pollClosingRequest(); }, 200);
    try {
      this.sendInput(JSON.stringify({ kind: 'closeRequested', id: this.closeRequestId }));
      this.pollClosingRequest();
    } catch (error) {
      this.reportError(shellString("shell_could_not_request_editor_closure_detail", "Could not request editor closure: {0}", (error as BusinessError).message));
    }
    return true;
  };

  constructor(context: common.UIAbilityContext, uiContext: UIContext,
    reportError: (message: string) => void, restoreFocus: () => void,
    authorizePaste: (read: () => Promise<ClipboardContent>,
      discard: (content: ClipboardContent) => Promise<void>) => Promise<ClipboardContent>,
    cancelPaste: () => void, fileOperationAvailable: () => boolean,
    fileTransactions: FileTransactionDrain = new FileTransactionDrain(),
    roots: AppStorageRoots = new AppStorageRoots(context)) {
    this.context = context;
    this.fileTransactions = fileTransactions;
    this.uiContext = uiContext;
    this.clipboard = new ClipboardBridge(context, roots);
    this.reportError = reportError;
    this.printing = new PrintingBridge(context, (update: PrintingUpdate) => {
      // Submission and blocked/unknown monitoring states keep the immutable PDF owned by Rust.
      // Only the helper's actual task callbacks or submission rejection authorize a terminal update.
      this.sendInput(JSON.stringify({ kind: 'printingUpdate', id: update.id, state: update.state,
        terminal: update.terminal, error: update.error }));
      if (update.error.length > 0) {
        this.reportError(shellString("shell_system_printing_detail", "System printing: {0}", update.error));
      }
    }, roots);
    this.restoreFocus = restoreFocus;
    this.authorizePaste = authorizePaste;
    this.cancelPaste = cancelPaste;
    this.destinations = new FolderDestinationBridge(context, roots);
    const publicationService = new FolderPublicationBridge(context,
      (handle: string, target: string): Promise<string> => this.destinations.resolve(handle, target), roots);
    const stageService = new FolderStageBridge(this.destinations, publicationService);
    this.folders = new FolderImportCoordinator(new FolderImportBridge(context, roots),
      (serialized: string): boolean => nativePhotoCraft.platformInput(serialized),
      () => !this.running && this.events.length === 0 && !this.outputFolders.hasPending() &&
        !this.publications.hasPending() && fileOperationAvailable(),
      (message: string) => { this.reportError(message); }, (_: boolean) => { this.updateFolderPolling(); });
    this.outputFolders = new FolderOutputCoordinator(
      (request: FolderDestinationRequest) => this.destinations.select(request),
      (request: FolderStageRequest) => stageService.allocate(request),
      (id: number, target: string): boolean => this.destinations.cancel(id, target),
      (handle: string, target: string): Promise<void> => this.destinations.release(handle, target),
      (serialized: string): boolean => nativePhotoCraft.platformInput(serialized),
      () => !this.running && this.events.length === 0 && !this.folders.hasPending() &&
        !this.publications.hasPending() && fileOperationAvailable(),
      (message: string) => { this.reportError(message); }, (_: boolean) => { this.updateFolderPolling(); });
    this.publications = new FolderPublicationCoordinator(
      (request: FolderPublicationRequest, progress: (value: FolderPublicationProgress) => void) =>
        publicationService.publishFolder(request, progress),
      (id: number, target: string): boolean => publicationService.cancel(id, target),
      (serialized: string): boolean => nativePhotoCraft.platformInput(serialized),
      () => !this.running && this.events.length === 0 && !this.folders.hasPending() &&
        !this.outputFolders.hasPending() && fileOperationAvailable(),
      (message: string) => { this.reportError(message); }, (_: boolean) => { this.updateFolderPolling(); });
  }

  async initialize(): Promise<void> {
    this.mainWindow = await window.getLastWindow(this.context);
    // true defers the OS title-bar close until the editor's Save/Discard/Cancel resolves.
    this.mainWindow.on('windowWillClose', this.closeHandler);
    this.viewportBridge = new ViewportBridge(this.mainWindow, (state: ViewportState) => {
      this.sendInput(JSON.stringify({ kind: 'viewportState', fullscreen: state.fullscreen, maximized: state.maximized }));
    }, (message: string) => { this.reportError(message); });
    try {
      this.viewportBridge.initialize();
    } catch (error) {
      this.reportError(shellString("shell_could_not_monitor_window_state_detail", "Could not monitor window state: {0}", (error as BusinessError).message));
    }
  }

  poll(): void {
    if (this.disposed || this.closing || this.mainWindow === undefined) {
      return;
    }
    this.flushInputs();
    this.folders.poll();
    this.outputFolders.poll();
    this.publications.poll();
    // Keep the ArkTS work queue bounded while an OS dialog/clipboard call is outstanding.
    if ((this.running || this.events.length > 0 || this.completions.length > 0) && !this.hasFolderWork()) {
      return;
    }
    const serialized = nativePhotoCraft.takePlatformOutput();
    if (serialized.length === 0) {
      return;
    }
    const output = JSON.parse(serialized) as PlatformOutput;
    if (output.closeState !== null && output.closeState !== undefined &&
      output.closeState.id === this.closeRequestId && !output.closeState.pending) {
      this.stopClosingPoll();
    }
    for (const event of output.events) {
      if (!Number.isSafeInteger(event.id) || event.id < 0) {
        this.reportError(shellString("shell_invalid_platform_request_number", "Invalid platform request number"));
        continue;
      }
      if (event.kind === 'close') {
        this.pendingClose = true;
      } else if (event.kind === 'folderCancel') {
        if (typeof event.target === 'string') { this.folders.cancel(event.id, event.target); }
      } else if (event.kind === 'folderImport') {
        this.beginFolderImport(event);
      } else if (event.kind === 'folderDestination' || event.kind === 'folderStageAllocate') {
        this.beginFolderOutput(event);
      } else if (event.kind === 'folderDestinationCancel' || event.kind === 'folderStageCancel') {
        if (typeof event.target === 'string') {
          this.outputFolders.cancel(event.kind === 'folderDestinationCancel' ? 'folderDestination' : 'folderStageAllocate',
            event.id, event.target);
        }
      } else if (event.kind === 'folderPublishCancel') {
        if (typeof event.target === 'string') { this.publications.cancel(event.id, event.target); }
      } else if (event.kind === 'folderPublish') {
        this.beginFolderPublish(event);
      } else if (event.kind === 'folderDestinationRelease') {
        this.releaseDestination(event);
      } else {
        this.events.push(event);
      }
    }
    this.processEvents();
    if (this.pendingClose) {
      // Safe close is already authorized by Rust; geometry or IME errors must not swallow it.
      this.closeApplication();
      return;
    }
    if (!this.pageVisible) { return; }
    // ComponentUtils Offset is already px and screen-relative, including window decorations.
    const rectangle = this.uiContext.getComponentUtils().getRectangleById('photocraft-surface');
    const properties = this.mainWindow.getWindowProperties();
    if (rectangle.size.width <= 0 || rectangle.size.height <= 0 ||
      !Number.isFinite(rectangle.screenOffset.x) || !Number.isFinite(rectangle.screenOffset.y)) {
      if (!this.geometryError) {
        this.reportError(shellString("shell_could_not_get_the_editor_screen_position_input_method", "Could not get the editor screen position. Input method suggestions cannot be positioned yet"));
        this.geometryError = true;
      }
      return;
    }
    this.geometryError = false;
    if (nativePhotoCraft.applyPlatformOutput(this.uiContext, serialized,
      rectangle.screenOffset.x, rectangle.screenOffset.y, properties.id) || this.pendingClose) {
      this.closeApplication();
    }
  }

  suspend(): void {
    nativePhotoCraft.releasePlatform();
  }

  hasFolderWork(): boolean {
    return this.folders.hasPending() || this.outputFolders.hasPending() || this.publications.hasPending();
  }

  setPageVisible(visible: boolean): void {
    this.pageVisible = visible;
    if (!visible) { this.cancelFolderOperations(); }
  }

  cancelFolderOperations(): void {
    this.folders.cancelCurrent(); this.outputFolders.cancelCurrent(); this.publications.cancelCurrent();
  }

  dispose(): void {
    this.fileTransactions.dispose();
    this.disposed = true;
    this.cancelPaste();
    this.folders.dispose();
    this.outputFolders.dispose();
    this.publications.dispose();
    if (this.folderPollTimer !== -1) {
      clearInterval(this.folderPollTimer);
      this.folderPollTimer = -1;
    }
    this.stopClosingPoll();
    try {
      this.suspend();
    } finally {
      try {
        this.printing.dispose();
      } finally {
        try {
          this.viewportBridge?.dispose();
        } finally {
          if (this.mainWindow !== undefined) {
            this.mainWindow.off('windowWillClose', this.closeHandler);
          }
        }
      }
    }
  }

  private pollClosingRequest(): void {
    if (this.closeRequestId < 0) {
      return;
    }
    try {
      this.poll();
    } catch (error) {
      this.reportError(shellString("shell_could_not_handle_the_close_request_detail", "Could not handle the close request: {0}", (error as BusinessError).message));
    }
  }

  private stopClosingPoll(): void {
    if (this.closePollTimer !== -1) {
      clearInterval(this.closePollTimer);
      this.closePollTimer = -1;
    }
    this.closeRequestId = -1;
  }

  private sendInput(serialized: string): void {
    if (this.disposed) {
      return;
    }
    if (this.completions.length === 0 && nativePhotoCraft.platformInput(serialized)) {
      return;
    }
    // A full worker mailbox never blocks the UI; retain the completion for a later poll.
    this.completions.push(serialized);
  }

  private flushInputs(): void {
    while (this.completions.length > 0) {
      if (!nativePhotoCraft.platformInput(this.completions[0])) {
        return;
      }
      this.completions.shift();
    }
  }

  private async processEvents(): Promise<void> {
    if (this.running) {
      return;
    }
    this.running = true;
    try {
      while (this.events.length > 0 && !this.disposed) {
        const event = this.events.shift();
        if (event !== undefined) {
          await this.runEvent(event);
        }
      }
    } finally {
      this.running = false;
      this.restoreFocus();
    }
  }

  private async runEvent(event: PlatformEvent): Promise<void> {
    if (event.kind === 'printRequest') {
      try {
        // The boolean is acceptance only. Duplicate/disposed requests must not end an existing task.
        await this.printing.submitPdf(event.id, event.pdfPath || '');
      } catch (error) {
        // An unexpected observer/bridge failure cannot prove whether a system task exists.
        const message = (error as BusinessError).message || shellString("shell_could_not_confirm_system_print_status_check_the_system", "Could not confirm system print status. Check the system print window");
        this.sendInput(JSON.stringify({ kind: 'printingUpdate', id: event.id, state: 'monitoringError',
          terminal: false, error: message }));
        this.reportError(shellString("shell_system_printing_detail", "System printing: {0}", message));
      }
      return;
    }
    const drain = this.fileTransactions;
    const clipboardWork = event.kind === 'paste' || event.kind === 'copyText' || event.kind === 'copyImage';
    const token = clipboardWork ? drain.tryBegin() : undefined;
    if (clipboardWork && token === undefined) {
      this.sendInput(JSON.stringify({ kind: event.kind === 'paste' ? 'clipboard' : 'platformComplete',
        id: event.id, result: 'cancel', text: '', imagePath: '', error: '' }));
      return;
    }
    let result = 'success';
    let errorMessage = '';
    let content: ClipboardContent = { text: '', imagePath: '' };
    try {
      if (event.kind === 'paste' && this.hasFolderWork()) {
        throw new Error(shellString("shell_a_folder_operation_is_in_progress_complete_or_cancel", "A folder operation is in progress. Complete or cancel it before pasting"));
      }
      if (event.kind === 'paste') {
        // Preserve this request's original editor owner before the OS security panel changes focus.
        this.sendInput(JSON.stringify({ kind: 'clipboardAuthorize', id: event.id }));
        content = await this.authorizePaste(() => this.clipboard.read(event.id, event.textOnly === true),
          async (discarded: ClipboardContent) => {
            if (discarded.imagePath.length > 0) {
              await this.cleanupImage(discarded.imagePath);
            }
          });
      } else if (event.kind === 'copyText') {
        await this.clipboard.writeText(event.text || '');
      } else if (event.kind === 'copyImage') {
        await this.clipboard.writeImage(event.path || '');
      } else if (event.kind === 'openUrl') {
        await this.context.openLink(event.url || '');
      } else if (event.kind === 'viewport') {
        if (this.viewportBridge === undefined) {
          throw new Error(shellString("shell_the_window_service_is_not_ready", "The window service is not ready"));
        }
        const request: ViewportRequest = { command: event.command || '', enabled: event.enabled };
        // StartDrag success acknowledges the SDK request, never a completed physical move.
        await this.viewportBridge.apply(request);
      } else {
        throw new Error(shellString("shell_unsupported_platform_request_detail", "Unsupported platform request: {0}", event.kind));
      }
      if (this.disposed) {
        throw new ClipboardReadCancelled();
      }
      const completion: PlatformCompletion = { kind: event.kind === 'paste' ? 'clipboard' : 'platformComplete',
        id: event.id, result: result, text: content.text, imagePath: content.imagePath, error: '' };
      const serialized = JSON.stringify(completion);
      if (new util.TextEncoder().encodeInto(serialized).byteLength > 1024 * 1024) {
        throw new Error(shellString("shell_clipboard_text_exceeds_the_1_mib_platform_input_limit", "Clipboard text exceeds the 1 MiB platform input limit"));
      }
      if (event.kind === 'paste' && content.imagePath.length > 0) {
        // Only true transfers the PNG to the worker. A rejected enqueue must keep no orphan file.
        if (this.completions.length > 0 || !nativePhotoCraft.platformInput(serialized)) {
          throw new Error(shellString("shell_the_editor_input_queue_is_busy_try_pasting_again", "The editor input queue is busy. Try pasting again"));
        }
      } else {
        this.sendInput(serialized);
      }
    } catch (error) {
      result = error instanceof ClipboardReadCancelled ? 'cancel' : 'error';
      errorMessage = result === 'cancel' ? '' :
        ((error as BusinessError).message || shellString("shell_error_detail", "Error {0}", (error as BusinessError).code));
      if (result === 'error' && !this.disposed) {
        this.reportError(shellString("shell_system_operation_failed_detail", "System operation failed: {0}", errorMessage));
      }
      const completion: PlatformCompletion = { kind: event.kind === 'paste' ? 'clipboard' : 'platformComplete',
        id: event.id, result: result, text: '', imagePath: '', error: errorMessage };
      this.sendInput(JSON.stringify(completion));
      if (content.imagePath.length > 0) {
        await this.cleanupImage(content.imagePath);
      }
    } finally {
      try {
        if (event.kind === 'copyImage' && event.path) {
          await this.cleanupImage(event.path);
        }
      } finally {
        if (token !== undefined) {
          drain.end(token);
        }
      }
    }
  }

  private async cleanupImage(localPath: string): Promise<void> {
    try {
      await this.clipboard.removeCacheFile(localPath);
    } catch (error) {
      this.reportError(shellString("shell_could_not_remove_the_clipboard_temporary_image_detail", "Could not remove the clipboard temporary image: {0}", (error as BusinessError).message));
    }
  }

  private async closeApplication(): Promise<void> {
    if (this.closing) {
      return;
    }
    this.closing = true;
    // Even an authorized clean close must not terminate while a provider worker still reads its sandbox target.
    try {
      await Promise.all([this.fileTransactions.waitForIdle(), this.folders.cancelAndWait(), this.outputFolders.cancelAndWait(), this.publications.cancelAndWait()]);
    } catch (error) {
      this.closing = false;
      this.pendingClose = false;
      this.fileTransactions.resumeAfterFailedClose();
      this.reportError(shellString("shell_could_not_wait_for_file_operations_to_finish_detail", "Could not wait for file operations to finish: {0}", (error as BusinessError).message));
      return;
    }
    this.stopClosingPoll();
    try {
      this.suspend();
    } catch (error) {
      this.reportError(shellString("shell_could_not_close_the_input_method_detail", "Could not close the input method: {0}", (error as BusinessError).message));
    }
    try {
      // destroyWindow does not close the Stage main window; terminate its UIAbility instead.
      await this.context.terminateSelf();
    } catch (error) {
      this.closing = false;
      this.pendingClose = false;
      this.fileTransactions.resumeAfterFailedClose();
      this.reportError(shellString("shell_could_not_close_photocraft_detail", "Could not close PhotoCraft: {0}", (error as BusinessError).message));
    }
  }

  private beginFolderImport(event: PlatformEvent): void {
    if (typeof event.target !== 'string' || event.target.length < 1 || event.scope !== 'topLevel' ||
      event.copyLayout !== 'sourceBasenameChild' || event.limits === undefined || event.intent === undefined) {
      this.reportError(shellString("shell_invalid_folder_input_request_format", "Invalid folder input request format"));
      if (typeof event.target === 'string' && event.target.length > 0) {
        this.sendInput(JSON.stringify({ kind: 'folderComplete', id: event.id, target: event.target,
          result: 'error', copyLayout: 'sourceBasenameChild', ownerRoot: '', resolvedRoot: '',
          manifestPath: '', bindingId: '', error: shellString("shell_invalid_folder_input_request_format", "Invalid folder input request format") }));
      }
      return;
    }
    const request: FolderImportEvent = { kind: 'folderImport', id: event.id, target: event.target,
      intent: event.intent, scope: 'topLevel', copyLayout: 'sourceBasenameChild', limits: event.limits };
    this.folders.begin(request);
  }

  private updateFolderPolling(): void {
    const active = this.hasFolderWork();
    if (active && !this.disposed && this.folderPollTimer === -1) {
      this.folderPollTimer = setInterval(() => {
        try { this.poll(); } catch (error) {
          this.reportError(shellString("shell_could_not_handle_the_folder_operation_detail", "Could not handle the folder operation: {0}", (error as BusinessError).message));
        }
      }, 200);
    } else if (!active && this.folderPollTimer !== -1) {
      clearInterval(this.folderPollTimer); this.folderPollTimer = -1; this.restoreFocus();
    }
  }

  private beginFolderOutput(event: PlatformEvent): void {
    if (typeof event.target !== 'string' || event.target.length === 0) {
      this.reportError(shellString("shell_invalid_output_folder_request_identity", "Invalid output folder request identity")); return;
    }
    if (event.kind === 'folderDestination') {
      const request: FolderDestinationRequest = { kind: 'folderDestination', id: event.id, target: event.target,
        intent: event.intent as 'file.scripts.imageProcessor' };
      this.outputFolders.begin(request);
    } else {
      const request: FolderStageRequest = { kind: 'folderStageAllocate', id: event.id, target: event.target,
        intent: event.intent as 'file.scripts.imageProcessor', destinationHandle: event.destinationHandle || '' };
      this.outputFolders.begin(request);
    }
  }

  private beginFolderPublish(event: PlatformEvent): void {
    if (typeof event.target !== 'string' || event.target.length === 0) {
      this.reportError(shellString("shell_invalid_folder_publication_request_identity", "Invalid folder publication request identity")); return;
    }
    const request: FolderPublicationRequest = { kind: 'folderPublish', id: event.id, target: event.target,
      ownerRoot: event.ownerRoot || '', stageRoot: event.stageRoot || '',
      destinationHandle: event.destinationHandle || '', policy: event.policy as 'freshSubdirectory',
      limits: event.limits || { maxFiles: 0, maxBytes: 0 } };
    this.publications.begin(request);
  }

  private async releaseDestination(event: PlatformEvent): Promise<void> {
    try {
      await this.destinations.release(event.destinationHandle || '', event.target || '');
    } catch (_) {
      this.reportError(shellString("shell_the_local_folder_selection_record_could_not_be_removed", "The local folder selection record could not be removed. It has been retained; user output files are unaffected"));
    }
  }

}
