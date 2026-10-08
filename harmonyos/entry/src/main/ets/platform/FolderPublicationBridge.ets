import { shellString } from './ShellLocalization';
import { AppStorageRoots } from './AppStorageRoots';
import { common } from '@kit.AbilityKit';
import { util } from '@kit.ArkTS';
import { BusinessError } from '@kit.BasicServicesKit';
import { fileIo, fileShare, fileUri, picker } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { FolderManifest, FolderNode, FolderStorage, inspectSandboxFolder } from './SandboxFolderManifest';
import { FolderPublicationCompletion, FolderPublicationJournal, FolderPublicationProgress, FolderPublicationRequest,
  FolderPublicationStorage, FolderPublicationTransaction, isFreshPublicationName,
  validateFolderPublicationIdentity, validateFolderPublicationPaths } from './FolderPublicationTransaction';

export interface FolderPublicationStage {
  ownerRoot: string;
  stageRoot: string;
  journalPath: string;
}

export interface FolderPublicationRecovery extends FolderPublicationStage {
  id: number;
  target: string;
  publicationName: string;
  phase: string;
  partialPossible: boolean;
  message: string;
}

interface FolderPublicationOwner {
  version: number;
  creationId: number;
  target: string;
  ownerRoot: string;
}

class PublicationFolderStorage implements FolderStorage, FolderPublicationStorage {
  private context: common.UIAbilityContext;
  private roots: AppStorageRoots;
  private signal: fileIo.TaskSignal | undefined = undefined;
  private destinationResolver: ((handle: string, target: string) => Promise<string>) | undefined;
  private destinationHandle: string = '';
  private destinationTarget: string = '';

  constructor(context: common.UIAbilityContext,
    destinationResolver?: (handle: string, target: string) => Promise<string>,
    roots: AppStorageRoots = new AppStorageRoots(context)) {
    this.context = context;
    this.roots = roots;
    this.destinationResolver = destinationResolver;
  }

  async list(directory: string): Promise<string[]> { return await fileIo.listFile(directory); }
  async inspect(path: string): Promise<FolderNode> {
    const stat = await fileIo.lstat(path);
    return { kind: stat.isSymbolicLink() ? 'symlink' : stat.isDirectory() ? 'directory' :
      stat.isFile() ? 'file' : 'other', bytes: stat.size };
  }
  // The transaction never invokes these methods: stages and external partial results are retained.
  async unlink(path: string): Promise<void> { throw new Error(shellString("shell_folder_publication_cannot_delete_files", "The folder publication service does not delete output files")); }
  async removeEmptyDirectory(path: string): Promise<void> { throw new Error(shellString("shell_folder_publication_cannot_delete_directories", "The folder publication service does not delete output directories")); }

  async createStage(id: number, target: string): Promise<FolderPublicationStage> {
    validateFolderPublicationIdentity(id, target);
    const base = await this.ensureBase(true);
    const ownerRoot = await fileIo.mkdtemp(`${base}/${id}-XXXXXX`);
    const stageRoot = `${ownerRoot}/contents`;
    await fileIo.mkdir(stageRoot);
    const owner: FolderPublicationOwner = { version: 1, creationId: id, target: target, ownerRoot: ownerRoot };
    await this.writeMetadata(`${ownerRoot}/owner.json`, JSON.stringify(owner));
    await this.writeJournal({ version: 1, id: id, target: target, phase: 'prepared', ownerRoot: ownerRoot,
      stageRoot: stageRoot, publicationName: '', destinationUri: '', partialPossible: false, fileCount: 0, totalBytes: 0 });
    await this.syncPath(stageRoot);
    await this.syncPath(ownerRoot);
    await this.syncPath(base);
    return { ownerRoot: ownerRoot, stageRoot: stageRoot, journalPath: `${ownerRoot}/publication.json` };
  }

  async validateStage(request: FolderPublicationRequest, isCancelled: () => boolean): Promise<FolderManifest> {
    validateFolderPublicationPaths(this.roots.filesDir, request.ownerRoot, request.stageRoot);
    await this.ensureBase(false);
    await this.requireDirectory(request.ownerRoot);
    const owner = await this.readOwner(request.ownerRoot);
    if (owner.target !== request.target) {
      throw new Error(shellString("shell_the_folder_output_belongs_to_another_task_generate_the", "The folder output belongs to another task. Generate the output again"));
    }
    this.destinationHandle = request.destinationHandle || '';
    this.destinationTarget = request.target;
    await this.requireDirectory(request.stageRoot);
    const manifest = await inspectSandboxFolder(this, request.stageRoot, request.limits, isCancelled);
    // The trusted producer must stop changing the entire job until this publish Promise settles.
    for (const entry of manifest.entries) {
      if (isCancelled()) { throw new Error(shellString("shell_folder_export_canceled", "Folder export canceled")); }
      if (entry.kind === 'file') { await this.syncPath(`${request.stageRoot}/${entry.relativePath}`); }
    }
    for (const entry of manifest.entries.slice().reverse()) {
      if (isCancelled()) { throw new Error(shellString("shell_folder_export_canceled", "Folder export canceled")); }
      if (entry.kind === 'directory') { await this.syncPath(`${request.stageRoot}/${entry.relativePath}`); }
    }
    await this.syncPath(request.stageRoot);
    await this.writeMetadata(`${request.ownerRoot}/manifest.json`, JSON.stringify(manifest));
    await this.syncPath(request.ownerRoot);
    return manifest;
  }

