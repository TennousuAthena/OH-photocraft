import { shellString } from './ShellLocalization';
export interface PublicationJournal {
  version: number;
  phase: string;
  destinationUri: string;
  stagePath: string;
  backupPath: string;
  retainedStagePath: string;
  error: string;
  rollbackError: string;
}

/** Locations are opaque to this state machine; only the host opens a granted URI. */
export interface PublicationStorage {
  openRead(location: string): Promise<number>;
  openWrite(location: string, create: boolean): Promise<number>;
  close(fd: number): Promise<void>;
  copy(source: number, destination: number): Promise<void>;
  sync(fd: number): Promise<void>;
  prepareWrite(fd: number): Promise<void>;
  truncate(fd: number): Promise<void>;
  move(source: string, destination: string): Promise<void>;
  remove(location: string): Promise<void>;
  writeJournal(journal: PublicationJournal): Promise<void>;
  clearJournal(): Promise<void>;
}

export class PublicationFailure extends Error {
  public retainStagePath: string;

  constructor(message: string, retainStagePath: string = '') {
    super(message);
    this.retainStagePath = retainStagePath;
  }
}

export interface PublicationOutcome {
  warning: string;
}

/** A restart only describes saved recovery records. It never replays an external write. */
export function publicationRecoveryNotice(phases: string[], unreadable: number): string {
  if (unreadable > 0 || phases.some((phase: string) => phase !== 'completed')) {
    return shellString("shell_an_incomplete_recovery_record_from_the_previous_save_was", "An incomplete recovery record from the previous save was found. Recovery files have been retained unchanged. ") +
      shellString("shell_check_the_original_file_then_save_the_current_edits", "Check the original file, then save the current edits with Save As. External files will not be overwritten automatically.");
  }
  return phases.length > 0 ? shellString("shell_a_save_record_for_a_written_file_still_needs", "A save record for a written file still needs cleanup. Recovery files have been retained unchanged. Check the destination file.") : '';
}

export class PublicationTransaction {
  private storage: PublicationStorage;

  constructor(storage: PublicationStorage) {
    this.storage = storage;
  }

  async publish(stagePath: string, destinationUri: string, backupPath: string,
    retainedStagePath: string): Promise<PublicationOutcome> {
    const journal: PublicationJournal = { version: 1, phase: 'prepared', destinationUri: destinationUri,
      stagePath: stagePath, backupPath: backupPath, retainedStagePath: retainedStagePath, error: '', rollbackError: '' };
    let destinationFd = -1;
    let stageFd = -1;
    let backupReady = false;
    let mutationAttempted = false;
    let failure: PublicationFailure | undefined = undefined;
    try {
      // A newly selected empty target is read and backed up too. Unreadable targets are never overwritten.
      const sourceFd = await this.storage.openRead(destinationUri);
      try {
        const backupFd = await this.storage.openWrite(backupPath, true);
        try {
          await this.storage.copy(sourceFd, backupFd);
          await this.storage.sync(backupFd);
          backupReady = true;
        } finally {
          await this.storage.close(backupFd);
        }
      } finally {
        await this.storage.close(sourceFd);
      }
      await this.storage.writeJournal(journal);
      stageFd = await this.storage.openRead(stagePath);
      // No CREATE/TRUNC here. A provider must support writing and seeking before we touch its bytes.
      destinationFd = await this.storage.openWrite(destinationUri, false);
      await this.storage.prepareWrite(destinationFd);
      journal.phase = 'writing';
      await this.storage.writeJournal(journal);
      mutationAttempted = true;
      await this.storage.truncate(destinationFd);
      await this.storage.copy(stageFd, destinationFd);
      await this.storage.sync(destinationFd);
      journal.phase = 'completed';
      await this.storage.writeJournal(journal);
    } catch (error) {
      journal.error = (error as Error).message;
      if (mutationAttempted && backupReady) {
        try {
          journal.phase = 'rollingBack';
          // The already durable 'writing' record is enough if updating the journal now fails.
          try { await this.storage.writeJournal(journal); } catch (_) { }
          const restoreFd = await this.storage.openRead(backupPath);
          try {
            // Keep the original destination descriptor open; permission/URI reopening is not required.
            await this.storage.truncate(destinationFd);
            await this.storage.copy(restoreFd, destinationFd);
            await this.storage.sync(destinationFd);
          } finally {
            try { await this.storage.close(restoreFd); } catch (_) { }
          }
          journal.phase = 'restored';
          try { await this.storage.writeJournal(journal); } catch (_) { }
          failure = new PublicationFailure(shellString("shell_save_failed_the_original_file_was_restored_try_saving", "Save failed. The original file was restored. Try saving again or use Save As. Reason: {0}", journal.error));
        } catch (rollbackError) {
          journal.phase = 'recoveryRequired';
          journal.rollbackError = (rollbackError as Error).message;
          try {
            await this.storage.move(stagePath, retainedStagePath);
            journal.retainedStagePath = retainedStagePath;
          } catch (retentionError) {
            // The caller must ask Rust to retain this pending stage if a same-sandbox rename also fails.
            journal.retainedStagePath = stagePath;
            journal.rollbackError += shellString("shell_could_not_retain_the_new_file_detail", "; could not retain the new file: {0}", (retentionError as Error).message);
          }
          try { await this.storage.writeJournal(journal); } catch (_) { }
          failure = new PublicationFailure(shellString("shell_save_failed_and_the_original_file_could_not_be", "Save failed and the original file could not be restored. The old backup and new file have been retained. ") +
            shellString("shell_use_save_as_to_save_the_current_edits_then", "Use Save As to save the current edits, then inspect the destination file before continuing. Save reason: {0}; recovery reason: {1}", journal.error, journal.rollbackError),
            journal.retainedStagePath === stagePath ? stagePath : '');
        }
      } else {
        failure = new PublicationFailure(shellString("shell_could_not_save_safely_the_original_file_is_unchanged", "Could not save safely. The original file is unchanged. Use Save As in a readable and writable location. ") +
          shellString("shell_reason_detail", "Reason: {0}", journal.error));
      }
    } finally {
      // Closing an fd is housekeeping: it must not replace a publication/rollback result.
      if (stageFd >= 0) {
        try { await this.storage.close(stageFd); } catch (_) { }
      }
      if (destinationFd >= 0) {
        try { await this.storage.close(destinationFd); } catch (_) { }
      }
    }
    if (journal.phase === 'recoveryRequired') {
      throw failure as PublicationFailure;
    }
    let cleanupError = '';
    try {
      await this.storage.remove(backupPath);
      await this.storage.clearJournal();
    } catch (error) {
      cleanupError = (error as Error).message;
    }
    if (failure !== undefined) {
      if (cleanupError.length > 0) {
        failure.message += shellString("shell_cleanup_after_saving_could_not_be_confirmed_check_the", " Cleanup after saving could not be confirmed. Check the original file.");
      }
      throw failure;
    }
    return { warning: cleanupError.length === 0 ? '' :
      shellString("shell_the_file_was_written_but_cleanup_after_saving_could", "The file was written, but cleanup after saving could not be confirmed. Check the destination file. Reason: {0}", cleanupError) };
  }
}
