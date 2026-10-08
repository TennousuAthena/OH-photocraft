import { shellString } from './ShellLocalization';
import { AppStorageRoots } from './AppStorageRoots';
import { common } from '@kit.AbilityKit';
import { BusinessError, pasteboard } from '@kit.BasicServicesKit';
import { fileIo } from '@kit.CoreFileKit';
import { image } from '@kit.ImageKit';

export interface ClipboardContent {
  text: string;
  imagePath: string;
}

/** System access occurs only for an explicit copy/paste request from the editor. */
export class ClipboardBridge {
  private context: common.UIAbilityContext;
  private roots: AppStorageRoots;
  private sequence: number = 0;

  constructor(context: common.UIAbilityContext, roots: AppStorageRoots = new AppStorageRoots(context)) {
    this.context = context; this.roots = roots;
  }

  async read(requestId: number, textOnly: boolean): Promise<ClipboardContent> {
    // Called only for an explicit paste gesture or a visible PasteButton SUCCESS callback.
    const data = await pasteboard.getSystemPasteboard().getData();
    let text = '';
    let pixelMap: image.PixelMap | undefined = undefined;
    let imageUri = '';
    try {
      // Records can contain multiple representations, including a delayed image entry.
      for (let index = 0; index < data.getRecordCount(); index++) {
        const record = data.getRecord(index);
        const types = record.getValidTypes(textOnly ? [pasteboard.MIMETYPE_TEXT_PLAIN] :
          [pasteboard.MIMETYPE_TEXT_PLAIN, pasteboard.MIMETYPE_PIXELMAP, pasteboard.MIMETYPE_TEXT_URI]);
        if (text.length === 0 && types.includes(pasteboard.MIMETYPE_TEXT_PLAIN)) {
          const value = await record.getData(pasteboard.MIMETYPE_TEXT_PLAIN);
          if (typeof value === 'string') {
            text = value;
          }
        }
        if (!textOnly && pixelMap === undefined && types.includes(pasteboard.MIMETYPE_PIXELMAP)) {
          pixelMap = await record.getData(pasteboard.MIMETYPE_PIXELMAP) as image.PixelMap;
        }
        if (!textOnly && imageUri.length === 0 && types.includes(pasteboard.MIMETYPE_TEXT_URI)) {
          const value = await record.getData(pasteboard.MIMETYPE_TEXT_URI);
          if (typeof value === 'string' && value.startsWith('file://')) { imageUri = value; }
        }
        if (text.length > 0 && (textOnly || pixelMap !== undefined)) {
          break;
        }
      }
      if (!textOnly && pixelMap === undefined && imageUri.length > 0) {
        pixelMap = await this.readImageUri(imageUri);
      }
      if (pixelMap === undefined) {
        if (!textOnly) {
          throw new Error(shellString("shell_the_clipboard_contains_no_image_to_paste", "The clipboard contains no image to paste"));
        }
        if (text.length === 0) {
          throw new Error(shellString("shell_the_clipboard_contains_no_text_to_paste", "The clipboard contains no text to paste"));
        }
        return { text: text, imagePath: '' };
      }
      const information = await pixelMap.getImageInfo();
      this.checkImageSize(information);
      return { text: text, imagePath: await this.writePng(pixelMap, requestId) };
    } finally {
      if (pixelMap !== undefined) {
        await pixelMap.release();
      }
    }
  }

  private checkImageSize(information: image.ImageInfo): void {
    const width = information.size.width;
    const height = information.size.height;
    if (!Number.isInteger(width) || !Number.isInteger(height) || width <= 0 || height <= 0 ||
      width > 16384 || height > 16384 || width * height * 4 > 64 * 1024 * 1024) {
      throw new Error(shellString("shell_the_decoded_clipboard_image_exceeds_64_mib_or_has", "The decoded clipboard image exceeds 64 MiB or has invalid dimensions"));
    }
  }

