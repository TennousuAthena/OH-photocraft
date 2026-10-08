import { shellString } from './ShellLocalization';
import { AppStorageRoots } from './AppStorageRoots';
import { common } from '@kit.AbilityKit';
import { util } from '@kit.ArkTS';
import { BusinessError } from '@kit.BasicServicesKit';
import { fileIo, fileShare, fileUri, picker } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { FolderImportCancelled, FolderLimits, FolderNode, FolderStorage,
  inspectSandboxFolder, removeOwnedSandboxFolder, resolveSourceBasenameChild,
  validateFolderLimits, validateFolderRelativePath } from './SandboxFolderManifest';

export interface FolderImportRequest {
  id: number;
  target: string;
  kind: 'folderImport';
  intent: string;
  limits: FolderLimits;
  recursive: boolean;
  copyLayout: 'sourceBasenameChild';
}

export interface FolderImportProgress {
  kind: 'folderProgress';
  id: number;
  target: string;
  processedBytes: number;
  totalBytes: number;
  filesDone: number;
}

export interface FolderImportCompletion {
  kind: 'folderComplete';
  id: number;
  target: string;
  result: 'success' | 'cancel' | 'error';
  copyLayout: 'sourceBasenameChild';
  ownerRoot: string;
  resolvedRoot: string;
  manifestPath: string;
  bindingId: string;
  error: string;
}

interface FolderImportSource {
  version: number;
  requestId: number;
  target: string;
  bindingId: string;
  copyLayout: string;
}

export interface FolderImportService {
  importFolder(request: FolderImportRequest,
    progress: (value: FolderImportProgress) => void): Promise<FolderImportCompletion>;
  cancel(id: number, target: string): boolean;
  releaseRetained(completion: FolderImportCompletion, expectedId: number, expectedTarget: string): Promise<void>;
}

export class SandboxFolderStorage implements FolderStorage {
  async list(directory: string): Promise<string[]> { return await fileIo.listFile(directory); }

  async inspect(path: string): Promise<FolderNode> {
    const stat = await fileIo.lstat(path);
    return { kind: stat.isSymbolicLink() ? 'symlink' : stat.isDirectory() ? 'directory' :
      stat.isFile() ? 'file' : 'other', bytes: stat.size };
  }

  async unlink(path: string): Promise<void> { await fileIo.unlink(path); }

  async removeEmptyDirectory(path: string): Promise<void> {
    // SDK rmdir is recursive. Ensure it cannot traverse any remaining child.
    if ((await fileIo.listFile(path)).length > 0) {
      throw new Error(shellString("shell_the_import_directory_is_not_empty_recursive_cleanup_was", "The import directory is not empty. Recursive cleanup was refused"));
    }
    await fileIo.rmdir(path);
  }
}

/** Folder URI is opened only by SDK copy. Everything enumerated/deleted is app-owned sandbox data. */
export class FolderImportBridge implements FolderImportService {
  private context: common.UIAbilityContext;
  private roots: AppStorageRoots;
  private storage: SandboxFolderStorage = new SandboxFolderStorage();
  private activeId: number = -1;
  private activeTarget: string = '';
  private cancelled: boolean = false;
  private limitExceeded: boolean = false;
  private signal: fileIo.TaskSignal | undefined = undefined;
  private copying: boolean = false;

  constructor(context: common.UIAbilityContext, roots: AppStorageRoots = new AppStorageRoots(context)) { this.context = context; this.roots = roots; }

  cancel(id: number, target: string): boolean {
    if (id !== this.activeId || target !== this.activeTarget) {
      return false;
    }
    this.cancelled = true;
    this.stopCopy();
    return true;
  }

  async importFolder(request: FolderImportRequest,
    onProgress: (progress: FolderImportProgress) => void): Promise<FolderImportCompletion> {
    return await this.copyFolder(request, onProgress);
  }

  /** A previously persisted private binding must be reactivated, never repersisted or guessed. */
  async reimportFolder(request: FolderImportRequest, sourceUri: string): Promise<FolderImportCompletion> {
    if (sourceUri.length === 0 || sourceUri.length > 8192 || sourceUri.includes('\x00')) {
      throw new Error(shellString("shell_invalid_original_folder_source_record", "Invalid original folder source record"));
    }
    return await this.copyFolder(request, (_: FolderImportProgress): void => {}, sourceUri);
  }

