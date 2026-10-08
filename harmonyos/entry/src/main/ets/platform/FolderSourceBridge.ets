import { shellString } from './ShellLocalization';
import { AppStorageRoots } from './AppStorageRoots';
import { common } from '@kit.AbilityKit';
import { fileIo, fileShare } from '@kit.CoreFileKit';
import { FolderImportBridge, FolderImportRequest, SandboxFolderStorage } from './FolderImportBridge';
import { FolderManifest, inspectSandboxFolder, validateFolderRelativePath } from './SandboxFolderManifest';

/** Private provenance. The source URI never enters Rust or a user-visible path. */
export interface FolderSourceBinding {
  folderUri: string;
  relativePath: string;
}

interface SourceMarker {
  version: number;
  requestId: number;
  target: string;
  bindingId: string;
  copyLayout: string;
  operationMode: number;
  sourceUri: string;
}

/** A scratch Batch source reads the real bound folder again, using the proven SDK copy layout. */
export class FolderSourceBridge {
  private context: common.UIAbilityContext;
  private roots: AppStorageRoots;
  private storage: SandboxFolderStorage = new SandboxFolderStorage();

  constructor(context: common.UIAbilityContext, roots: AppStorageRoots = new AppStorageRoots(context)) { this.context = context; this.roots = roots; }

  validate(binding: FolderSourceBinding): void {
    if (typeof binding.folderUri !== 'string' || binding.folderUri.length < 1 ||
      binding.folderUri.length > 8192 || binding.folderUri.includes('\x00') ||
      typeof binding.relativePath !== 'string') {
      throw new Error(shellString("shell_invalid_original_folder_source_record", "Invalid original folder source record"));
    }
    validateFolderRelativePath(binding.relativePath);
  }

  async discover(path: string): Promise<FolderSourceBinding | undefined> {
    const base = `${this.roots.filesDir}/PhotoCraft/Documents/folder-imports/`;
    if (!path.startsWith(base)) { return undefined; }
    const parts = path.substring(base.length).split('/');
    const match = parts[0].match(/^(\d+)-[A-Za-z0-9]{6}$/);
    if (match === null || parts.length < 4 || parts[1] !== 'contents' || path.length > 8192) {
      throw new Error(shellString("shell_invalid_original_folder_copy_identity", "Invalid original folder copy identity"));
    }
    const requestId = Number(match[1]);
    if (!Number.isSafeInteger(requestId) || requestId < 0) { throw new Error(shellString("shell_invalid_original_folder_request_identity", "Invalid original folder request identity")); }
    validateFolderRelativePath(parts.slice(2).join('/'));
    const owner = `${base}${parts[0]}`;
    const root = `${owner}/contents/${parts[2]}`;
    let ancestor = this.roots.filesDir;
    for (const part of ['', 'PhotoCraft', 'Documents', 'folder-imports', ...parts]) {
      if (part.length > 0) { ancestor += `/${part}`; }
      const node = await this.storage.inspect(ancestor);
      if (ancestor === path ? node.kind !== 'file' : node.kind !== 'directory') {
        throw new Error(shellString("shell_the_original_folder_copy_contains_links_or_invalid_entries", "The original folder copy contains links or invalid entries"));
      }
    }
    const marker = JSON.parse(await this.readRecord(`${owner}/source.json`, 65536)) as SourceMarker;
    if (marker.version !== 1 || marker.requestId !== requestId || marker.bindingId !== parts[0] ||
      marker.copyLayout !== 'sourceBasenameChild' || marker.operationMode !== fileShare.OperationMode.READ_MODE ||
      typeof marker.target !== 'string' || marker.target.length < 1 || marker.target.length > 512 || marker.target.includes('\x00')) {
      throw new Error(shellString("shell_the_original_folder_source_ownership_record_does_not_match", "The original folder source ownership record does not match"));
    }
    const manifest = JSON.parse(await this.readRecord(`${owner}/manifest.json`, 1024 * 1024)) as FolderManifest;
    const inspected = await inspectSandboxFolder(this.storage, root,
      { maxFiles: 500, maxBytes: 1024 * 1024 * 1024 }, () => false);
    // Original private manifest is deletion/read authority only if it still matches its owned tree.
    if (manifest.version !== 1 || manifest.fileCount !== inspected.fileCount || manifest.totalBytes !== inspected.totalBytes ||
      !Array.isArray(manifest.entries) || manifest.entries.length !== inspected.entries.length ||
      manifest.entries.some((entry, index) => entry.relativePath !== inspected.entries[index].relativePath ||
        entry.kind !== inspected.entries[index].kind || entry.bytes !== inspected.entries[index].bytes)) {
      throw new Error(shellString("shell_the_original_folder_copy_does_not_match_the_source", "The original folder copy does not match the source manifest"));
    }
    const binding: FolderSourceBinding = { folderUri: marker.sourceUri, relativePath: parts.slice(3).join('/') };
    this.validate(binding);
    return binding;
  }

  async fresh<T>(binding: FolderSourceBinding, requestId: number,
    consume: (path: string, name: string) => Promise<T>): Promise<T> {
    this.validate(binding);
    const service = new FolderImportBridge(this.context, this.roots);
    const request: FolderImportRequest = { id: requestId, target: `source-refresh-${requestId}`, kind: 'folderImport',
      intent: 'file.revert', recursive: true, copyLayout: 'sourceBasenameChild',
      limits: { maxFiles: 500, maxBytes: 1024 * 1024 * 1024 } };
    const completion = await service.reimportFolder(request, binding.folderUri);
    if (completion.result !== 'success') {
      throw new Error(completion.error || shellString("shell_could_not_read_the_original_folder_again_authorize_the", "Could not read the original folder again. Authorize the source again"));
    }
    try {
      const manifest = JSON.parse(await this.readRecord(completion.manifestPath, 1024 * 1024)) as FolderManifest;
      const selected = manifest.entries.find(entry => entry.relativePath === binding.relativePath && entry.kind === 'file');
      if (selected === undefined) { throw new Error(shellString("shell_the_source_file_in_the_original_folder_was_deleted", "The source file in the original folder was deleted or renamed. Select the source again")); }
      const path = `${completion.resolvedRoot}/${binding.relativePath}`;
      const node = await this.storage.inspect(path);
      if (node.kind !== 'file' || node.bytes !== selected.bytes) { throw new Error(shellString("shell_the_newly_read_source_file_is_invalid", "The newly read source file is invalid")); }
      return await consume(path, binding.relativePath.substring(binding.relativePath.lastIndexOf('/') + 1));
    } finally {
      await service.releaseRetained(completion, request.id, request.target);
    }
  }

  private async readRecord(path: string, max: number): Promise<string> {
    const node = await this.storage.inspect(path);
    if (node.kind !== 'file' || node.bytes < 1 || node.bytes > max) { throw new Error(shellString("shell_the_folder_source_record_is_invalid_or_too_large", "The folder source record is invalid or too large")); }
    return await fileIo.readText(path);
  }
}
