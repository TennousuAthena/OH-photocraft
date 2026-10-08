import { shellString } from './ShellLocalization';
import { AppStorageRoots } from './AppStorageRoots';
import { common } from '@kit.AbilityKit';
import { util } from '@kit.ArkTS';
import { BusinessError } from '@kit.BasicServicesKit';
import { fileIo, fileShare, fileUri, picker } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { FolderSourceBridge, FolderSourceBinding } from './FolderSourceBridge';
import { PublicationFileStorage } from './PublicationFileStorage';
import { PublicationJournal, PublicationTransaction, publicationRecoveryNotice } from './PublicationTransaction';

export interface FileRequest {
  id: number;
  kind: 'open' | 'save';
  path: string;
  suggestedName: string;
  chooseDestination: boolean;
  previousPath: string;
  intent: string;
  allowFormatChoice: boolean;
  allowedExtensions: string[];
  retainSource?: boolean;
  bindToPrevious?: boolean;
}

interface SourceBinding {
  path: string;
  uri: string;
  folder?: FolderSourceBinding;
}

export class ImportedDocument {
  public path: string;
  public sourceUri: string;
  public originalName: string;
  public folderSource: FolderSourceBinding | undefined = undefined;

  constructor(path: string, sourceUri: string, originalName: string) {
    this.path = path;
    this.sourceUri = sourceUri;
    this.originalName = originalName;
  }
}

/** Picker/UDMF URIs stay in ArkTS; Rust receives retained sandbox files. */
export class DocumentBridge {
  private context: common.UIAbilityContext;
  private roots: AppStorageRoots;
  private importSequence: number = 0;
  private sourceUris: Map<string, string> = new Map<string, string>();
  private folderSources: Map<string, FolderSourceBinding> = new Map<string, FolderSourceBinding>();
  private folderBridge: FolderSourceBridge;
  private publicationWarning: string = '';

  constructor(context: common.UIAbilityContext, roots: AppStorageRoots = new AppStorageRoots(context)) {
    this.context = context; this.roots = roots;
    this.folderBridge = new FolderSourceBridge(context, roots);
  }

  async restoreBindings(): Promise<string> {
    // Recovery discovery must still run when an unrelated URI binding file is damaged.
    const recoveryNotice = await this.inspectPublicationRecovery();
    try {
      if (await fileIo.access(this.bindingsPath())) {
        await this.ensureSandboxDirectory(['PhotoCraft', 'Documents']);
        const info = await fileIo.lstat(this.bindingsPath());
        if (!info.isFile() || info.isSymbolicLink() || info.size > 1024 * 1024) {
          throw new Error(shellString("shell_the_recent_file_record_is_invalid_or_too_large", "The recent file record is invalid or too large. Open the file again"));
        }
        const bindings = JSON.parse(await fileIo.readText(this.bindingsPath())) as SourceBinding[];
        if (!Array.isArray(bindings) || bindings.length > 4096) {
          throw new Error(shellString("shell_the_recent_file_records_are_invalid_or_too_numerous", "The recent file records are invalid or too numerous. Open the file again"));
        }
        const restored: Map<string, string> = new Map<string, string>();
        const folders: Map<string, FolderSourceBinding> = new Map<string, FolderSourceBinding>();
        const seen: Set<string> = new Set<string>();
        for (const binding of bindings) {
          if (seen.has(binding.path)) { throw new Error(shellString("shell_duplicate_recent_file_source_record", "Duplicate recent file source record")); }
          seen.add(binding.path);
          if (typeof binding.path !== 'string' || typeof binding.uri !== 'string' ||
            binding.uri.length > 8192 || binding.uri.includes('\x00')) {
            throw new Error(shellString("shell_the_recent_file_source_record_is_invalid_open_the", "The recent file source record is invalid. Open the file again"));
          }
          await this.validateBindingPath(binding.path);
          if (binding.folder !== undefined) {
            if (binding.uri.length !== 0) { throw new Error(shellString("shell_a_folder_source_cannot_be_used_as_a_single", "A folder source cannot be used as a single file source")); }
            this.folderBridge.validate(binding.folder);
            folders.set(binding.path, binding.folder);
          } else {
            if (binding.uri.length === 0) { throw new Error(shellString("shell_the_recent_file_source_record_is_empty", "The recent file source record is empty")); }
            restored.set(binding.path, binding.uri);
          }
        }
        this.sourceUris = restored;
        this.folderSources = folders;
      }
    } catch (error) {
      throw new Error(`${(error as Error).message}${recoveryNotice.length > 0 ? `；${recoveryNotice}` : ''}`);
    }
    return recoveryNotice;
  }