  async freshName(ownerRoot: string, previousRoot: string): Promise<string> {
    for (let attempt = 0; attempt < 8; attempt++) {
      const name = `PhotoCraft-Export-${util.generateRandomUUID(true)}`;
      if (!isFreshPublicationName(name)) { throw new Error(shellString("shell_invalid_system_generated_folder_name", "Invalid system generated folder name")); }
      const destination = `${ownerRoot}/${name}`;
      if (destination !== previousRoot && !await this.pathExists(destination)) { return name; }
    }
    throw new Error(shellString("shell_could_not_create_a_new_output_folder_name", "Could not create a new output folder name"));
  }

  async renameStage(source: string, destination: string): Promise<void> {
    await this.requireDirectory(source);
    if (await this.pathExists(destination)) {
      throw new Error(shellString("shell_the_output_folder_name_already_exists_overwriting_was_refused", "The output folder name already exists. Overwriting was refused"));
    }
    await this.perform<void>('folder-stage-rename', () => fileIo.rename(source, destination));
  }

  async locateStage(ownerRoot: string, previousRoot: string, publicationName: string): Promise<string> {
    const candidate = `${ownerRoot}/${publicationName}`;
    const oldExists = await this.pathExists(previousRoot);
    const newExists = await this.pathExists(candidate);
    if (oldExists === newExists) { throw new Error(shellString("shell_the_output_copy_location_needs_manual_inspection", "The output copy location needs manual inspection")); }
    const root = oldExists ? previousRoot : candidate;
    await this.requireDirectory(root);
    return root;
  }

  async syncOwner(ownerRoot: string): Promise<void> { await this.syncPath(ownerRoot); }

  async writeJournal(journal: FolderPublicationJournal): Promise<void> {
    await this.requireDirectory(journal.ownerRoot);
    await this.writeMetadata(`${journal.ownerRoot}/publication.json`, JSON.stringify(journal));
    await this.syncPath(journal.ownerRoot);
  }

  async selectDestination(): Promise<string | undefined> {
    if (this.destinationResolver !== undefined) {
      if (this.destinationHandle.length === 0) { throw new Error(shellString("shell_select_the_output_folder_in_the_original_dialog_first", "Select the output folder in the original dialog first")); }
      return await this.destinationResolver(this.destinationHandle, this.destinationTarget);
    }
    if (this.destinationHandle.length > 0) { throw new Error(shellString("shell_the_output_folder_permission_service_is_not_ready", "The output folder permission service is not ready")); }
    if (!canIUse('SystemCapability.FileManagement.UserFileService.FolderSelection') ||
      !canIUse('SystemCapability.FileManagement.AppFileService.FolderAuthorization')) {
      throw new Error(shellString("shell_this_device_does_not_support_authorized_folder_export", "This device does not support authorized folder export"));
    }
    const options = new picker.DocumentSelectOptions();
    options.selectMode = picker.DocumentSelectMode.FOLDER;
    options.maxSelectNumber = 1;
    const selected = await this.perform<string[]>('folder-picker', () =>
      new picker.DocumentViewPicker(this.context).select(options));
    if (selected.length === 0) { return undefined; }
    if (selected.length !== 1 || selected[0].length === 0) { throw new Error(shellString("shell_select_one_output_folder", "Select one output folder")); }
    return selected[0];
  }

  async authorizeDestination(uri: string): Promise<void> {
    const policies: fileShare.PolicyInfo[] = [{ uri: uri,
      operationMode: fileShare.OperationMode.READ_MODE | fileShare.OperationMode.WRITE_MODE }];
    // A form-selected destination was already persisted by FolderDestinationBridge.
    // Resolving its handle validates and activates that saved grant; the picker
    // temporary authorization is not required to survive until processing ends.
    // Only this adapter's fresh picker path may establish a new persistent grant.
    if (this.destinationResolver === undefined) {
      await this.perform<void>('folder-permission-persist', () => fileShare.persistPermission(policies));
    }
    const granted = await this.perform<boolean[]>('folder-permission-check', () =>
      fileShare.checkPersistentPermission(policies));
    if (granted.length !== 1 || !granted[0]) { throw new Error(shellString("shell_could_not_authorize_the_selected_output_folder", "Could not authorize the selected output folder")); }
    await this.perform<void>('folder-permission-activate', () => fileShare.activatePermission(policies));
    // Public API22 URI stat: do not decode this URI or manufacture a child URI.
    const stat = await this.perform<fileIo.Stat>('folder-destination-stat', () => fileIo.stat(uri));
    if (!stat.isDirectory()) { throw new Error(shellString("shell_the_selected_output_location_is_not_a_folder", "The selected output location is not a folder")); }
  }

