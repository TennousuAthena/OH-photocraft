import BuildProfile from 'BuildProfile';
import { AppStorageRoots } from './AppStorageRoots';

export interface PublishedFileEvidence {
  runId: string;
  requestId: number;
  uri: string;
  filename: string;
}

/** Expose the genuine picker publication outcome only in an isolated test build. */
export function recordDeviceTestPublication(roots: AppStorageRoots, requestId: number, uri: string, filename: string): void {
  if (BuildProfile.DEVICE_TESTS && AppStorageRoots.validRunId(roots.runId)) {
    const evidence: PublishedFileEvidence = { runId: roots.runId, requestId: requestId, uri: uri, filename: filename };
    AppStorage.setOrCreate('photocraft.test.publishedFile', evidence);
  }
}