  private async readImageUri(uri: string): Promise<image.PixelMap> {
    const file = await fileIo.open(uri, fileIo.OpenMode.READ_ONLY);
    let source: image.ImageSource | undefined = undefined;
    try {
      const stat = await fileIo.stat(file.fd);
      if (stat.size <= 0 || stat.size > 64 * 1024 * 1024) {
        throw new Error(shellString("shell_the_clipboard_image_file_is_empty_or_exceeds_64", "The clipboard image file is empty or exceeds 64 MiB"));
      }
      source = image.createImageSource(file.fd);
      this.checkImageSize(await source.getImageInfo());
      return await source.createPixelMap();
    } finally {
      try { await source?.release(); }
      finally { await fileIo.close(file); }
    }
  }

  async writeText(text: string): Promise<void> {
    await pasteboard.getSystemPasteboard().setData(pasteboard.createData(pasteboard.MIMETYPE_TEXT_PLAIN, text));
  }

  async writeImage(localPath: string): Promise<void> {
    this.checkSandboxPath(localPath);
    const file = await fileIo.open(localPath, fileIo.OpenMode.READ_ONLY);
    let source: image.ImageSource | undefined = undefined;
    let pixelMap: image.PixelMap | undefined = undefined;
    try {
      source = image.createImageSource(file.fd);
      pixelMap = await source.createPixelMap();
      await pasteboard.getSystemPasteboard().setData(pasteboard.createData(pasteboard.MIMETYPE_PIXELMAP, pixelMap));
    } finally {
      try {
        if (pixelMap !== undefined) {
          await pixelMap.release();
        }
      } finally {
        try {
          if (source !== undefined) {
            await source.release();
          }
        } finally {
          await fileIo.close(file);
        }
      }
    }
  }

  async removeCacheFile(localPath: string): Promise<void> {
    if (!localPath.startsWith(`${this.roots.cacheDir}/`) || localPath.includes('/../')) {
      throw new Error(shellString("shell_invalid_clipboard_temporary_file_path", "Invalid clipboard temporary file path"));
    }
    if (await fileIo.access(localPath)) {
      await fileIo.unlink(localPath);
    }
  }

  private checkSandboxPath(localPath: string): void {
    if ((!localPath.startsWith(`${this.roots.filesDir}/`) && !localPath.startsWith(`${this.roots.cacheDir}/`)) ||
      localPath.includes('/../')) {
      throw new Error(shellString("shell_the_copied_image_is_outside_the_app_sandbox", "The copied image is outside the app sandbox"));
    }
  }

  private async writePng(pixelMap: image.PixelMap, requestId: number): Promise<string> {
    // Rust owns accepted paste files only under this same cacheDir/Clipboard root.
    const directory = `${this.roots.cacheDir}/Clipboard`;
    // Native initialization normally creates this directory. HarmonyOS mkdir still
    // reports EEXIST for an existing directory even with its recursive option.
    if (!await fileIo.access(directory)) {
      try { await fileIo.mkdir(directory); }
      catch (error) {
        if ((error as BusinessError).code !== 13900015) { throw error as Error; }
      }
    }
    const directoryInfo = await fileIo.lstat(directory);
    if (!directoryInfo.isDirectory() || directoryInfo.isSymbolicLink()) {
      throw new Error(shellString("shell_invalid_clipboard_temporary_file_path", "Invalid clipboard temporary file path"));
    }
    const localPath = `${directory}/paste-${requestId}-${Date.now()}-${this.sequence++}.png`;
    const file = await fileIo.open(localPath,
      fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
    let packer: image.ImagePacker | undefined = undefined;
    try {
      packer = image.createImagePacker();
      // PNG is lossless; packToFile avoids the default 25 MB encoded buffer limit.
      await packer.packToFile(pixelMap, file.fd, { format: 'image/png', quality: 100 });
      await fileIo.fsync(file.fd);
    } catch (error) {
      await this.removeCacheFile(localPath);
      throw error as Error;
    } finally {
      try {
        if (packer !== undefined) {
          await packer.release();
        }
      } finally {
        await fileIo.close(file);
      }
    }
    return localPath;
  }
}