  async openRequestedDocument(request: FileRequest): Promise<ImportedDocument | undefined> {
    if (request.kind !== 'open' || !Number.isSafeInteger(request.id) || request.id < 0) {
      throw new Error(shellString("shell_invalid_open_file_request", "Invalid open file request"));
    }
    if (request.intent === 'file.openRecent') {
      if (request.chooseDestination || request.previousPath.length === 0) {
        throw new Error(shellString("shell_the_recent_file_request_is_missing_its_original_source", "The recent file request is missing its original source"));
      }
    }
    if (!request.chooseDestination && request.previousPath.length > 0) {
      // Revert and linked Smart Object commands preserve their original intent,
      // but read fresh bytes through the same retained URI authority as Recent.
      return await this.reopenSource(request.previousPath, request.id);
    }
    return await this.selectDocument(request.id, request.intent);
  }

  async selectDocument(requestId: number, intent: string): Promise<ImportedDocument | undefined> {
    const documentPicker = new picker.DocumentViewPicker(this.context);
    const options = new picker.DocumentSelectOptions();
    options.fileSuffixFilters = this.filtersForIntent(intent);
    options.maxSelectNumber = 1;
    const uris = await documentPicker.select(options);
    return uris.length > 0 ? await this.importUri(uris[0], requestId) : undefined;
  }

  private filtersForIntent(intent: string): string[] {
    if (intent === 'file.import.notes') {
      // notes_cmds imports annotations from a PSD/PSB or native document.
      return [shellString("shell_documents_with_notes", "Documents with notes|.pcraft,.psd,.psb")];
    }
    if (intent === 'file.scripts.browse') {
      // automate_cmds accepts a JSON action or command.id {json} text lines.
      return [shellString("shell_photocraft_scripts", "PhotoCraft scripts|.json,.jsonl,.txt"), shellString("shell_all_files", "All files|.*")];
    }
    if (intent === 'edit.presets.exportImportPresets') {
      return [shellString("shell_photocraft_presets", "PhotoCraft presets|.pcpresets,.json")];
    }
    if (intent !== 'file.open' && intent !== 'file.openAs' &&
      intent !== 'file.placeEmbedded' && intent !== 'file.placeLinked' &&
      intent !== 'layer.smartObjects.replaceContents' && intent !== 'layer.smartObjects.relinkToFile') {
      return [shellString("shell_all_files", "All files|.*")];
    }
    // Matches photocraft-io and the enabled codecs. AVIF has no decoder.
    return [
      shellString("shell_all_supported_images_and_projects", "All supported images and projects|.pcraft,.psd,.psb,.png,.apng,.jpg,.jpeg,.jpe,.jfif,.tif,.tiff,.webp,.gif,.bmp,.dib,.tga,.icb,.vda,.vst,.ico,.pnm,.pbm,.pgm,.ppm,.pam,.pfm,.qoi,.exr,.hdr,.dng,.cr2,.cr3,.nef,.nrw,.arw,.srf,.sr2,.pef,.orf,.rw2,.raf,.abr,.grd"),
      'PhotoCraft / Photoshop|.pcraft,.psd,.psb',
      shellString("shell_common_images", "Common images|.png,.apng,.jpg,.jpeg,.jpe,.jfif,.tif,.tiff,.webp,.gif,.bmp,.dib"),
      shellString("shell_other_images", "Other images|.tga,.icb,.vda,.vst,.ico,.pnm,.pbm,.pgm,.ppm,.pam,.pfm,.qoi,.exr,.hdr"),
      shellString("shell_camera_raw_selected_formats", "Camera RAW (selected formats)|.dng,.cr2,.cr3,.nef,.nrw,.arw,.srf,.sr2,.pef,.orf,.rw2,.raf")
    ];
  }

