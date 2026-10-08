import { shellString } from './ShellLocalization';
export interface FolderLimits {
  maxFiles: number;
  maxBytes: number;
}

export interface FolderNode {
  kind: 'file' | 'directory' | 'symlink' | 'other';
  bytes: number;
}

export interface FolderEntry {
  relativePath: string;
  kind: 'file' | 'directory';
  bytes: number;
}

export interface FolderManifest {
  version: number;
  entries: FolderEntry[];
  fileCount: number;
  totalBytes: number;
}

export interface FolderStorage {
  list(directory: string): Promise<string[]>;
  inspect(path: string): Promise<FolderNode>;
  unlink(path: string): Promise<void>;
  removeEmptyDirectory(path: string): Promise<void>;
}

export class FolderImportCancelled extends Error {
  constructor() { super(shellString("shell_folder_import_canceled", "Folder import canceled")); }
}

export function validateFolderLimits(limits: FolderLimits): void {
  if (!Number.isSafeInteger(limits.maxFiles) || limits.maxFiles < 1 || limits.maxFiles > 500 ||
    !Number.isSafeInteger(limits.maxBytes) || limits.maxBytes < 1 || limits.maxBytes > 1024 * 1024 * 1024) {
    throw new Error(shellString("shell_folder_import_limits_must_be_1_500_files_and", "Folder import limits must be 1–500 files and no more than 1 GiB"));
  }
}

export function validateFolderRelativePath(path: string): void {
  const parts = path.split('/');
  if (path.length === 0 || path.startsWith('/') || path.includes('\\') || path.includes('\x00') ||
    parts.length > 32 || parts.some((part: string) => part.length === 0 || part === '.' || part === '..')) {
    throw new Error(shellString("shell_the_folder_contains_an_unsafe_relative_path", "The folder contains an unsafe relative path"));
  }
}

function validateRoot(root: string): void {
  if (!root.startsWith('/') || root === '/') {
    throw new Error(shellString("shell_the_import_root_must_be_a_unique_sandbox_directory", "The import root must be a unique sandbox directory"));
  }
  const parts = root.substring(1).split('/');
  if (root.includes('\\') || root.includes('\x00') ||
    parts.some((part: string) => part.length === 0 || part === '.' || part === '..')) {
    throw new Error(shellString("shell_the_sandbox_root_contains_an_unsafe_path", "The sandbox root contains an unsafe path"));
  }
}

function checkCancelled(isCancelled: () => boolean): void {
  if (isCancelled()) {
    throw new FolderImportCancelled();
  }
}

/** Explicit SDK copy contract, never an arbitrary one-child heuristic or an external URI lookup. */
export async function resolveSourceBasenameChild(storage: FolderStorage, containerRoot: string,
  isCancelled: () => boolean): Promise<string> {
  validateRoot(containerRoot);
  checkCancelled(isCancelled);
  if ((await storage.inspect(containerRoot)).kind !== 'directory') {
    throw new Error(shellString("shell_the_folder_copy_container_is_not_a_regular_directory", "The folder copy container is not a regular directory"));
  }
  const names = await storage.list(containerRoot);
  checkCancelled(isCancelled);
  if (names.length !== 1 || names[0].includes('/')) {
    throw new Error(shellString("shell_the_folder_provider_copy_hierarchy_does_not_match_a", "The folder provider copy hierarchy does not match a supported directory layout"));
  }
  validateFolderRelativePath(names[0]);
  const root = `${containerRoot}/${names[0]}`;
  if ((await storage.inspect(root)).kind !== 'directory') {
    throw new Error(shellString("shell_the_folder_copy_is_not_a_unique_regular_source", "The folder copy is not a unique regular source directory. Links will not be followed"));
  }
  checkCancelled(isCancelled);
  return root;
}

/** Metadata only. No external URI, content reads or followed symbolic links. */
export async function inspectSandboxFolder(storage: FolderStorage, root: string, limits: FolderLimits,
  isCancelled: () => boolean): Promise<FolderManifest> {
  validateRoot(root);
  validateFolderLimits(limits);
  const manifest: FolderManifest = { version: 1, entries: [], fileCount: 0, totalBytes: 0 };
  const pending: string[] = [''];
  const seen: Set<string> = new Set<string>();
  while (pending.length > 0) {
    checkCancelled(isCancelled);
    const relative = pending.shift() as string;
    const directory = relative.length === 0 ? root : `${root}/${relative}`;
    if ((await storage.inspect(directory)).kind !== 'directory') {
      throw new Error(shellString("shell_the_import_directory_is_not_a_regular_directory_links", "The import directory is not a regular directory. Links will not be followed"));
    }
    checkCancelled(isCancelled);
    for (const name of await storage.list(directory)) {
      // list() is first-level only. Do not trust a returned child to be a path.
      if (name.includes('/')) {
        throw new Error(shellString("shell_the_folder_manifest_contains_a_path_outside_its_directory", "The folder manifest contains a path outside its directory"));
      }
      const child = relative.length === 0 ? name : `${relative}/${name}`;
      validateFolderRelativePath(child);
      if (seen.has(child)) {
        throw new Error(shellString("shell_the_folder_manifest_contains_duplicate_paths", "The folder manifest contains duplicate paths"));
      }
      seen.add(child);
      if (seen.size > 4096) {
        throw new Error(shellString("shell_the_folder_exceeds_the_limit_of_4096_entries", "The folder exceeds the limit of 4096 entries"));
      }
      checkCancelled(isCancelled);
      const node = await storage.inspect(`${root}/${child}`);
      if (node.kind === 'symlink' || node.kind === 'other') {
        throw new Error(shellString("shell_the_folder_contains_links_or_special_files_and_was", "The folder contains links or special files and was not imported"));
      }
      if (!Number.isSafeInteger(node.bytes) || node.bytes < 0) {
        throw new Error(shellString("shell_invalid_folder_entry_size", "Invalid folder entry size"));
      }
      if (node.kind === 'file') {
        manifest.fileCount++;
        manifest.totalBytes += node.bytes;
        if (manifest.fileCount > limits.maxFiles || manifest.totalBytes > limits.maxBytes) {
          throw new Error(shellString("shell_the_folder_exceeds_the_file_count_or_size_limit", "The folder exceeds the file count or size limit"));
        }
      } else {
        pending.push(child);
      }
      manifest.entries.push({ relativePath: child, kind: node.kind, bytes: node.kind === 'file' ? node.bytes : 0 });
    }
  }
  checkCancelled(isCancelled);
  manifest.entries.sort((left: FolderEntry, right: FolderEntry) =>
    left.relativePath < right.relativePath ? -1 : left.relativePath > right.relativePath ? 1 : 0);
  return manifest;
}

/** Only call for a freshly allocated owned job. Unlink links without traversing their targets. */
export async function removeOwnedSandboxFolder(storage: FolderStorage, root: string): Promise<void> {
  validateRoot(root);
  const node = await storage.inspect(root);
  if (node.kind !== 'directory') {
    await storage.unlink(root);
    return;
  }
  for (const name of await storage.list(root)) {
    if (name.includes('/')) {
      throw new Error(shellString("shell_could_not_safely_remove_an_invalid_directory_entry", "Could not safely remove an invalid directory entry"));
    }
    validateFolderRelativePath(name);
    await removeOwnedSandboxFolder(storage, `${root}/${name}`);
  }
  await storage.removeEmptyDirectory(root);
}
