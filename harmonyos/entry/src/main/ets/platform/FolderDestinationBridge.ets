import { shellString } from './ShellLocalization';
import { AppStorageRoots } from './AppStorageRoots';
import { common } from '@kit.AbilityKit';
import { util } from '@kit.ArkTS';
import { BusinessError } from '@kit.BasicServicesKit';
import { fileIo, fileShare, picker } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { validateFolderPublicationIdentity } from './FolderPublicationTransaction';

export type FolderOutputIntent = 'file.scripts.imageProcessor' | 'file.automate.batch' | 'file.automate.lensCorrection';

export function isFolderOutputIntent(intent: string): boolean {
  return intent === 'file.scripts.imageProcessor' || intent === 'file.automate.batch' || intent === 'file.automate.lensCorrection';
}

export interface FolderDestinationRequest {
  kind: 'folderDestination';
  id: number;
  target: string;
  intent: FolderOutputIntent;
}

export interface FolderDestinationCompletion {
  kind: 'folderDestinationComplete';
  id: number;
  target: string;
  result: 'success' | 'cancel' | 'error';
  destinationHandle: string;
  error: string;
}

interface FolderDestinationRecord {
  version: number;
  id: number;
  target: string;
  handle: string;
  uri: string;
}

export function isFolderDestinationHandle(handle: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(handle);
}

/** Authorize an explicitly selected folder; URI stays in a private durable record behind an opaque handle. */
export class FolderDestinationBridge {
  private context: common.UIAbilityContext;
  private roots: AppStorageRoots;
  private activeId: number = -1;
  private activeTarget: string = '';
  private cancelled: boolean = false;

  constructor(context: common.UIAbilityContext, roots: AppStorageRoots = new AppStorageRoots(context)) { this.context = context; this.roots = roots; }

  cancel(id: number, target: string): boolean {
    if (id !== this.activeId || target !== this.activeTarget) { return false; }
    this.cancelled = true;
    return true;
  }

  async select(request: FolderDestinationRequest): Promise<FolderDestinationCompletion> {
    const completion: FolderDestinationCompletion = { kind: 'folderDestinationComplete', id: request.id,
      target: request.target, result: 'error', destinationHandle: '', error: '' };
    if (this.activeId >= 0) {
      completion.error = shellString("shell_another_folder_selection_is_in_progress_complete_or_cancel", "Another folder selection is in progress. Complete or cancel it and try again");
      return completion;
    }
    try {
      validateFolderPublicationIdentity(request.id, request.target);
      if (request.kind !== 'folderDestination' || !isFolderOutputIntent(request.intent)) {
        throw new Error(shellString("shell_this_dialog_does_not_support_folder_output", "This dialog does not support folder output"));
      }
      this.activeId = request.id;
      this.activeTarget = request.target;
      this.cancelled = false;
      if (!canIUse('SystemCapability.FileManagement.UserFileService.FolderSelection') ||
        !canIUse('SystemCapability.FileManagement.AppFileService.FolderAuthorization')) {
        throw new Error(shellString("shell_this_device_does_not_support_authorized_folder_export", "This device does not support authorized folder export"));
      }
      const options = new picker.DocumentSelectOptions();
      options.selectMode = picker.DocumentSelectMode.FOLDER;
      options.maxSelectNumber = 1;
      const selected = await this.perform<string[]>('destination-picker', () =>
        new picker.DocumentViewPicker(this.context).select(options));
      if (this.cancelled || selected.length === 0) { completion.result = 'cancel'; return completion; }
      if (selected.length !== 1 || typeof selected[0] !== 'string' || selected[0].length === 0 ||
        selected[0].includes('\x00')) { throw new Error(shellString("shell_select_one_output_folder", "Select one output folder")); }
      await this.authorize(selected[0], true);
      if (this.cancelled) { completion.result = 'cancel'; return completion; }
      const base = await this.ensureBase(true);
      if ((await fileIo.listFile(base)).length >= 4096) { throw new Error(shellString("shell_too_many_folder_selection_records_select_again_or_clear", "Too many folder selection records. Select again or clear completed tasks")); }
      let handle = '';
      for (let count = 0; count < 8; count++) {
        const candidate = util.generateRandomUUID(true);
        if (!isFolderDestinationHandle(candidate)) { throw new Error(shellString("shell_invalid_system_generated_folder_identifier", "Invalid system generated folder identifier")); }
        if (!await this.exists(`${base}/${candidate}.json`) && !await this.exists(`${base}/${candidate}.json.next`)) {
          handle = candidate;
          break;
        }
      }
      if (handle.length === 0) { throw new Error(shellString("shell_could_not_create_a_unique_folder_selection_record", "Could not create a unique folder selection record")); }
      const record: FolderDestinationRecord = { version: 1, id: request.id, target: request.target,
        handle: handle, uri: selected[0] };
      await this.writeRecord(`${base}/${handle}.json`, record);
      if (this.cancelled) { await this.release(handle, request.target); completion.result = 'cancel'; return completion; }
      completion.destinationHandle = handle;
      completion.result = 'success';
      return completion;
    } catch (error) {
      completion.result = 'error';
      const code = (error as BusinessError).code;
      completion.error = typeof code === 'number' ? shellString("shell_could_not_authorize_the_selected_output_folder_error_detail", "Could not authorize the selected output folder (error {0}). Select it again", code) :
        (error as Error).message || shellString("shell_could_not_select_the_output_folder_try_again", "Could not select the output folder. Try again");
      return completion;
    } finally {
      this.activeId = -1;
      this.activeTarget = '';
      this.cancelled = false;
    }
  }