  private async copyFolder(request: FolderImportRequest,
    onProgress: (progress: FolderImportProgress) => void, boundUri?: string): Promise<FolderImportCompletion> {
    const completion: FolderImportCompletion = { kind: 'folderComplete', id: request.id, target: request.target,
      result: 'error', copyLayout: 'sourceBasenameChild', ownerRoot: '',
      resolvedRoot: '', manifestPath: '', bindingId: '', error: '' };
    if (this.activeId >= 0) {
      completion.error = shellString("shell_another_folder_import_is_in_progress", "Another folder import is in progress");
      return completion;
    }
    let job = '';
    this.cancelled = false;
    this.limitExceeded = false;
    let progressError = '';
    let phase = 'validate-request';
    try {
      if (!Number.isSafeInteger(request.id) || request.id < 0 || request.kind !== 'folderImport' || !request.recursive ||
        typeof request.target !== 'string' || request.target.length < 1 || request.target.length > 512 ||
        request.target.includes('\x00') || request.copyLayout !== 'sourceBasenameChild') {
        throw new Error(shellString("shell_invalid_folder_import_request", "Invalid folder import request"));
      }
      validateFolderLimits(request.limits);
      if (!canIUse('SystemCapability.FileManagement.UserFileService.FolderSelection') ||
        !canIUse('SystemCapability.FileManagement.AppFileService.FolderAuthorization')) {
        throw new Error(shellString("shell_this_device_does_not_support_persistent_folder_import_permissions", "This device does not support persistent folder import permissions"));
      }
      this.activeId = request.id;
      this.activeTarget = request.target;
      this.cancelled = false;
      this.limitExceeded = false;
      const options = new picker.DocumentSelectOptions();
      options.selectMode = picker.DocumentSelectMode.FOLDER;
      options.maxSelectNumber = 1;
      phase = 'folder-picker';
      const selected = boundUri === undefined ? await new picker.DocumentViewPicker(this.context).select(options) : [boundUri];
      this.checkCancelled();
      if (selected.length === 0) {
        throw new FolderImportCancelled();
      }
      if (selected.length !== 1) {
        throw new Error(shellString("shell_select_only_one_folder_to_import", "Select only one folder to import"));
      }
      const uri = selected[0];
      phase = 'source-permission';
      const policies: fileShare.PolicyInfo[] = [{ uri: uri, operationMode: fileShare.OperationMode.READ_MODE }];
      if (boundUri === undefined) { await fileShare.persistPermission(policies); }
      if (!(await fileShare.checkPersistentPermission(policies))[0]) {
        throw new Error(shellString("shell_could_not_retain_permission_to_the_selected_folder", "Could not retain permission to the selected folder"));
      }
      await fileShare.activatePermission(policies);
      this.checkCancelled();
      phase = 'sandbox-allocation';
      const imports = await this.prepareImportDirectory();
      job = await fileIo.mkdtemp(`${imports}/${request.id}-XXXXXX`);
      const contents = `${job}/contents`;
      await fileIo.mkdir(contents);
      const bindingId = job.substring(job.lastIndexOf('/') + 1);
      await this.writeMetadata(`${job}/source.json`, JSON.stringify({ version: 1, requestId: request.id,
        target: request.target, bindingId: bindingId, sourceUri: uri, intent: request.intent,
        copyLayout: request.copyLayout, operationMode: fileShare.OperationMode.READ_MODE }), 65536);
      this.signal = new fileIo.TaskSignal();
      this.copying = true;
      phase = 'provider-copy';
      try {
        await fileIo.copy(uri, fileUri.getUriFromPath(contents), {
          copySignal: this.signal,
          progressListener: (progress: fileIo.Progress) => {
            if (progress.processedSize > request.limits.maxBytes || progress.totalSize > request.limits.maxBytes) {
              this.limitExceeded = true;
              this.stopCopy();
            }
            try {
              onProgress({ kind: 'folderProgress', id: request.id, target: request.target,
                processedBytes: progress.processedSize,
                totalBytes: progress.totalSize, filesDone: 0 });
            } catch (error) {
              progressError = (error as Error).message;
              this.stopCopy();
            }
          }
        });
      } finally {
        this.copying = false;
        this.signal = undefined;
      }
      this.checkCancelled();
      if (progressError.length > 0) {
        throw new Error(progressError);
      }
      if (this.limitExceeded) {
        throw new Error(shellString("shell_the_folder_exceeds_the_size_limit_import_stopped", "The folder exceeds the size limit. Import stopped"));
      }
      phase = 'copy-layout';
      // This is the explicitly selected SDK layout contract, not an arbitrary tree-unwrapping heuristic.
      const resolvedRoot = await resolveSourceBasenameChild(this.storage, contents, () => this.cancelled);
      const manifest = await inspectSandboxFolder(this.storage, resolvedRoot, request.limits, () => this.cancelled);
      phase = 'manifest-sync';
      for (const entry of manifest.entries) {
        this.checkCancelled();
        if (entry.kind === 'file') {
          await this.syncPath(`${resolvedRoot}/${entry.relativePath}`);
        }
      }
      const manifestPath = `${job}/manifest.json`;
      await this.writeMetadata(manifestPath, JSON.stringify(manifest));
      // Persist sandbox directory entries before returning ownership to the caller.
      for (const entry of manifest.entries.slice().reverse()) {
        if (entry.kind === 'directory') {
          await this.syncPath(`${resolvedRoot}/${entry.relativePath}`);
        }
      }
      await this.syncPath(resolvedRoot);
      await this.syncPath(contents);
      await this.syncPath(job);
      await this.syncPath(imports);
      await this.syncPath(imports.substring(0, imports.lastIndexOf('/')));
      this.checkCancelled();
      onProgress({ kind: 'folderProgress', id: request.id, target: request.target, processedBytes: manifest.totalBytes,
        totalBytes: manifest.totalBytes, filesDone: manifest.fileCount });
      this.checkCancelled();
      completion.result = 'success';
      completion.ownerRoot = job;
      completion.resolvedRoot = resolvedRoot;
      completion.manifestPath = manifestPath;
      completion.bindingId = bindingId;
      return completion;
    } catch (error) {
      const code = (error as BusinessError).code;
      hilog.error(0x0000, 'PhotoCraft', 'folder-import stage=%{public}s code=%{public}d',
        phase, typeof code === 'number' ? code : 0);
      completion.result = this.cancelled || error instanceof FolderImportCancelled ? 'cancel' : 'error';
      completion.error = completion.result === 'cancel' ? '' : this.limitExceeded ?
        shellString("shell_the_folder_exceeds_the_size_limit_import_stopped", "The folder exceeds the size limit. Import stopped") : typeof code === 'number' ?
          shellString("shell_folder_import_failed_check_the_permissions_or_folder_and", "Folder import failed. Check the permissions or folder and try again (error {0})", code) :
          ((error as Error).message || shellString("shell_folder_import_failed_try_again", "Folder import failed. Try again"));
      if (job.length > 0) {
        try {
          // copy has settled before cleanup; never race a still-running provider worker.
          await removeOwnedSandboxFolder(this.storage, job);
        } catch (cleanupError) {
          completion.result = 'error';
          completion.ownerRoot = job;
          completion.bindingId = job.substring(job.lastIndexOf('/') + 1);
          completion.error += shellString("shell_cleanup_of_the_imported_copy_is_incomplete_local_files", "; cleanup of the imported copy is incomplete. Local files have been retained");
        }
      }
      return completion;
    } finally {
      this.activeId = -1;
      this.activeTarget = '';
      this.copying = false;
      this.signal = undefined;
    }
  }