  async importUri(sourceUri: string, requestId: number = 0): Promise<ImportedDocument> {
    return await this.importSource(sourceUri, requestId, true);
  }

  /** Recent always reads the external source again. The retained cache is never a freshness fallback. */
  async reopenSource(previousPath: string, requestId: number): Promise<ImportedDocument> {
    const folder = this.folderSources.get(previousPath) ?? await this.folderBridge.discover(previousPath);
    if (folder !== undefined) {
      return await this.folderBridge.fresh(folder, requestId, async (path: string, name: string): Promise<ImportedDocument> => {
        const directory = await this.ensureSandboxDirectory(['PhotoCraft', 'Documents', 'imports',
          `${requestId}-${Date.now()}-${this.importSequence++}`]);
        const localPath = `${directory}/${this.safeFileName(name)}`;
        try { await this.copyFile(path, localPath, true); } catch (error) {
          try { await this.removeTemporaryFile(localPath); } catch (_) { }
          throw error as Error;
        }
        const imported = new ImportedDocument(localPath, '', name);
        imported.folderSource = folder;
        return imported;
      });
    }
    const sourceUri = this.sourceUris.get(previousPath);
    if (sourceUri === undefined) {
      throw new Error(shellString("shell_the_file_source_record_is_unavailable_select_the_source", "The file source record is unavailable. Select the source file again"));
    }
    const policies: fileShare.PolicyInfo[] = [{ uri: sourceUri, operationMode: fileShare.OperationMode.READ_MODE }];
    if (canIUse('SystemCapability.FileManagement.AppFileService.FolderAuthorization')) {
      const granted = await fileShare.checkPersistentPermission(policies);
      if (granted.length !== 1 || !granted[0]) {
        throw new Error(shellString("shell_permission_to_read_the_file_source_has_expired_select", "Permission to read the file source has expired. Select the source file again"));
      }
      await fileShare.activatePermission(policies);
    }
    return await this.importSource(sourceUri, requestId, false);
  }

  private async importSource(sourceUri: string, requestId: number, persist: boolean): Promise<ImportedDocument> {
    const originalName = this.fileName(sourceUri);
    const importDirectory = await this.ensureSandboxDirectory(['PhotoCraft', 'Documents', 'imports',
      `${requestId}-${Date.now()}-${this.importSequence++}`]);
    const localPath = `${importDirectory}/${originalName}`;
    try {
      await this.copyFile(sourceUri, localPath, true);
    } catch (error) {
      try {
        await this.removeTemporaryFile(localPath);
      } catch (_) {
        // Preserve the original copy error if no partial file was created.
      }
      throw error as Error;
    }
    if (persist) { await this.persistUri(sourceUri, fileShare.OperationMode.READ_MODE); }
    return new ImportedDocument(localPath, sourceUri, originalName);
  }

  async bindSource(localPath: string, sourceUri: string): Promise<void> {
    await this.validateBindingPath(localPath);
    if (sourceUri.length === 0 || sourceUri.length > 8192 || sourceUri.includes('\x00')) {
      throw new Error(shellString("shell_invalid_recent_file_source", "Invalid recent file source"));
    }
    this.folderSources.delete(localPath);
    this.sourceUris.set(localPath, sourceUri);
    await this.writeBindings();
  }

  async bindImportedSource(localPath: string, imported: ImportedDocument): Promise<void> {
    if (imported.folderSource === undefined) {
      await this.bindSource(localPath, imported.sourceUri);
      return;
    }
    await this.validateBindingPath(localPath);
    this.folderBridge.validate(imported.folderSource);
    this.sourceUris.delete(localPath);
    this.folderSources.set(localPath, imported.folderSource);
    await this.writeBindings();
  }

  destinationFor(previousPath: string): string | undefined {
    return this.sourceUris.get(previousPath);
  }

  async forgetSource(previousPath: string): Promise<void> {
    this.sourceUris.delete(previousPath);
    this.folderSources.delete(previousPath);
    await this.writeBindings();
  }

