import { shellString } from './ShellLocalization';
import { FolderLimits, FolderManifest, validateFolderLimits } from './SandboxFolderManifest';

export interface FolderPublicationRequest {
  kind: 'folderPublish';
  id: number;
  target: string;
  ownerRoot: string;
  stageRoot: string;
  limits: FolderLimits;
  policy: 'freshSubdirectory';
  destinationHandle?: string;
}

export interface FolderPublicationProgress {
  kind: 'folderPublishProgress';
  id: number;
  target: string;
  processedBytes: number;
  totalBytes: number;
  filesDone: number;
}

export interface FolderPublicationCompletion {
  kind: 'folderPublishComplete';
  id: number;
  target: string;
  result: 'success' | 'cancel' | 'error';
  ownerRoot: string;
  stageRoot: string;
  journalPath: string;
  publicationName: string;
  publishedCount: number;
  partialPossible: boolean;
  retainStage: boolean;
  error: string;
}

export interface FolderPublicationJournal {
  version: number;
  id: number;
  target: string;
  phase: 'prepared' | 'renamed' | 'copying' | 'complete' | 'cancelled' | 'failed';
  ownerRoot: string;
  stageRoot: string;
  publicationName: string;
  destinationUri: string;
  partialPossible: boolean;
  fileCount: number;
  totalBytes: number;
}

export interface FolderPublicationStorage {
  validateStage(request: FolderPublicationRequest, isCancelled: () => boolean): Promise<FolderManifest>;
  freshName(ownerRoot: string, previousRoot: string): Promise<string>;
  renameStage(source: string, destination: string): Promise<void>;
  locateStage(ownerRoot: string, previousRoot: string, publicationName: string): Promise<string>;
  syncOwner(ownerRoot: string): Promise<void>;
  writeJournal(journal: FolderPublicationJournal): Promise<void>;
  selectDestination(): Promise<string | undefined>;
  authorizeDestination(uri: string): Promise<void>;
  copyTree(stageRoot: string, destinationUri: string,
    progress: (processedBytes: number, totalBytes: number) => void): Promise<void>;
  cancelCopy(): void;
}

export function validateFolderPublicationIdentity(id: number, target: string): void {
  if (!Number.isSafeInteger(id) || id < 0 || typeof target !== 'string' || target.length < 1 ||
    target.length > 512 || target.includes('\x00')) {
    throw new Error(shellString("shell_invalid_folder_export_request_identity", "Invalid folder export request identity"));
  }
}

export function isFreshPublicationName(name: string): boolean {
  return /^PhotoCraft-Export-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(name);
}

/** Lexical checks precede every filesystem operation; the SDK adapter also lstat-checks each ancestor. */
export function validateFolderPublicationPaths(filesDir: string, ownerRoot: string, stageRoot: string): void {
  if (!filesDir.startsWith('/') || filesDir === '/' || filesDir.includes('\\') || filesDir.includes('\x00') ||
    filesDir.substring(1).split('/').some((part: string) => part.length === 0 || part === '.' || part === '..')) {
    throw new Error(shellString("shell_invalid_folder_export_sandbox_root", "Invalid folder export sandbox root"));
  }
  const base = `${filesDir}/PhotoCraft/Documents/FolderPublication/`;
  const jobName = ownerRoot.startsWith(base) ? ownerRoot.substring(base.length) : '';
  if (!/^[0-9]+-[A-Za-z0-9]{6}$/.test(jobName)) {
    throw new Error(shellString("shell_the_folder_export_does_not_belong_to_a_unique", "The folder export does not belong to a unique output task of this app"));
  }
  const name = stageRoot.startsWith(`${ownerRoot}/`) ? stageRoot.substring(ownerRoot.length + 1) : '';
  if (name !== 'contents' && !isFreshPublicationName(name)) {
    throw new Error(shellString("shell_the_folder_export_source_must_be_the_complete_output", "The folder export source must be the complete output tree of the task"));
  }
}

class FolderPublicationCancelled extends Error {
  constructor() { super(shellString("shell_folder_export_canceled", "Folder export canceled")); }
}