  async releaseRetained(completion: FolderImportCompletion, expectedId: number, expectedTarget: string): Promise<void> {
    if (this.activeId === expectedId && this.activeTarget === expectedTarget) {
      throw new Error(shellString("shell_folder_copying_is_still_running_copy_cleanup_was_refused", "Folder copying is still running. Copy cleanup was refused"));
    }
    const base = `${this.roots.filesDir}/PhotoCraft/Documents/folder-imports/`;
    const name = completion.ownerRoot.startsWith(base) ? completion.ownerRoot.substring(base.length) : '';
    if (!Number.isSafeInteger(expectedId) || expectedId < 0 ||
      !new RegExp(`^${expectedId}-[A-Za-z0-9]{6}$`).test(name) || completion.bindingId !== name) {
      throw new Error(shellString("shell_the_imported_copy_belongs_to_another_request_cleanup_was", "The imported copy belongs to another request. Cleanup was refused"));
    }
    // Check every owned ancestor again; cleanup must never follow a replaced parent link.
    let directory = this.roots.filesDir;
    for (const part of ['', 'PhotoCraft', 'Documents', 'folder-imports', name]) {
      if (part.length > 0) { directory += `/${part}`; }
      if ((await this.storage.inspect(directory)).kind !== 'directory') {
        throw new Error(shellString("shell_an_imported_copy_ancestor_is_not_a_regular_directory", "An imported copy ancestor is not a regular directory. Cleanup was refused"));
      }
    }
    const sourcePath = `${completion.ownerRoot}/source.json`;
    const sourceNode = await this.storage.inspect(sourcePath);
    if (sourceNode.kind !== 'file' || sourceNode.bytes < 1 || sourceNode.bytes > 65536) {
      throw new Error(shellString("shell_the_imported_copy_ownership_record_is_invalid_cleanup_was", "The imported copy ownership record is invalid. Cleanup was refused"));
    }
    const source = JSON.parse(await fileIo.readText(sourcePath)) as FolderImportSource;
    if (source.version !== 1 || source.requestId !== expectedId || source.target !== expectedTarget ||
      source.bindingId !== name || source.copyLayout !== 'sourceBasenameChild') {
      throw new Error(shellString("shell_the_imported_copy_target_does_not_match_cleanup_was", "The imported copy target does not match. Cleanup was refused"));
    }
    if (completion.resolvedRoot.length > 0) {
      const prefix = `${completion.ownerRoot}/contents/`;
      const child = completion.resolvedRoot.startsWith(prefix) ? completion.resolvedRoot.substring(prefix.length) : '';
      validateFolderRelativePath(child);
      if (child.includes('/') || completion.manifestPath !== `${completion.ownerRoot}/manifest.json`) {
        throw new Error(shellString("shell_the_imported_copy_root_or_manifest_is_outside_its", "The imported copy root or manifest is outside its directory. Cleanup was refused"));
      }
    }
    await removeOwnedSandboxFolder(this.storage, completion.ownerRoot);
  }