  async selectSaveDestination(suggestedName: string, allowFormatChoice: boolean,
    allowedExtensions: string[]): Promise<string | undefined> {
    const safeName = this.safeFileName(suggestedName);
    const extension = this.extension(safeName);
    if (extension.length === 0) {
      throw new Error(shellString("shell_the_save_request_is_missing_the_encoded_file_extension", "The save request is missing the encoded file extension"));
    }
    const documentPicker = new picker.DocumentViewPicker(this.context);
    const options = new picker.DocumentSaveOptions();
    options.newFileNames = [safeName];
    options.fileSuffixChoices = this.saveChoices(extension, allowFormatChoice, allowedExtensions);
    const uris = await documentPicker.save(options);
    if (uris.length === 0) {
      return undefined;
    }
    await this.persistUri(uris[0], fileShare.OperationMode.READ_MODE | fileShare.OperationMode.WRITE_MODE);
    return uris[0];
  }

  private saveChoices(extension: string, allowFormatChoice: boolean, allowedExtensions: string[]): string[] {
    if (!allowFormatChoice) {
      return [shellString("shell_detail_file", "{0} file|.{1}", extension.toUpperCase(), extension)];
    }
    // upstream SAVE_FILTERS. Separate PSD/PSB suffixes for DocumentSaveOptions.
    const extensions: string[] = ['psd', 'psb', 'pcraft', 'png', 'jpg', 'tif', 'tga', 'exr'];
    const names: string[] = ['Photoshop PSD', 'Photoshop PSB', 'PhotoCraft', 'PNG', 'JPEG', 'TIFF', 'Targa', 'OpenEXR'];
    const choices: string[] = [];
    for (let index = 0; index < extensions.length; index++) {
      const format = extensions[index];
      if (!allowedExtensions.includes(format)) {
        continue;
      }
      const choice = `${names[index]}|.${format}`;
      if (format === extension) {
        choices.unshift(choice);
      } else {
        choices.push(choice);
      }
    }
    if (choices.length === 0) {
      throw new Error(shellString("shell_the_save_request_has_no_available_output_format", "The save request has no available output format"));
    }
    return choices;
  }

  destinationName(destinationUri: string): string {
    return this.fileName(destinationUri);
  }

  async publishDocument(localPath: string, destinationUri: string): Promise<string> {
    this.publicationWarning = '';
    let phase = 'validate-format';
    try {
      const destinationName = this.fileName(destinationUri);
      if (this.extension(localPath) !== this.extension(destinationName)) {
        throw new Error(shellString("shell_the_saved_file_extension_must_be_detail", "The saved file extension must be .{0}", this.extension(localPath)));
      }
      phase = 'activate-permission';
      await this.activatePublicationPermission(destinationUri);
      phase = 'create-recovery-root';
      const root = await this.ensureSandboxDirectory(['PhotoCraft', 'Documents', 'PublicationRecovery']);
      phase = 'create-recovery-job';
      // CoreFileKit passes this template to uv_fs_mkdtemp; the six X characters are required.
      const directory = await fileIo.mkdtemp(`${root}/publication-XXXXXX`);
      const backupPath = `${directory}/old-${destinationName}`;
      const retainedPath = `${directory}/new-${this.safeFileName(localPath.substring(localPath.lastIndexOf('/') + 1))}`;
      const publisher = new PublicationTransaction(new PublicationFileStorage(directory));
      phase = 'publication-transaction';
      this.publicationWarning = (await publisher.publish(localPath, destinationUri, backupPath, retainedPath)).warning;
      return destinationName;
    } catch (error) {
      // Log only a fixed stage and numeric code: no document names, URI, paths, or text.
      hilog.error(0x0000, 'PhotoCraft', 'Publication failure stage=%{public}s code=%{public}d',
        phase, (error as BusinessError).code || 0);
      throw error as Error;
    }
  }

  takePublicationWarning(): string {
    const message = this.publicationWarning;
    this.publicationWarning = '';
    return message;
  }

  private publicationRecoveryRoot(): string {
    return `${this.roots.filesDir}/PhotoCraft/Documents/PublicationRecovery`;
  }