/** Never deletes a stage or an external URI. Its Promise settles only after the provider copy settles. */
export class FolderPublicationTransaction {
  private storage: FolderPublicationStorage;
  private activeId: number = -1;
  private activeTarget: string = '';
  private cancelled: boolean = false;
  private copying: boolean = false;
  private usedIds: Set<number> = new Set<number>();

  constructor(storage: FolderPublicationStorage) { this.storage = storage; }

  cancel(id: number, target: string): boolean {
    if (id !== this.activeId || target !== this.activeTarget) {
      return false;
    }
    this.cancelled = true;
    if (this.copying) {
      try { this.storage.cancelCopy(); } catch (_) { }
    }
    return true;
  }

  async publish(request: FolderPublicationRequest,
    onProgress: (progress: FolderPublicationProgress) => void): Promise<FolderPublicationCompletion> {
    const completion: FolderPublicationCompletion = { kind: 'folderPublishComplete', id: request.id,
      target: request.target, result: 'error', ownerRoot: '', stageRoot: '', journalPath: '', publicationName: '',
      publishedCount: 0, partialPossible: false, retainStage: true, error: '' };
    if (this.activeId >= 0) {
      completion.error = shellString("shell_another_folder_export_is_in_progress_wait_or_cancel", "Another folder export is in progress. Wait or cancel it and try again");
      return completion;
    }
    let journal: FolderPublicationJournal | undefined = undefined;
    let journalReady = false;
    let copyCompleted = false;
    let progressFailed = false;
    let lastProgressTime = 0;
    let previousRoot = '';
    try {
      validateFolderPublicationIdentity(request.id, request.target);
      validateFolderLimits(request.limits);
      if (request.kind !== 'folderPublish' || request.policy !== 'freshSubdirectory' || this.usedIds.has(request.id)) {
        throw new Error(shellString("shell_the_folder_export_request_is_invalid_or_already_processed", "The folder export request is invalid or already processed. Try again with a new request"));
      }
      if (this.usedIds.size >= 4096) {
        throw new Error(shellString("shell_too_many_folder_export_requests_in_this_session_reopen", "Too many folder export requests in this session. Reopen the app"));
      }
      this.usedIds.add(request.id);
      this.activeId = request.id;
      this.activeTarget = request.target;
      this.cancelled = false;
      const manifest = await this.storage.validateStage(request, () => this.cancelled);
      this.checkCancelled();
      if (manifest.fileCount === 0) {
        throw new Error(shellString("shell_the_folder_export_contains_no_output_files", "The folder export contains no output files"));
      }
      completion.ownerRoot = request.ownerRoot;
      completion.stageRoot = request.stageRoot;
      completion.journalPath = `${request.ownerRoot}/publication.json`;
      const name = await this.storage.freshName(request.ownerRoot, request.stageRoot);
      if (!isFreshPublicationName(name) || request.stageRoot === `${request.ownerRoot}/${name}`) {
        throw new Error(shellString("shell_could_not_generate_a_new_safe_output_folder_name", "Could not generate a new safe output folder name"));
      }
      completion.publicationName = name;
      journal = { version: 1, id: request.id, target: request.target, phase: 'prepared', ownerRoot: request.ownerRoot,
        stageRoot: request.stageRoot, publicationName: name, destinationUri: '', partialPossible: false,
        fileCount: manifest.fileCount, totalBytes: manifest.totalBytes };
      await this.storage.writeJournal(journal);
      journalReady = true;
      this.checkCancelled();
      previousRoot = request.stageRoot;
      const nextRoot = `${request.ownerRoot}/${name}`;
      await this.storage.renameStage(previousRoot, nextRoot);
      completion.stageRoot = nextRoot;
      journal.stageRoot = nextRoot;
      await this.storage.syncOwner(request.ownerRoot);
      journal.phase = 'renamed';
      await this.storage.writeJournal(journal);
      this.checkCancelled();
      const uri = await this.storage.selectDestination();
      this.checkCancelled();
      if (uri === undefined || uri.length === 0) {
        throw new FolderPublicationCancelled();
      }
      await this.storage.authorizeDestination(uri);
      this.checkCancelled();
      journal.destinationUri = uri;
      journal.phase = 'copying';
      journal.partialPossible = true;
      // Durable intent must precede provider mutation. On process loss preserve, never replay this journal.
      await this.storage.writeJournal(journal);
      this.checkCancelled();
      completion.partialPossible = true;
      this.copying = true;
      try {
        await this.storage.copyTree(nextRoot, uri, (processedBytes: number, totalBytes: number) => {
          if (this.cancelled || progressFailed) {
            return;
          }
          if (!Number.isSafeInteger(processedBytes) || !Number.isSafeInteger(totalBytes) ||
            processedBytes < 0 || totalBytes < 0 || processedBytes > totalBytes || processedBytes > request.limits.maxBytes ||
            totalBytes > request.limits.maxBytes) {
            progressFailed = true;
            try { this.storage.cancelCopy(); } catch (_) { }
            return;
          }
          try {
            const now = Date.now();
            if (lastProgressTime > 0 && now - lastProgressTime < 200) { return; }
            lastProgressTime = now;
            onProgress({ kind: 'folderPublishProgress', id: request.id, target: request.target,
              processedBytes: processedBytes, totalBytes: totalBytes, filesDone: 0 });
          } catch (_) {
            progressFailed = true;
            try { this.storage.cancelCopy(); } catch (_) { }
          }
        });
        copyCompleted = true;
      } finally {
        this.copying = false;
      }
      this.checkCancelled();
      if (progressFailed) {
        throw new Error(shellString("shell_could_not_process_folder_export_progress", "Could not process folder export progress"));
      }
      journal.phase = 'complete';
      journal.partialPossible = false;
      await this.storage.writeJournal(journal);
      this.checkCancelled();
      completion.result = 'success';
      completion.partialPossible = false;
      completion.publishedCount = manifest.fileCount;
      // Final notification is informational; a callback failure cannot undo an acknowledged provider copy.
      try {
        onProgress({ kind: 'folderPublishProgress', id: request.id, target: request.target,
          processedBytes: manifest.totalBytes, totalBytes: manifest.totalBytes, filesDone: manifest.fileCount });
      } catch (_) { }
      return completion;
    } catch (error) {
      completion.result = this.cancelled || error instanceof FolderPublicationCancelled ? 'cancel' : 'error';
      if (journal !== undefined && journalReady) {
        if (previousRoot.length > 0) {
          try {
            const located = await this.storage.locateStage(request.ownerRoot, previousRoot, journal.publicationName);
            completion.stageRoot = located;
            journal.stageRoot = located;
          } catch (_) {
            // Preserve the entire owner job; both candidate names are recorded in the prepared journal.
            completion.stageRoot = '';
          }
        }
        journal.phase = completion.result === 'cancel' ? 'cancelled' : 'failed';
        journal.partialPossible = completion.partialPossible;
        try {
          await this.storage.writeJournal(journal);
        } catch (_) {
          completion.result = 'error';
          completion.error = shellString("shell_folder_export_is_incomplete_and_the_recovery_record_could", "Folder export is incomplete and the recovery record could not be updated. Complete local output has been retained. Inspect the output folder and retry or choose another folder");
        }
      }
      if (completion.error.length === 0) {
        completion.error = completion.partialPossible ?
          (copyCompleted ? shellString("shell_folder_copying_finished_but_export_completion_could_not_be", "Folder copying finished, but export completion could not be confirmed. Complete output has been retained. Inspect the new output folder and retry") :
            shellString("shell_folder_export_is_incomplete_and_partial_results_may_remain", "Folder export is incomplete and partial results may remain. Complete output has been retained. Inspect the new output folder and retry or choose another folder")) :
          completion.result === 'cancel' ? shellString("shell_folder_export_was_canceled_complete_output_has_been_retained", "Folder export was canceled. Complete output has been retained and can be exported again") :
            shellString("shell_folder_export_did_not_start_complete_output_has_been", "Folder export did not start. Complete output has been retained. Check permissions or output files and retry");
      }
      return completion;
    } finally {
      this.activeId = -1;
      this.activeTarget = '';
      this.cancelled = false;
      this.copying = false;
    }
  }

  private checkCancelled(): void {
    if (this.cancelled) { throw new FolderPublicationCancelled(); }
  }
}
