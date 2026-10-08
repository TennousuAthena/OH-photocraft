import { fileIo } from '@kit.CoreFileKit';
import { hiAppEvent } from '@kit.PerformanceAnalysisKit';

const QUEUE_LIMIT: number = 32;
const FAULT_SLOTS: number = 16;
const LIFECYCLE_SLOTS: number = 8;
const RECORD_BYTES: number = 2048;
const CALLBACK_SCAN_LIMIT: number = 64;
const OWN_BUNDLE: string = 'moe.kiwi.photocraft';

interface FaultSignal {
  signo?: number;
  code?: number;
}

interface FaultException {
  name?: string;
  signal?: FaultSignal;
}

interface FaultMemory {
  rss?: number;
}

interface FaultParams {
  bundle_name?: string;
  time?: number;
  crash_type?: string;
  foreground?: boolean;
  pid?: number;
  process_life_time?: number;
  log_over_limit?: boolean;
  exception?: FaultException;
  memory?: FaultMemory;
}

interface FaultRecord {
  version: number;
  kind: string;
  observedAtMs: number;
  observerStartedAtMs: number;
  eventAtMs: number | null;
  crashType: string;
  jsErrorType: string;
  foreground: boolean | null;
  pid: number | null;
  processLifeSeconds: number | null;
  signalNumber: number | null;
  signalCode: number | null;
  rssKiB: number | null;
  logOverLimit: boolean | null;
  droppedBefore: number;
}

interface FaultRecordStorage {
  write(slot: string, text: string): Promise<void>;
}

/** Only fixed ASCII metadata reaches this private storage; raw OS payloads never do. */
class PrivateFaultRecordStorage implements FaultRecordStorage {
  private filesDir: string;

  constructor(filesDir: string) { this.filesDir = filesDir; }

  async write(slot: string, text: string): Promise<void> {
    if (!/^(fault-(0[0-9]|1[0-5])|lifecycle-0[0-7])\.json$/.test(slot) ||
      text.length > RECORD_BYTES) {
      throw new Error('Invalid diagnostic record');
    }
    let directory = this.filesDir;
    let info = await fileIo.lstat(directory);
    if (!info.isDirectory() || info.isSymbolicLink()) {
      throw new Error('Invalid private diagnostic directory');
    }
    for (const name of ['PhotoCraft', 'Diagnostics']) {
      directory += `/${name}`;
      if (!await fileIo.access(directory)) {
        try {
          await fileIo.mkdir(directory);
        } catch (_) {
          // Concurrent creation is accepted only after checking the real directory below.
        }
      }
      info = await fileIo.lstat(directory);
      if (!info.isDirectory() || info.isSymbolicLink()) {
        throw new Error('Invalid private diagnostic directory');
      }
    }
    const destination = `${directory}/${slot}`;
    const temporary = `${directory}/pending.json`;
    for (const path of [destination, temporary]) {
      if (await fileIo.access(path)) {
        const existing = await fileIo.lstat(path);
        if (!existing.isFile() || existing.isSymbolicLink()) {
          throw new Error('Invalid private diagnostic file');
        }
      }
    }
    const file = await fileIo.open(temporary,
      fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC | fileIo.OpenMode.NOFOLLOW);
    try {
      if (!(await fileIo.stat(file.fd)).isFile() || await fileIo.write(file.fd, text) !== text.length) {
        throw new Error('Incomplete diagnostic record');
      }
      await fileIo.fsync(file.fd);
    } finally {
      await fileIo.close(file);
    }
    await fileIo.rename(temporary, destination);
  }
}

/** One process-wide bounded writer prevents overlapping callbacks/ability owners from racing the fixed temp file. */
class FaultJournal {
  private storage: FaultRecordStorage;
  private queue: FaultRecord[] = [];
  private writing: boolean = false;
  private faultSlot: number = 0;
  private lifecycleSlot: number = 0;
  private dropped: number = 0;
  private recentFaultKeys: string[] = [];

  constructor(storage: FaultRecordStorage) { this.storage = storage; }

