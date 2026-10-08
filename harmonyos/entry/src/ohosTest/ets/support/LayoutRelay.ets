import { abilityDelegatorRegistry } from '@kit.TestKit';
import { fileIo } from '@kit.CoreFileKit';
import { cryptoFramework } from '@kit.CryptoArchitectureKit';
import { util } from '@kit.ArkTS';

interface RelayReply {
  runId: string; nonce: string; ticket: string; path: string; ok: boolean;
  layout?: Object; error?: string;
}
interface RelayError { code?: number; message?: string; }
interface RelayReady { runId: string; nonce: string; ticket: string; path: string; payloadBytes: number; payloadSha256: string; }
const MAX_LAYOUT_BYTES = 4 * 1024 * 1024;
const MAX_REPLY_BYTES = MAX_LAYOUT_BYTES + 65536;
function check(condition: boolean, message: string): void { if (!condition) { throw new Error(message); } }

/** Test-module-only transport. The host reads a daemon-owned layout and publishes one authenticated reply
 * through HDC's official debug-bundle file transfer, inside this run's real isolated cache. */
export class LayoutRelay {
  private runId: string;
  private bundleName: string;
  private cache: string;
  private sequence: number = 0;
  private closed: boolean = false;
  private active: Promise<string> | undefined = undefined;
  constructor(runId: string, bundleName: string, cache: string) {
    check(/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(runId) && /^[A-Za-z0-9_.]+$/.test(bundleName),
      'Invalid layout relay identity');
    check(/^\/[A-Za-z0-9_./-]+$/.test(cache) && !cache.includes('//') &&
      !cache.split('/').some((part: string) => part === '.' || part === '..') &&
      cache.endsWith(`/haps/entry/cache/PhotoCraftTestRuns/${runId}/cache`),
      'Layout relay requires the validated entry-module test cache');
    this.runId = runId; this.bundleName = bundleName; this.cache = cache;
  }
  async read(path: string, targetBundle: string): Promise<string> {
    check(!this.closed && this.active === undefined, 'Layout relay must be open and serial');
    check(targetBundle === this.bundleName || targetBundle === 'com.huawei.hmos.filemanager',
      'Layout relay is outside the application and system picker');
    check(new RegExp(`^/data/local/tmp/PhotoCraftTest-${this.runId}-[0-9]+-[0-9]+-query\\.json$`).test(path),
      'Layout relay can read only this run\'s owned layout');
    const operation = this.request(path, targetBundle);
    this.active = operation;
    try { return await operation; } finally { this.active = undefined; }
  }
  private async request(path: string, targetBundle: string): Promise<string> {
    const stat = await fileIo.lstat(this.cache);
    check(stat.isDirectory() && !stat.isSymbolicLink(), 'Layout relay cache is not a real isolated directory');
    const ticket = `layout-${++this.sequence}`;
    const directory = `${this.cache}/LayoutRelay-${Date.now()}-${this.sequence}`;
    const replyPath = `${directory}/reply.part`;
    const readyPath = `${directory}/ready.json`;
    const nonceBytes = cryptoFramework.createRandom().generateRandomSync(32).data;
    check(nonceBytes.length === 32, 'Layout relay cryptographic nonce generation failed');
    const nonce = Array.from(nonceBytes).map((value: number) => value.toString(16).padStart(2, '0')).join('');
    await fileIo.mkdir(directory);
    let failure: Error | undefined = undefined;
    try {
      await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_LAYOUT_REQUEST ${JSON.stringify({
        runId: this.runId, bundleName: targetBundle, path: path, ticket: ticket, nonce: nonce,
        replyPath: replyPath, readyPath: readyPath })}\n`);
      const until = Date.now() + 10000;
      while (Date.now() < until) {
        check(!this.closed, 'Layout relay was closed while a request was pending');
        if (await fileIo.access(readyPath)) {
          const readyStat = await fileIo.lstat(readyPath);
          check(readyStat.isFile() && !readyStat.isSymbolicLink() && readyStat.size <= 4096,
            'Layout relay ready marker is not a bounded regular file');
          let ready: RelayReady | undefined = undefined;
          if (readyStat.size > 0) {
            const marker = await fileIo.readText(readyPath, { length: readyStat.size });
            // HDC writes this final small marker directly. Do not consume a partially received JSON file.
            if (new util.TextEncoder().encodeInto(marker).length === readyStat.size) {
              try { ready = JSON.parse(marker) as RelayReady; } catch (_partialJson) { /* Await the transfer's completed marker. */ }
            }
          }
          if (ready === undefined) {
            await new Promise<void>((resolve: () => void) => { setTimeout(resolve, 100); });
            continue;
          }
          check(Object.keys(ready).length === 6 && Object.keys(ready).every((key: string) =>
            ['runId', 'nonce', 'ticket', 'path', 'payloadBytes', 'payloadSha256'].includes(key)) &&
            ready.runId === this.runId && ready.nonce === nonce && ready.ticket === ticket && ready.path === path &&
            Number.isInteger(ready.payloadBytes) && ready.payloadBytes > 0 && ready.payloadBytes <= MAX_REPLY_BYTES &&
            /^[0-9a-f]{64}$/.test(ready.payloadSha256), 'Layout relay ready marker identity or schema is invalid');
          const replyStat = await fileIo.lstat(replyPath);
          check(replyStat.isFile() && !replyStat.isSymbolicLink() && replyStat.size === ready.payloadBytes,
            'Layout relay reply is not a bounded regular file');
          const handle = await fileIo.open(replyPath, fileIo.OpenMode.READ_ONLY | fileIo.OpenMode.NOFOLLOW);
          const buffer = new ArrayBuffer(ready.payloadBytes);
          try {
            const openedStat = await fileIo.stat(handle.fd);
            check(openedStat.isFile() && openedStat.size === ready.payloadBytes, 'Layout relay payload changed before opening');
            check(await fileIo.read(handle.fd, buffer) === ready.payloadBytes, 'Layout relay payload read is incomplete');
          } finally { await fileIo.close(handle); }
          const bytes = new Uint8Array(buffer);
          const digest = cryptoFramework.createMd('SHA256');
          await digest.update({ data: bytes });
          const actualSha = Array.from((await digest.digest()).data).map((value: number) => value.toString(16).padStart(2, '0')).join('');
          check(actualSha === ready.payloadSha256, 'Layout relay payload does not match the completed host transfer digest');
          const serialized = util.TextDecoder.create('utf-8', { fatal: true }).decodeToString(bytes);
          const reply = JSON.parse(serialized) as RelayReply;
          check(Object.keys(reply).every((key: string) => ['runId', 'nonce', 'ticket', 'path', 'ok', 'layout', 'error'].includes(key)) &&
            reply.runId === this.runId && reply.nonce === nonce && reply.ticket === ticket && reply.path === path &&
            typeof reply.ok === 'boolean', 'Layout relay reply identity or schema is invalid');
          if (!reply.ok) {
            check(reply.layout === undefined && typeof reply.error === 'string' && reply.error.length <= 1024,
              'Layout relay error reply is invalid');
            throw new Error(`Host layout relay failed: ${reply.error}`);
          }
          check(reply.error === undefined && reply.layout !== undefined && reply.layout !== null &&
            typeof reply.layout === 'object' && !Array.isArray(reply.layout), 'Layout relay success reply has no JSON layout');
          const layout = JSON.stringify(reply.layout);
          check(new util.TextEncoder().encodeInto(layout).length <= MAX_LAYOUT_BYTES, 'Layout relay tree exceeds its size limit');
          return layout;
        }
        await new Promise<void>((resolve: () => void) => { setTimeout(resolve, 100); });
      }
      throw new Error('Host layout relay did not publish an authenticated reply within 10 seconds');
    } catch (error) { failure = error as Error; throw error as Error; }
    finally {
      try {
        for (const owned of [replyPath, readyPath]) {
          if (await fileIo.access(owned)) { await fileIo.unlink(owned); }
        }
        await fileIo.rmdir(directory);
      } catch (error) {
        const detail = error as RelayError;
        try { await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_LAYOUT_CLEANUP_ERROR ${JSON.stringify({
          runId: this.runId, ticket: ticket, code: detail.code ?? -1, message: detail.message ?? 'Unknown cleanup error' })}\n`); }
        catch (_printError) { /* Preserve the original request/cleanup error. */ }
        if (failure === undefined) { throw error as Error; }
      }
    }
  }
  async close(): Promise<void> {
    this.closed = true;
    const operation = this.active;
    if (operation !== undefined) { try { await operation; } catch (_requestError) { /* The requesting case owns the failure. */ } }
  }
}