  private stopCopy(): void {
    if (this.copying && this.signal !== undefined) {
      // If the provider cannot cancel immediately, still wait for copy to settle before cleanup.
      try { this.signal.cancel(); } catch (_) { }
    }
  }

  private checkCancelled(): void {
    if (this.cancelled) {
      throw new FolderImportCancelled();
    }
  }

  private async prepareImportDirectory(): Promise<string> {
    let directory = this.roots.filesDir;
    for (const name of ['', 'PhotoCraft', 'Documents', 'folder-imports']) {
      if (name.length > 0) {
        directory += `/${name}`;
        if (!await fileIo.access(directory)) {
          try { await fileIo.mkdir(directory); } catch (error) {
            if ((error as BusinessError).code !== 13900015) { throw error as Error; }
          }
        }
      }
      if ((await this.storage.inspect(directory)).kind !== 'directory') {
        throw new Error(shellString("shell_the_folder_import_storage_directory_is_invalid_links_will", "The folder import storage directory is invalid. Links will not be followed"));
      }
    }
    return directory;
  }

  private async syncPath(path: string): Promise<void> {
    const file = await fileIo.open(path, fileIo.OpenMode.READ_ONLY | fileIo.OpenMode.NOFOLLOW);
    try { await fileIo.fsync(file.fd); } finally { await fileIo.close(file); }
  }

  private async writeMetadata(path: string, value: string, maxBytes: number = 1024 * 1024): Promise<void> {
    const file = await fileIo.open(path, fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.NOFOLLOW);
    try {
      const bytes = new util.TextEncoder().encodeInto(value);
      if (bytes.byteLength > maxBytes) { throw new Error(shellString("shell_the_folder_manifest_or_permission_record_exceeds_the_size", "The folder manifest or permission record exceeds the size limit")); }
      if (await fileIo.write(file.fd, bytes.buffer as ArrayBuffer) !== bytes.byteLength) {
        throw new Error(shellString("shell_the_folder_manifest_was_not_fully_written", "The folder manifest was not fully written"));
      }
      await fileIo.fsync(file.fd);
    } finally {
      await fileIo.close(file);
    }
  }
}