  enqueue(record: FaultRecord): void {
    const isFault = record.kind === 'crash' || record.kind === 'freeze';
    const key = isFault && record.eventAtMs !== null ?
      `${record.kind}:${record.eventAtMs}:${record.pid}:${record.crashType}:${record.signalNumber}:${record.signalCode}` : '';
    if (key.length > 0 && this.recentFaultKeys.includes(key)) { return; }
    if (this.queue.length >= QUEUE_LIMIT) {
      this.dropped = Math.min(Number.MAX_SAFE_INTEGER, this.dropped + 1);
      return;
    }
    record.droppedBefore = this.dropped;
    if (key.length > 0) {
      this.recentFaultKeys.push(key);
      if (this.recentFaultKeys.length > CALLBACK_SCAN_LIMIT) { this.recentFaultKeys.shift(); }
    }
    this.queue.push(record);
    if (!this.writing) { void this.drain(); }
  }

  private async drain(): Promise<void> {
    this.writing = true;
    try {
      while (this.queue.length > 0) {
        const record = this.queue.shift();
        if (!record) { continue; }
        const isFault = record.kind === 'crash' || record.kind === 'freeze';
        const index = isFault ? this.faultSlot : this.lifecycleSlot;
        const slot = `${isFault ? 'fault' : 'lifecycle'}-${index.toString().padStart(2, '0')}.json`;
        if (isFault) { this.faultSlot = (this.faultSlot + 1) % FAULT_SLOTS; }
        else { this.lifecycleSlot = (this.lifecycleSlot + 1) % LIFECYCLE_SLOTS; }
        try {
          await this.storage.write(slot, JSON.stringify(record));
        } catch (_) {
          // Diagnostics are best effort. Full disk, unsupported IO or provider errors cannot break launch/editor work.
          this.dropped = Math.min(Number.MAX_SAFE_INTEGER, this.dropped + 1);
        }
      }
    } finally {
      this.writing = false;
    }
  }
}

/** Own-app OS fault subscription. It neither exports data nor changes crash/freeze collection policy. */
export class AppFaultObserver {
  private static active: AppFaultObserver | undefined;
  private static journal: FaultJournal | undefined;
  private journal: FaultJournal;
  private watcher: hiAppEvent.Watcher;
  private started: boolean = false;
  private disposed: boolean = false;
  private registered: boolean = false;
  private startedAt: number;
  private subscribeToOsFaults: boolean;

  constructor(filesDir: string, storage?: FaultRecordStorage, subscribeToOsFaults: boolean = true) {
    this.subscribeToOsFaults = subscribeToOsFaults;
    this.startedAt = Date.now();
    if (!AppFaultObserver.journal) {
      AppFaultObserver.journal = new FaultJournal(storage ?? new PrivateFaultRecordStorage(filesDir));
    }
    this.journal = AppFaultObserver.journal;
    this.watcher = {
      name: 'PhotoCraftFaultWatcher',
      appEventFilters: [{ domain: hiAppEvent.domain.OS, eventTypes: [hiAppEvent.EventType.FAULT],
        names: [hiAppEvent.event.APP_CRASH, hiAppEvent.event.APP_FREEZE] }],
      onReceive: (domain: string, groups: hiAppEvent.AppEventGroup[]): void => {
        try { this.receive(domain, groups); } catch (_) { /* malformed diagnostics never reach editor lifecycle */ }
      }
    };
  }

  start(): void {
    if (this.started || this.disposed) { return; }
    AppFaultObserver.active?.dispose();
    AppFaultObserver.active = this;
    this.started = true;
    this.lifecycle('created');
    // HiAppEvent owns shared databases outside the supplied journal root. An isolated
    // device test keeps private lifecycle records; the host captures faults by PID from platform logs.
    if (!this.subscribeToOsFaults) {
      this.lifecycle('watcherSkippedForDeviceTest');
      return;
    }
    try {
      this.registered = hiAppEvent.addWatcher(this.watcher) !== null;
    } catch (_) {
      this.registered = false;
    }
    this.lifecycle(this.registered ? 'watcherReady' : 'watcherUnavailable');
  }