  async copyTree(stageRoot: string, destinationUri: string,
    progress: (processedBytes: number, totalBytes: number) => void): Promise<void> {
    this.signal = new fileIo.TaskSignal();
    try {
      // SDK puts the complete named source directory beneath the existing destination directory.
      // No extra `contents` container, URI concatenation or external-path conversion is used.
      await this.perform<void>('folder-provider-copy', () => fileIo.copy(fileUri.getUriFromPath(stageRoot),
        destinationUri, { copySignal: this.signal, progressListener: (value: fileIo.Progress) =>
          progress(value.processedSize, value.totalSize) }));
    } finally {
      this.signal = undefined;
    }
  }

  cancelCopy(): void { if (this.signal !== undefined) { this.signal.cancel(); } }

  async discoverRecovery(): Promise<FolderPublicationRecovery[]> {
    const base = `${this.roots.filesDir}/PhotoCraft/Documents/FolderPublication`;
    if (!await fileIo.access(base)) { return []; }
    await this.ensureBase(false);
    const names = await fileIo.listFile(base);
    if (names.length > 128) { throw new Error(shellString("shell_too_many_retained_folder_output_tasks_to_inspect_resolve", "Too many retained folder output tasks to inspect. Resolve the retained copies first")); }
    const recoveries: FolderPublicationRecovery[] = [];
    for (const name of names) {
      if (!/^[0-9]+-[A-Za-z0-9]{6}$/.test(name)) {
        throw new Error(shellString("shell_the_folder_output_storage_contains_an_invalid_task_it", "The folder output storage contains an invalid task. It has been retained unchanged"));
      }
      const ownerRoot = `${base}/${name}`;
      const recovery: FolderPublicationRecovery = { id: -1, target: '', ownerRoot: ownerRoot, stageRoot: '',
        journalPath: `${ownerRoot}/publication.json`, publicationName: '', phase: 'unreadable', partialPossible: true,
        message: shellString("shell_an_incomplete_folder_output_record_was_found_the_local", "An incomplete folder output record was found. The local copy has been retained. Inspect the output folder and export again") };
      try {
        await this.requireDirectory(ownerRoot);
        const owner = await this.readOwner(ownerRoot);
        const journal = JSON.parse(await this.readMetadata(recovery.journalPath, 16384)) as FolderPublicationJournal;
        validateFolderPublicationIdentity(journal.id, journal.target);
        validateFolderPublicationPaths(this.roots.filesDir, ownerRoot, journal.stageRoot);
        if (journal.version !== 1 || journal.ownerRoot !== ownerRoot || journal.target !== owner.target ||
          typeof journal.partialPossible !== 'boolean' ||
          !['prepared', 'renamed', 'copying', 'complete', 'cancelled', 'failed'].includes(journal.phase) ||
          (journal.publicationName !== '' && !isFreshPublicationName(journal.publicationName))) {
          throw new Error(shellString("shell_invalid_folder_output_record", "Invalid folder output record"));
        }
        recovery.id = journal.id;
        recovery.target = journal.target;
        recovery.publicationName = journal.publicationName;
        recovery.phase = journal.phase;
        recovery.partialPossible = journal.partialPossible;
        if (journal.publicationName.length > 0) {
          recovery.stageRoot = await this.locateStage(ownerRoot, journal.stageRoot, journal.publicationName)
            .catch(() => journal.stageRoot);
        } else {
          recovery.stageRoot = journal.stageRoot;
        }
        await this.requireDirectory(recovery.stageRoot);
        recovery.message = journal.phase === 'complete' ? shellString("shell_a_local_copy_of_a_completed_folder_export_was", "A local copy of a completed folder export was found. Waiting for the original task to confirm") :
          shellString("shell_an_incomplete_folder_output_was_found_local_output_files", "An incomplete folder output was found. Local output files have been retained. Inspect the output folder and export again");
      } catch (_) { }
      recoveries.push(recovery);
    }
    return recoveries;
  }

