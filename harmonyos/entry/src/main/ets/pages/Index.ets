import { shellString } from '../platform/ShellLocalization';
import { AppStorageRoots } from '../platform/AppStorageRoots';
import { recordDeviceTestPublication } from '../platform/DeviceTestEvidence';
import { common } from '@kit.AbilityKit';
import { unifiedDataChannel, uniformDataStruct, uniformTypeDescriptor } from '@kit.ArkData';
import { BusinessError } from '@kit.BasicServicesKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import nativePhotoCraft from 'libphotocraft.so';
import { DocumentBridge, FileRequest, ImportedDocument } from '../platform/DocumentBridge';
import { PlatformBridge } from '../platform/PlatformBridge';
import { FileTransactionDrain, FileTransactionToken } from '../platform/FileTransactionDrain';
import { PublicationFailure } from '../platform/PublicationTransaction';
import { ClipboardContent } from '../platform/ClipboardBridge';
import { PasteAccessRequest, ClipboardReadCancelled } from '../platform/PasteAuthorization';

@CustomDialog
struct PasteAccessDialog {
  controller?: CustomDialogController;
  approve: () => Promise<void> = async () => {};
  cancel: () => void = () => {};
  fail: (error: Error) => void = () => {};
  @State private reading: boolean = false;

  build() {
    Column({ space: 20 }) {
      Text(this.reading ? $r('app.string.shell_index_paste_reading') : $r('app.string.shell_index_paste_confirm'))
        .fontSize(18)
        .fontColor('#18202B')
      Row({ space: 20 }) {
        if (!this.reading) {
          // Keep the system security component visible, with its default icon/text/background.
          PasteButton()
            .id('photocraft-paste-authorize')
            .padding({ top: 12, bottom: 12, left: 24, right: 24 })
            .onClick((_event: ClickEvent, result: PasteButtonOnClickResult) => {
              if (result === PasteButtonOnClickResult.SUCCESS) {
                this.reading = true;
                this.approve();
              } else {
                this.fail(new Error(shellString('shell_index_paste_denied', "The system could not authorize this paste. Please try again.")));
              }
            })
        }
        Button($r('app.string.shell_index_cancel'))
          .onClick(() => { this.cancel(); })
      }
    }
    .padding(28)
    .backgroundColor('#FFFFFF')
    .borderRadius(16)
  }
}

@Entry
@Component
struct Index {
  @State private isReady: boolean = false;
  @State private fatalError: string = '';
  @StorageLink('photocraft.foreground') @Watch('foregroundChanged') private foreground: boolean = true;
  private isBusy: boolean = false;
  private disposed: boolean = false;
  private fileTransactions: FileTransactionDrain = new FileTransactionDrain();
  private pageVisible: boolean = false;
  private pollTimer: number = -1;
  private documentBridge: DocumentBridge | undefined = undefined;
  private storageRoots: AppStorageRoots | undefined = undefined;
  private dropKey: string = '';
  private dropLoading: boolean = false;
  private platformBridge: PlatformBridge | undefined = undefined;
  private pasteAccess: PasteAccessRequest | undefined = undefined;
  private pasteDialog: CustomDialogController | undefined = undefined;
  private recentSourcesChanged: boolean = false;

  onPageShow(): void {
    this.pageVisible = true;
    this.platformBridge?.setPageVisible(true);
    this.startPolling();
    this.focusEditor();
  }

  onPageHide(): void {
    this.pageVisible = false;
    this.cancelPasteAccess();
    this.platformBridge?.setPageVisible(false);
    this.stopPolling();
    this.suspendPlatform();
  }

  aboutToDisappear(): void {
    this.disposed = true;
    this.pageVisible = false;
    this.cancelPasteAccess();
    this.stopPolling();
    this.fileTransactions.dispose();
    try {
      this.platformBridge?.dispose();
    } catch (error) {
      this.reportFileError(shellString('shell_index_close_platform_failed', "Cannot close platform services: {0}", this.errorMessage(error as BusinessError)));
    }
    if (this.dropLoading && this.dropKey.length > 0) {
      try {
        this.getUIContext().getDragController().cancelDataLoading(this.dropKey);
      } catch (error) {
        this.reportFileError(shellString('shell_index_cancel_drop_failed', "Cannot cancel file drop: {0}", this.errorMessage(error as BusinessError)));
      }
    }
  }