  foreground(): void { this.lifecycle('foreground'); }
  background(): void { this.lifecycle('background'); }

  dispose(): void {
    if (this.disposed) { return; }
    this.lifecycle('destroyed');
    this.disposed = true;
    if (AppFaultObserver.active === this) { AppFaultObserver.active = undefined; }
    if (this.registered) {
      this.registered = false;
      try { hiAppEvent.removeWatcher(this.watcher); } catch (_) { /* never fail ability destruction */ }
    }
  }

  private lifecycle(kind: string): void {
    if (!this.started || this.disposed) { return; }
    this.journal.enqueue(this.record(kind));
  }

  private receive(domain: string, groups: hiAppEvent.AppEventGroup[]): void {
    if (!this.subscribeToOsFaults || !this.started || this.disposed || domain !== hiAppEvent.domain.OS || !Array.isArray(groups)) { return; }
    let inspected = 0;
    for (let groupIndex = 0; groupIndex < Math.min(groups.length, CALLBACK_SCAN_LIMIT); groupIndex++) {
      const group = groups[groupIndex];
      if (!group || (group.name !== hiAppEvent.event.APP_CRASH && group.name !== hiAppEvent.event.APP_FREEZE) ||
        !Array.isArray(group.appEventInfos)) { continue; }
      for (const info of group.appEventInfos) {
        if (++inspected > CALLBACK_SCAN_LIMIT) { return; }
        if (!info || info.domain !== domain || info.name !== group.name ||
          info.eventType !== hiAppEvent.EventType.FAULT || typeof info.params !== 'object' || !info.params) { continue; }
        const params = info.params as FaultParams;
        // HiAppEvent exposes only the application's sandbox events; additionally reject an explicit foreign bundle.
        if (params.bundle_name !== undefined && params.bundle_name !== OWN_BUNDLE) { continue; }
        const record = this.record(info.name === hiAppEvent.event.APP_CRASH ? 'crash' : 'freeze');
        record.eventAtMs = this.number(params.time, 0, Number.MAX_SAFE_INTEGER);
        record.crashType = params.crash_type === 'NativeCrash' || params.crash_type === 'JsError' ?
          params.crash_type : 'unknown';
        record.foreground = typeof params.foreground === 'boolean' ? params.foreground : null;
        record.pid = this.number(params.pid, 1, 2147483647);
        record.processLifeSeconds = this.number(params.process_life_time, 0, Number.MAX_SAFE_INTEGER);
        record.logOverLimit = typeof params.log_over_limit === 'boolean' ? params.log_over_limit : null;
        if (params.memory && typeof params.memory === 'object') {
          record.rssKiB = this.number(params.memory.rss, 0, Number.MAX_SAFE_INTEGER);
        }
        if (params.exception && typeof params.exception === 'object') {
          const name = params.exception.name;
          if (record.crashType === 'JsError' && name &&
            ['Error', 'TypeError', 'RangeError', 'ReferenceError', 'SyntaxError', 'URIError',
              'EvalError', 'OutOfMemoryError'].includes(name)) { record.jsErrorType = name; }
          if (record.crashType === 'NativeCrash' && params.exception.signal &&
            typeof params.exception.signal === 'object') {
            record.signalNumber = this.number(params.exception.signal.signo, 1, 128);
            record.signalCode = this.number(params.exception.signal.code, -2147483648, 2147483647);
          }
        }
        this.journal.enqueue(record);
      }
    }
  }

  private number(value: number | undefined, minimum: number, maximum: number): number | null {
    return typeof value === 'number' && Number.isSafeInteger(value) && value >= minimum && value <= maximum ? value : null;
  }

  private record(kind: string): FaultRecord {
    return { version: 1, kind: kind, observedAtMs: Date.now(), observerStartedAtMs: this.startedAt,
      eventAtMs: null, crashType: 'unknown', jsErrorType: '', foreground: null, pid: null,
      processLifeSeconds: null, signalNumber: null, signalCode: null, rssKiB: null,
      logOverLimit: null, droppedBefore: 0 };
  }
}
