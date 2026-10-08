import { shellString } from './ShellLocalization';
import { util } from '@kit.ArkTS';
import { BusinessError } from '@kit.BasicServicesKit';
import { fileIo } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { PublicationJournal, PublicationStorage } from './PublicationTransaction';

/** Only openRead/openWrite see an external URI. Every metadata/move/delete operation is sandbox-only. */
export class PublicationFileStorage implements PublicationStorage {
  private directory: string;
  private files: Map<number, fileIo.File> = new Map<number, fileIo.File>();

  constructor(directory: string) {
    this.directory = directory;
  }

  async openRead(location: string): Promise<number> {
    const file = await this.perform('open-read', () => fileIo.open(location, fileIo.OpenMode.READ_ONLY));
    this.files.set(file.fd, file);
    return file.fd;
  }

  async openWrite(location: string, create: boolean): Promise<number> {
    // Existing provider targets are deliberately opened without TRUNC or CREATE.
    const file = await this.perform('open-write', () =>
      fileIo.open(location, fileIo.OpenMode.WRITE_ONLY | (create ? fileIo.OpenMode.CREATE : 0)));
    this.files.set(file.fd, file);
    return file.fd;
  }

  async close(fd: number): Promise<void> {
    try {
      await this.perform('close', () => fileIo.close(fd));
    } finally {
      this.files.delete(fd);
    }
  }

  async copy(source: number, destination: number): Promise<void> {
    await this.perform('fd-copy', () => fileIo.copyFile(source, destination));
  }

  async sync(fd: number): Promise<void> {
    await this.perform('file-fsync', () => fileIo.fsync(fd));
  }

  async prepareWrite(fd: number): Promise<void> {
    // copyFile internally truncates before seeking; test seek support before any truncation.
    await this.perform('seek-preflight', async () => { fileIo.lseek(fd, 0); });
  }

  async truncate(fd: number): Promise<void> {
    await this.perform('truncate', () => fileIo.truncate(fd, 0));
  }

  async move(source: string, destination: string): Promise<void> {
    await this.perform('retain-stage', () => fileIo.rename(source, destination));
  }

  async remove(location: string): Promise<void> {
    if (await this.perform('cleanup-access', () => fileIo.access(location))) {
      await this.perform('cleanup-unlink', () => fileIo.unlink(location));
    }
  }

  async writeJournal(journal: PublicationJournal): Promise<void> {
    const temporary = `${this.directory}/journal.json.tmp`;
    const file = await this.perform('journal-open', () => fileIo.open(temporary,
      fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC));
    try {
      const bytes = new util.TextEncoder().encodeInto(JSON.stringify(journal));
      if (await this.perform('journal-write', () => fileIo.write(file.fd, bytes.buffer as ArrayBuffer)) !== bytes.byteLength) {
        throw new Error(shellString("shell_the_save_recovery_record_was_not_fully_written", "The save recovery record was not fully written"));
      }
      await this.perform('journal-file-fsync', () => fileIo.fsync(file.fd));
    } finally {
      await this.perform('journal-close', () => fileIo.close(file));
    }
    await this.perform('journal-rename', () => fileIo.rename(temporary, `${this.directory}/journal.json`));
    await this.syncDirectory(this.directory);
    const parent = this.directory.substring(0, this.directory.lastIndexOf('/'));
    await this.syncDirectory(parent);
    await this.syncDirectory(parent.substring(0, parent.lastIndexOf('/')));
  }

  async clearJournal(): Promise<void> {
    await this.remove(`${this.directory}/journal.json.tmp`);
    await this.remove(`${this.directory}/journal.json`);
    await this.syncDirectory(this.directory);
    // rmdir is recursive in this SDK, so use it only after checking our unique directory is empty.
    if ((await this.perform('cleanup-list', () => fileIo.listFile(this.directory))).length === 0) {
      await this.perform('cleanup-rmdir', () => fileIo.rmdir(this.directory));
      await this.syncDirectory(this.directory.substring(0, this.directory.lastIndexOf('/')));
    }
  }

  private async syncDirectory(directory: string): Promise<void> {
    const fd = await this.openRead(directory);
    try {
      await this.perform('journal-directory-fsync', () => fileIo.fsync(fd));
    } finally {
      await this.close(fd);
    }
  }

  private async perform<T>(stage: string, operation: () => Promise<T>): Promise<T> {
    try {
      return await operation();
    } catch (error) {
      hilog.error(0x0000, 'PhotoCraft', 'Publication IO failure stage=%{public}s code=%{public}d',
        stage, (error as BusinessError).code || 0);
      throw error as Error;
    }
  }
}