  private async readOwner(ownerRoot: string): Promise<FolderPublicationOwner> {
    const owner = JSON.parse(await this.readMetadata(`${ownerRoot}/owner.json`, 4096)) as FolderPublicationOwner;
    validateFolderPublicationIdentity(owner.creationId, owner.target);
    if (owner.version !== 1 || owner.ownerRoot !== ownerRoot ||
      !ownerRoot.substring(ownerRoot.lastIndexOf('/') + 1).startsWith(`${owner.creationId}-`)) {
      throw new Error(shellString("shell_invalid_folder_output_task_ownership_record", "Invalid folder output task ownership record"));
    }
    return owner;
  }

  private async ensureBase(create: boolean): Promise<string> {
    let directory = this.roots.filesDir;
    for (const name of ['', 'PhotoCraft', 'Documents', 'FolderPublication']) {
      if (name.length > 0) {
        directory += `/${name}`;
        if (create && !await fileIo.access(directory)) {
          try { await fileIo.mkdir(directory); } catch (error) {
            if ((error as BusinessError).code !== 13900015) { throw error as Error; }
          }
        }
      }
      await this.requireDirectory(directory);
    }
    return directory;
  }

  private async requireDirectory(path: string): Promise<void> {
    if ((await this.inspect(path)).kind !== 'directory') {
      throw new Error(shellString("shell_the_folder_output_path_is_invalid_links_will_not", "The folder output path is invalid. Links will not be followed"));
    }
  }

  private async pathExists(path: string): Promise<boolean> {
    try {
      await fileIo.lstat(path);
      return true;
    } catch (error) {
      if ((error as BusinessError).code === 13900002) { return false; }
      throw error as Error;
    }
  }

  private async syncPath(path: string): Promise<void> {
    await this.perform<void>('folder-stage-fsync', async () => {
      const handle = await fileIo.open(path, fileIo.OpenMode.READ_ONLY | fileIo.OpenMode.NOFOLLOW);
      try { await fileIo.fsync(handle.fd); } finally { await fileIo.close(handle); }
    });
  }

  private async readMetadata(path: string, maxBytes: number): Promise<string> {
    const node = await this.inspect(path);
    if (node.kind !== 'file' || !Number.isSafeInteger(node.bytes) || node.bytes < 1 || node.bytes > maxBytes) {
      throw new Error(shellString("shell_the_folder_output_record_is_not_a_valid_regular", "The folder output record is not a valid regular file"));
    }
    return await fileIo.readText(path);
  }

  private async writeMetadata(path: string, value: string): Promise<void> {
    await this.perform<void>('folder-journal-write', async () => {
      const temporary = `${path}.next`;
      for (const location of [path, temporary]) {
        if (await this.pathExists(location) && (await this.inspect(location)).kind !== 'file') {
          throw new Error(shellString("shell_the_folder_output_record_contains_links_or_special_files", "The folder output record contains links or special files"));
        }
      }
      const handle = await fileIo.open(temporary,
        fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC | fileIo.OpenMode.NOFOLLOW);
      try {
        const bytes = new util.TextEncoder().encodeInto(value);
        if (await fileIo.write(handle.fd, bytes.buffer as ArrayBuffer) !== bytes.byteLength) {
          throw new Error(shellString("shell_the_folder_output_record_was_not_fully_written", "The folder output record was not fully written"));
        }
        await fileIo.fsync(handle.fd);
      } finally { await fileIo.close(handle); }
      await fileIo.rename(temporary, path);
    });
  }

  private async perform<T>(stage: string, action: () => Promise<T>): Promise<T> {
    try { return await action(); } catch (error) {
      const code = (error as BusinessError).code;
      hilog.error(0x0000, 'PhotoCraft', 'folder-publication stage=%{public}s code=%{public}d',
        stage, typeof code === 'number' ? code : 0);
      throw error as Error;
    }
  }
}

/** Call publishFolder without blocking platform control polling; cancel is independent. */
export class FolderPublicationBridge {
  private storage: PublicationFolderStorage;
  private transaction: FolderPublicationTransaction;

  constructor(context: common.UIAbilityContext,
    destinationResolver?: (handle: string, target: string) => Promise<string>,
    roots: AppStorageRoots = new AppStorageRoots(context)) {
    this.storage = new PublicationFolderStorage(context, destinationResolver, roots);
    this.transaction = new FolderPublicationTransaction(this.storage);
  }

  async createStage(id: number, target: string): Promise<FolderPublicationStage> {
    return await this.storage.createStage(id, target);
  }

  async publishFolder(request: FolderPublicationRequest,
    onProgress: (progress: FolderPublicationProgress) => void): Promise<FolderPublicationCompletion> {
    return await this.transaction.publish(request, onProgress);
  }

  cancel(id: number, target: string): boolean { return this.transaction.cancel(id, target); }

  async discoverRecovery(): Promise<FolderPublicationRecovery[]> {
    // Read-only discovery: no automatic retry, external cleanup or stage deletion after an interrupted job.
    return await this.storage.discoverRecovery();
  }
}