  private async inspectPublicationRecovery(): Promise<string> {
    const root = this.publicationRecoveryRoot();
    if (!await fileIo.access(root)) {
      return '';
    }
    const phases: string[] = [];
    let unreadable = 0;
    for (const directory of await fileIo.listFile(root)) {
      if (directory.includes('/') || directory.includes('\\') || directory === '.' || directory === '..') {
        continue;
      }
      const journalPath = `${root}/${directory}/journal.json`;
      if (!await fileIo.access(journalPath)) {
        continue;
      }
      try {
        if ((await fileIo.stat(journalPath)).size > 64 * 1024) {
          throw new Error(shellString("shell_invalid_recovery_record_size", "Invalid recovery record size"));
        }
        const journal = JSON.parse(await fileIo.readText(journalPath)) as PublicationJournal;
        if (journal.version !== 1 || typeof journal.phase !== 'string') {
          throw new Error(shellString("shell_invalid_recovery_record_format", "Invalid recovery record format"));
        }
        phases.push(journal.phase);
      } catch (_) {
        unreadable++;
      }
    }
    // Discovery is deliberately read-only. Never replay an external URI write after a restart.
    return publicationRecoveryNotice(phases, unreadable);
  }

  async removeTemporaryFile(localPath: string): Promise<void> {
    if (await fileIo.access(localPath)) {
      await fileIo.unlink(localPath);
    }
  }

  private bindingsPath(): string {
    return `${this.roots.filesDir}/PhotoCraft/Documents/source-uris.json`;
  }

  private async validateBindingPath(path: string): Promise<void> {
    const root = `${this.roots.filesDir}/PhotoCraft/Documents`;
    const relative = path.startsWith(`${root}/`) ? path.substring(root.length + 1) : '';
    const names = relative.split('/');
    if (path.length > 8192 || names.length !== 3 || !['imports', 'staging'].includes(names[0]) ||
      names.some((name: string) => name.length === 0 || name === '.' || name === '..' ||
        name.includes('\\') || name.includes('\x00'))) {
      throw new Error(shellString("shell_the_recent_file_is_outside_the_app_document_storage", "The recent file is outside the app document storage"));
    }
    let current = this.roots.filesDir;
    for (const name of ['PhotoCraft', 'Documents', ...names]) {
      current += `/${name}`;
      if (!await fileIo.access(current)) { break; }
      const info = await fileIo.lstat(current);
      if (info.isSymbolicLink() || (current !== path && !info.isDirectory())) {
        throw new Error(shellString("shell_the_recent_file_storage_contains_an_invalid_directory_or", "The recent file storage contains an invalid directory or link"));
      }
    }
  }

  /** Check each existing ancestor without following a link; mkdir never relies on recursive EEXIST behavior. */
  private async ensureSandboxDirectory(names: string[]): Promise<string> {
    let directory = this.roots.filesDir;
    let info = await fileIo.lstat(directory);
    if (!info.isDirectory() || info.isSymbolicLink()) {
      throw new Error(shellString("shell_the_app_file_storage_directory_is_invalid_cannot_save", "The app file storage directory is invalid. Cannot save safely"));
    }
    for (const name of names) {
      if (name.length === 0 || name === '.' || name === '..' || name.includes('/') ||
        name.includes('\\') || name.includes('\0')) {
        throw new Error(shellString("shell_invalid_app_file_storage_directory", "Invalid app file storage directory"));
      }
      directory += `/${name}`;
      if (!await fileIo.access(directory)) {
        try {
          await fileIo.mkdir(directory);
        } catch (error) {
          // A concurrent owner may have created it after access; only a real directory is accepted below.
          if ((error as BusinessError).code !== 13900015) {
            throw error as Error;
          }
        }
      }
      info = await fileIo.lstat(directory);
      if (!info.isDirectory() || info.isSymbolicLink()) {
        throw new Error(shellString("shell_the_app_file_storage_directory_is_invalid_cannot_save", "The app file storage directory is invalid. Cannot save safely"));
      }
    }
    return directory;
  }