  async resolve(handle: string, target: string): Promise<string> {
    const record = await this.readRecord(handle, target);
    await this.authorize(record.uri, false);
    return record.uri;
  }

  async release(handle: string, target: string): Promise<void> {
    await this.readRecord(handle, target);
    const base = await this.ensureBase(false);
    await this.perform<void>('destination-record-release', () => fileIo.unlink(`${base}/${handle}.json`));
    await this.syncDirectory(base);
    // Never revoke a permission that may also be shared with another selected destination.
  }

  private async readRecord(handle: string, target: string): Promise<FolderDestinationRecord> {
    if (!isFolderDestinationHandle(handle) || typeof target !== 'string' || target.length < 1 ||
      target.length > 512 || target.includes('\x00')) { throw new Error(shellString("shell_invalid_folder_selection_identifier_select_the_folder_again", "Invalid folder selection identifier. Select the folder again")); }
    const base = await this.ensureBase(false);
    const location = `${base}/${handle}.json`;
    const stat = await fileIo.lstat(location);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size < 1 || stat.size > 65536) {
      throw new Error(shellString("shell_invalid_folder_selection_record_select_the_folder_again", "Invalid folder selection record. Select the folder again"));
    }
    const record = JSON.parse(await fileIo.readText(location)) as FolderDestinationRecord;
    validateFolderPublicationIdentity(record.id, record.target);
    if (record.version !== 1 || record.target !== target || record.handle !== handle || typeof record.uri !== 'string' ||
      record.uri.length === 0 || record.uri.includes('\x00')) { throw new Error(shellString("shell_the_folder_selection_belongs_to_another_task_select_the", "The folder selection belongs to another task. Select the folder again")); }
    return record;
  }

  private async authorize(uri: string, persist: boolean): Promise<void> {
    const policies: fileShare.PolicyInfo[] = [{ uri: uri,
      operationMode: fileShare.OperationMode.READ_MODE | fileShare.OperationMode.WRITE_MODE }];
    if (persist) { await this.perform<void>('destination-permission-persist', () => fileShare.persistPermission(policies)); }
    const granted = await this.perform<boolean[]>('destination-permission-check', () =>
      fileShare.checkPersistentPermission(policies));
    if (granted.length !== 1 || !granted[0]) { throw new Error(shellString("shell_output_folder_permission_has_expired_select_the_folder_again", "Output folder permission has expired. Select the folder again")); }
    await this.perform<void>('destination-permission-activate', () => fileShare.activatePermission(policies));
    const stat = await this.perform<fileIo.Stat>('destination-stat', () => fileIo.stat(uri));
    if (!stat.isDirectory()) { throw new Error(shellString("shell_the_selected_output_location_is_not_a_folder_select", "The selected output location is not a folder. Select it again")); }
  }

  private async ensureBase(create: boolean): Promise<string> {
    let directory = this.roots.filesDir;
    for (const part of ['', 'PhotoCraft', 'Documents', 'FolderDestinations']) {
      if (part.length > 0) {
        directory += `/${part}`;
        if (create && !await fileIo.access(directory)) {
          try { await fileIo.mkdir(directory); } catch (error) {
            if ((error as BusinessError).code !== 13900015) { throw error as Error; }
          }
        }
      }
      const stat = await fileIo.lstat(directory);
      if (!stat.isDirectory() || stat.isSymbolicLink()) { throw new Error(shellString("shell_invalid_folder_selection_record_path", "Invalid folder selection record path")); }
    }
    return directory;
  }

  private async exists(location: string): Promise<boolean> {
    try { await fileIo.lstat(location); return true; } catch (error) {
      if ((error as BusinessError).code === 13900002) { return false; }
      throw error as Error;
    }
  }

  private async writeRecord(location: string, record: FolderDestinationRecord): Promise<void> {
    const bytes = new util.TextEncoder().encodeInto(JSON.stringify(record));
    if (bytes.byteLength > 65536) { throw new Error(shellString("shell_the_folder_selection_record_exceeds_the_size_limit", "The folder selection record exceeds the size limit")); }
    await this.perform<void>('destination-record-write', async () => {
      const handle = await fileIo.open(`${location}.next`,
        fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.NOFOLLOW);
      try {
        if (await fileIo.write(handle.fd, bytes.buffer as ArrayBuffer) !== bytes.byteLength) {
          throw new Error(shellString("shell_the_folder_selection_record_was_not_fully_written", "The folder selection record was not fully written"));
        }
        await fileIo.fsync(handle.fd);
      } finally { await fileIo.close(handle); }
      await fileIo.rename(`${location}.next`, location);
      await this.syncDirectory(location.substring(0, location.lastIndexOf('/')));
    });
  }

  private async syncDirectory(location: string): Promise<void> {
    const handle = await fileIo.open(location, fileIo.OpenMode.READ_ONLY | fileIo.OpenMode.NOFOLLOW);
    try { await fileIo.fsync(handle.fd); } finally { await fileIo.close(handle); }
  }

  private async perform<T>(phase: string, action: () => Promise<T>): Promise<T> {
    try { return await action(); } catch (error) {
      const code = (error as BusinessError).code;
      hilog.error(0x0000, 'PhotoCraft', 'folder-destination phase=%{public}s code=%{public}d',
        phase, typeof code === 'number' ? code : 0);
      throw error as Error;
    }
  }
}