  private focusEditor(): void {
    if (this.isReady && this.pageVisible && this.foreground && !this.isBusy && this.pasteAccess === undefined &&
      !this.platformBridge?.hasFolderWork()) {
      try {
        this.getUIContext().getFocusController().requestFocus('photocraft-surface');
      } catch (error) {
        hilog.warn(0x0000, 'PhotoCraft', 'Cannot focus editor: %{public}s',
          this.errorMessage(error as BusinessError));
      }
    }
  }

  private foregroundChanged(): void {
    if (this.foreground) {
      this.startPolling();
      this.focusEditor();
    } else {
      this.cancelPasteAccess();
      this.stopPolling();
      this.suspendPlatform();
    }
  }

  private async initializeNative(): Promise<void> {
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      const roots = AppStorageRoots.current(context);
      this.storageRoots = roots;
      this.documentBridge = new DocumentBridge(context, roots);
      let restoreError = '';
      try {
        restoreError = await this.documentBridge.restoreBindings();
      } catch (error) {
        restoreError = shellString('shell_index_restore_bindings_failed', "Cannot restore file destinations: {0}", this.errorMessage(error as BusinessError));
      }
      // Use the actual PC window display scale, including desktop scaling.
      this.isReady = nativePhotoCraft.initialize(roots.filesDir, roots.cacheDir, this.getUIContext().vp2px(1));
      if (!this.isReady) {
        this.fatalError = nativePhotoCraft.lastError() || shellString('shell_index_init_failed', "PhotoCraft initialization failed");
        return;
      }
      this.platformBridge = new PlatformBridge(context, this.getUIContext(),
        (message: string) => { this.reportFileError(message); }, () => { this.focusEditor(); },
        (read: () => Promise<ClipboardContent>, discard: (content: ClipboardContent) => Promise<void>) =>
          this.authorizePaste(read, discard), () => { this.cancelPasteAccess(); },
        () => this.isReady && this.pageVisible && !this.isBusy && this.pasteAccess === undefined, this.fileTransactions, roots);
      try {
        await this.platformBridge.initialize();
      } catch (error) {
        this.reportFileError(shellString('shell_index_platform_init_failed', "Cannot initialize system services: {0}", this.errorMessage(error as BusinessError)));
      }
      if (restoreError.length > 0) {
        this.reportFileError(restoreError);
      }
      this.startPolling();
      this.focusEditor();
    } catch (error) {
      this.fatalError = shellString('shell_index_init_detail', "PhotoCraft initialization failed: {0}", this.errorMessage(error as BusinessError));
    }
  }

  private startPolling(): void {
    if (this.isReady && this.pageVisible && this.foreground && this.pollTimer === -1) {
      this.pollTimer = setInterval(() => { this.notifyRecentSources(); this.pollPlatform(); this.pollFileRequests(); }, 200);
    }
  }

  private async authorizePaste(read: () => Promise<ClipboardContent>,
    discard: (content: ClipboardContent) => Promise<void>): Promise<ClipboardContent> {
    const drain = this.fileTransactions;
    const token = drain.tryBegin();
    if (token === undefined) {
      throw new ClipboardReadCancelled();
    }
    const request = new PasteAccessRequest(read, discard);
    if (!this.pageVisible || !this.foreground || this.pasteAccess !== undefined) {
      request.cancel();
      drain.end(token);
      return await request.completion;
    }
    this.pasteAccess = request;
    let dialog: CustomDialogController | undefined = undefined;
    try {
      // The system grants foreground physical Ctrl+V briefly. Show the security button only if that grant is absent.
      if (await request.tryDirect()) { return await request.completion; }
      dialog = new CustomDialogController({
        builder: PasteAccessDialog({ approve: async () => { await request.approve(); },
          cancel: () => { request.cancel(); }, fail: (error: Error) => { request.fail(error); } }),
        autoCancel: true,
        cancel: () => { request.cancel(); }
      });
      this.pasteDialog = dialog;
      dialog.open();
      return await request.completion;
    } catch (error) {
      request.fail(error as Error);
      return await request.completion;
    } finally {
      try {
        try {
          dialog?.close();
        } catch (error) {
          this.reportFileError(shellString('shell_index_close_paste_failed', "Cannot close the paste panel: {0}", this.errorMessage(error as BusinessError)));
        }
      } finally {
        // Cancel rejects completion immediately; the provider read and a late PNG discard still own this token.
        await request.settled;
        try {
          if (request.cleanupError !== undefined) {
            this.reportFileError(shellString('shell_index_read_paste_failed', "Cannot read or clean up pasted content: {0}", this.errorMessage(request.cleanupError as BusinessError)));
          }
        } finally {
          if (this.pasteAccess === request) {
            this.pasteAccess = undefined;
            this.pasteDialog = undefined;
          }
          drain.end(token);
        }
      }
    }
  }

  private cancelPasteAccess(): void {
    this.pasteAccess?.cancel();
  }

  private pollPlatform(): void {
    if (!this.isReady || !this.pageVisible || !this.foreground || this.isBusy) {
      return;
    }
    try {
      this.platformBridge?.poll();
    } catch (error) {
      this.reportFileError(shellString('shell_index_platform_output_failed', "Cannot handle platform output: {0}", this.errorMessage(error as BusinessError)));
    }
  }

  private suspendPlatform(): void {
    try {
      this.platformBridge?.suspend();
    } catch (error) {
      this.reportFileError(shellString('shell_index_suspend_ime_failed', "Cannot suspend the input method: {0}", this.errorMessage(error as BusinessError)));
    }
  }

  private stopPolling(): void {
    if (this.pollTimer !== -1) {
      clearInterval(this.pollTimer);
      this.pollTimer = -1;
    }
  }

  private errorMessage(error: BusinessError): string {
    return error.message || shellString('shell_index_error_code', "Error {0}", String(error.code));
  }

  private reportFileError(message: string): void {
    hilog.error(0x0000, 'PhotoCraft', '%{public}s', message);
    if (this.isReady) {
      nativePhotoCraft.reportFileError(message);
    }
  }

  private pollFileRequests(): void {
    if (!this.isReady || !this.pageVisible || !this.foreground || this.isBusy || !this.documentBridge ||
      this.pasteAccess !== undefined) {
      return;
    }
    if (this.platformBridge?.hasFolderWork()) { return; }
    const drain = this.fileTransactions;
    const token = drain.tryBegin();
    if (token === undefined) { return; }
    let handedOff = false;
    try {
      const serialized = nativePhotoCraft.takeFileRequest();
      if (serialized.length === 0) {
        return;
      }
      const request = JSON.parse(serialized) as FileRequest;
      if (!Number.isSafeInteger(request.id) || request.id < 0) {
        this.reportFileError(shellString('shell_index_request_id_invalid', "Invalid file request ID"));
        return;
      }
      this.isBusy = true;
      this.suspendPlatform();
      handedOff = true;
      this.runFileRequest(request, drain, token);
    } catch (error) {
      this.reportFileError(shellString('shell_index_read_request_failed', "Cannot read file request: {0}", this.errorMessage(error as BusinessError)));
    } finally {
      if (!handedOff) { drain.end(token); }
    }
  }

  private async runFileRequest(request: FileRequest, drain: FileTransactionDrain, token: FileTransactionToken): Promise<void> {
    const bridge = this.documentBridge;
    let imported: ImportedDocument | undefined = undefined;
    let accepted: boolean = false;
    let completed: boolean = false;
    let phase = 'open-picker';
    try {
      if (bridge === undefined) {
        throw new Error(shellString('shell_index_file_service_not_ready', "File service is not ready"));
      }
      if (request.kind === 'open') {
        imported = await bridge.openRequestedDocument(request);
        if (this.disposed) { return; }
        if (imported === undefined) {
          completed = true;
          nativePhotoCraft.completeFileRequest(request.id, 'cancel', '', '', '');
        } else {
          completed = true;
          // Rust retains the original menu intent (Open As / Place / Open).
          accepted = nativePhotoCraft.completeFileRequest(request.id, 'success', imported.path,
            imported.originalName, '');
          if (accepted && request.retainSource !== false) {
            await bridge.bindImportedSource(request.bindToPrevious ? request.previousPath : imported.path, imported);
            this.recentSourcesChanged = true;
            this.notifyRecentSources();
          }
        }
      } else if (request.kind === 'save') {
        phase = 'save-picker';
        let destination = request.chooseDestination ? undefined : bridge.destinationFor(request.previousPath);
        const reusedDestination = destination !== undefined;
        if (destination === undefined) {
          destination = await bridge.selectSaveDestination(request.suggestedName,
            request.allowFormatChoice, request.allowedExtensions);
        }
        if (destination === undefined) {
          completed = true;
          nativePhotoCraft.completeFileRequest(request.id, 'cancel', '', '', '');
        } else {
          let destinationName = bridge.destinationName(destination);
          // Re-encode the request's captured snapshot, never the currently selected tab.
          phase = 'prepare-save';
          const actualStage = nativePhotoCraft.prepareFileSave(request.id, destinationName);
          if (actualStage.length === 0) {
            throw new Error(nativePhotoCraft.lastError() || shellString('shell_index_encode_failed', "Cannot encode the selected save format"));
          }
          try {
            phase = 'publish-save';
            destinationName = await bridge.publishDocument(actualStage, destination);
          } catch (error) {
            if (reusedDestination) {
              // Expired/revoked URI grants must lead to a picker on the next Save.
              try {
                await bridge.forgetSource(request.previousPath);
              } catch (bindingError) {
                hilog.warn(0x0000, 'PhotoCraft', 'Cannot persist removed URI binding: %{public}s',
                  this.errorMessage(bindingError as BusinessError));
              }
            }
            throw error as Error;
          }
          if (this.disposed) {
            // Publication already changed the external file. Preserve its private
            // binding even when the old component has disappeared; never ACK a replacement app.
            if (request.intent === 'file.save' || request.retainSource === true) {
              await bridge.bindSource(actualStage, destination);
            }
            return;
          }
          completed = true;
          phase = 'complete-save';
          accepted = nativePhotoCraft.completeFileRequest(request.id, 'success', '', destinationName, '');
          if (accepted && this.storageRoots !== undefined) {
            recordDeviceTestPublication(this.storageRoots, request.id, destination, destinationName);
          }
          if (accepted && (request.intent === 'file.save' || request.retainSource === true)) {
            phase = 'persist-save-binding';
            await bridge.bindSource(actualStage, destination);
            this.recentSourcesChanged = true;
            this.notifyRecentSources();
          }
          const publicationWarning = bridge.takePublicationWarning();
          if (publicationWarning.length > 0) {
            this.reportFileError(publicationWarning);
          }
        }
      } else {
        throw new Error(shellString('shell_index_request_unsupported', "Unsupported file request"));
      }
    } catch (error) {
      hilog.error(0x0000, 'PhotoCraft', 'File request failure stage=%{public}s code=%{public}d',
        phase, (error as BusinessError).code || 0);
      const message = this.errorMessage(error as BusinessError);
      if (!completed && !this.disposed) {
        completed = true;
        const retainedStage = error instanceof PublicationFailure ? error.retainStagePath : '';
        nativePhotoCraft.completeFileRequest(request.id, 'error', retainedStage, '', message);
      } else if (!this.disposed) {
        this.reportFileError(message);
      }
    } finally {
      if (bridge !== undefined && imported !== undefined && !accepted) {
        try {
          await bridge.removeTemporaryFile(imported.path);
        } catch (error) {
          hilog.warn(0x0000, 'PhotoCraft', 'Cannot remove failed import: %{public}s',
            this.errorMessage(error as BusinessError));
        }
      }
      this.isBusy = false;
      drain.end(token);
      this.focusEditor();
    }
  }

  private notifyRecentSources(): void {
    if (this.isReady && this.recentSourcesChanged) {
      // Queue backpressure retries on the existing poll. A published file stays successful.
      this.recentSourcesChanged = !nativePhotoCraft.platformInput(JSON.stringify({ kind: 'recentSourcesChanged' }));
    }
  }

  private beginDrop(event: DragEvent): void {
    // Prioritize an already queued menu request; never complete it with drop data.
    this.pollFileRequests();
    // onDrop is explicit cross-app intent even when the source window owns foreground focus.
    if (!this.isReady || !this.pageVisible || this.isBusy || this.pasteAccess !== undefined ||
      this.platformBridge?.hasFolderWork()) {
      event.setResult(DragResult.DRAG_FAILED);
      this.reportFileError(shellString('shell_index_drop_busy', "A file operation is in progress. Complete or cancel it before dropping files."));
      return;
    }
    const drain = this.fileTransactions;
    const token = drain.tryBegin();
    if (token === undefined) {
      event.setResult(DragResult.DRAG_FAILED);
      this.reportFileError(shellString('shell_index_drop_closing', "The app is closing. Cancel closing before dropping files."));
      return;
    }
    this.isBusy = true;
    this.suspendPlatform();
    this.dropLoading = true;
    try {
      // API 15+: deferred/cross-application UDMF data loads asynchronously.
      this.dropKey = event.startDataLoading({
        progressIndicator: unifiedDataChannel.ProgressIndicator.DEFAULT,
        dataProgressListener: (progress: unifiedDataChannel.ProgressInfo,
          data: unifiedDataChannel.UnifiedData | null): void => {
          if (!this.dropLoading) {
            return;
          }
          if (progress.status === unifiedDataChannel.ListenerStatus.PROCESSING) {
            return;
          }
          this.dropLoading = false;
          this.dropKey = '';
          if (this.disposed) {
            this.isBusy = false;
            drain.end(token);
            return;
          }
          if (progress.status === unifiedDataChannel.ListenerStatus.FINISHED && data !== null) {
            this.openDroppedDocuments(data, drain, token);
          } else {
            if (progress.status === unifiedDataChannel.ListenerStatus.CANCELED) {
              this.reportFileError(shellString('shell_index_drop_cancelled', "File drop cancelled"));
            } else {
              this.reportFileError(shellString('shell_index_drop_loading_failed', "Cannot load dropped files; UDMF status: {0}", String(progress.status)));
            }
            this.isBusy = false;
            drain.end(token);
            this.focusEditor();
          }
        }
      });
      event.setResult(DragResult.DRAG_SUCCESSFUL);
    } catch (error) {
      event.setResult(DragResult.DRAG_FAILED);
      this.dropLoading = false;
      this.dropKey = '';
      this.isBusy = false;
      drain.end(token);
      this.reportFileError(shellString('shell_index_drop_failed', "File drop failed: {0}", this.errorMessage(error as BusinessError)));
      this.focusEditor();
    }
  }

  private async openDroppedDocuments(data: unifiedDataChannel.UnifiedData, drain: FileTransactionDrain, token: FileTransactionToken): Promise<void> {
    const bridge = this.documentBridge;
    const failures: string[] = [];
    try {
      if (bridge === undefined) {
        throw new Error(shellString('shell_index_file_service_not_ready', "File service is not ready"));
      }
      const uris: Set<string> = new Set<string>();
      for (const record of data.getRecords()) {
        if (record.getTypes().includes(uniformTypeDescriptor.UniformDataType.FILE_URI)) {
          const entry = record.getEntry(uniformTypeDescriptor.UniformDataType.FILE_URI) as uniformDataStruct.FileUri;
          if (entry.oriUri) {
            uris.add(entry.oriUri);
          }
        } else if (record instanceof unifiedDataChannel.File && record.uri) {
          // Accept legacy File/Image records from older drag sources too.
          uris.add(record.uri);
        }
      }
      if (uris.size === 0) {
        throw new Error(shellString('shell_index_drop_no_uri', "The dropped data contains no readable file URI"));
      }
      for (const uri of uris) {
        let imported: ImportedDocument | undefined = undefined;
        let opened: boolean = false;
        try {
          imported = await bridge.importUri(uri);
          if (this.disposed) { break; }
          opened = nativePhotoCraft.openDocument(imported.path);
          if (opened) {
            await bridge.bindImportedSource(imported.path, imported);
            this.recentSourcesChanged = true;
            this.notifyRecentSources();
          } else {
            failures.push(`${imported.originalName}：${nativePhotoCraft.lastError()}`);
          }
        } catch (error) {
          failures.push(this.errorMessage(error as BusinessError));
        } finally {
          if (imported !== undefined && !opened) {
            try {
              await bridge.removeTemporaryFile(imported.path);
            } catch (error) {
              hilog.warn(0x0000, 'PhotoCraft', 'Cannot remove failed drop: %{public}s',
                this.errorMessage(error as BusinessError));
            }
          }
        }
      }
    } catch (error) {
      failures.push(this.errorMessage(error as BusinessError));
    } finally {
      if (failures.length > 0) {
        this.reportFileError(shellString('shell_index_drop_partial', "Some files could not be opened: {0}", failures.join('; ')));
      }
      this.isBusy = false;
      drain.end(token);
      this.focusEditor();
    }
  }

  build() {
    Stack() {
      XComponent({ id: 'photocraft', type: XComponentType.SURFACE, libraryname: 'photocraft' })
        .id('photocraft-surface')
        .width('100%')
        .height('100%')
        .focusable(true)
        .defaultFocus(true)
        .onLoad(() => { this.initializeNative(); })

      if (this.fatalError.length > 0) {
        Text(this.fatalError)
          .fontColor('#FFFFFF')
          .padding(24)
          .backgroundColor('#20242B')
      }
    }
    .width('100%')
    .height('100%')
    .backgroundColor('#20242B')
    .allowDrop([uniformTypeDescriptor.UniformDataType.FILE_URI, uniformTypeDescriptor.UniformDataType.FILE])
    .onDrop((event: DragEvent) => { this.beginDrop(event); }, { disableDataPrefetch: true })
  }
}