  private async writeBindings(): Promise<void> {
    await this.ensureSandboxDirectory(['PhotoCraft', 'Documents']);
    const bindings: SourceBinding[] = [];
    this.sourceUris.forEach((uri: string, path: string) => { bindings.push({ path: path, uri: uri }); });
    this.folderSources.forEach((folder: FolderSourceBinding, path: string) => { bindings.push({ path: path, uri: '', folder: folder }); });
    const text = JSON.stringify(bindings);
    if (bindings.length > 4096 || new util.TextEncoder().encodeInto(text).byteLength > 1024 * 1024) {
      throw new Error(shellString("shell_too_many_recent_file_source_records_clear_the_history", "Too many recent file source records. Clear the history and open the file again"));
    }
    const temporaryPath = `${this.bindingsPath()}.tmp`;
    const file = await fileIo.open(temporaryPath,
      fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC | fileIo.OpenMode.NOFOLLOW);
    try {
      const bytes = new util.TextEncoder().encodeInto(text);
      if (await fileIo.write(file.fd, bytes.buffer as ArrayBuffer) !== bytes.byteLength) {
        throw new Error(shellString("shell_the_file_destination_record_was_not_fully_written", "The file destination record was not fully written"));
      }
      await fileIo.fsync(file.fd);
    } finally {
      await fileIo.close(file);
    }
    await fileIo.rename(temporaryPath, this.bindingsPath());
    const directory = await fileIo.open(`${this.roots.filesDir}/PhotoCraft/Documents`,
      fileIo.OpenMode.READ_ONLY | fileIo.OpenMode.NOFOLLOW);
    try { await fileIo.fsync(directory.fd); } finally { await fileIo.close(directory); }
  }

  private async persistUri(uri: string, mode: number): Promise<void> {
    if (!canIUse('SystemCapability.FileManagement.AppFileService.FolderAuthorization')) {
      return;
    }
    try {
      const policies: fileShare.PolicyInfo[] = [{ uri: uri, operationMode: mode }];
      const granted = await fileShare.checkPersistentPermission(policies);
      if (granted.length === 1 && granted[0]) {
        await fileShare.activatePermission(policies);
      } else {
        await fileShare.persistPermission(policies);
      }
    } catch (error) {
      // Some providers only grant temporary access; the stage remains usable.
      hilog.warn(0x0000, 'PhotoCraft', 'URI permission could not be persisted: %{public}s',
        (error as BusinessError).message);
    }
  }

  private async activatePublicationPermission(uri: string): Promise<void> {
    if (!canIUse('SystemCapability.FileManagement.AppFileService.FolderAuthorization')) {
      return;
    }
    // Publishing also reads the old target. Reactivate both saved grants after an app restart.
    const policies: fileShare.PolicyInfo[] = [{ uri: uri,
      operationMode: fileShare.OperationMode.READ_MODE | fileShare.OperationMode.WRITE_MODE }];
    try {
      const persisted = await fileShare.checkPersistentPermission(policies);
      if (persisted[0]) {
        await fileShare.activatePermission(policies);
      }
    } catch (error) {
      // A fresh picker grant can still work when its provider has no persistence.
      // fileIo.open below remains the authority for actual read/write permission.
      hilog.warn(0x0000, 'PhotoCraft', 'URI permission could not be activated: %{public}s',
        (error as BusinessError).message);
    }
  }

  private fileName(uri: string): string {
    return this.safeFileName(new fileUri.FileUri(uri).name);
  }

  private safeFileName(name: string): string {
    const safeName = name.replace(/[\\/\x00]/g, '_');
    if (safeName.length === 0 || safeName === '.' || safeName === '..') {
      throw new Error(shellString("shell_invalid_file_name", "Invalid file name"));
    }
    return safeName;
  }

  private extension(path: string): string {
    const baseName = path.substring(path.lastIndexOf('/') + 1);
    const dot = baseName.lastIndexOf('.');
    return dot >= 0 ? baseName.substring(dot + 1).toLowerCase() : '';
  }

  private async copyFile(source: string, destination: string, createDestination: boolean): Promise<void> {
    const sourceFile = await fileIo.open(source, fileIo.OpenMode.READ_ONLY);
    try {
      const mode = fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.TRUNC |
        (createDestination ? fileIo.OpenMode.CREATE : 0);
      const destinationFile = await fileIo.open(destination, mode);
      try {
        await fileIo.copyFile(sourceFile.fd, destinationFile.fd);
        await fileIo.fsync(destinationFile.fd);
      } finally {
        await fileIo.close(destinationFile);
      }
    } finally {
      await fileIo.close(sourceFile);
    }
  }
}
