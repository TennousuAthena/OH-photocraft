import { Configuration, UIAbility, Want } from '@kit.AbilityKit';
import { window } from '@kit.ArkUI';
import { i18n } from '@kit.LocalizationKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { hiAppEvent, hilog } from '@kit.PerformanceAnalysisKit';
import { AppFaultObserver } from '../platform/AppFaultObserver';
import { AppStorageRoots } from '../platform/AppStorageRoots';
import { configureShellLocalization } from '../platform/ShellLocalization';
import nativePhotoCraft from 'libphotocraft.so';
import BuildProfile from 'BuildProfile';

export default class EntryAbility extends UIAbility {
  private faultObserver: AppFaultObserver | undefined;

  onCreate(want: Want): void {
    const requestedRunId = want?.parameters?.['photocraft.testRunId'];
    if (BuildProfile.DEVICE_TESTS && requestedRunId !== undefined && typeof requestedRunId !== 'string') {
      throw new Error('PhotoCraft device test run ID must be a string');
    }
    const roots = AppStorageRoots.activate(this.context, BuildProfile.DEVICE_TESTS,
      typeof requestedRunId === 'string' ? requestedRunId : undefined);
    const subscribeToOsFaults = !(BuildProfile.DEVICE_TESTS && roots.runId.length > 0);
    // Framework event writers share application-level storage. The official logging switch is process-local,
    // so an isolated test run disables it before any editor UI or system input initializes.
    // SDK26 throws on configuration failure; letting that error escape fails test startup rather than weakening isolation.
    if (!subscribeToOsFaults) { hiAppEvent.configure({ disable: true }); }
    this.faultObserver = new AppFaultObserver(roots.filesDir, undefined, subscribeToOsFaults);
    this.faultObserver.start();
    this.synchronizeLanguage();
  }

  onForeground(): void {
    // Also refresh on resume in case the system changed language while this ability was suspended.
    this.synchronizeLanguage();
    this.faultObserver?.foreground();
    AppStorage.setOrCreate('photocraft.foreground', true);
  }

  onBackground(): void {
    this.faultObserver?.background();
    AppStorage.setOrCreate('photocraft.foreground', false);
  }

  onConfigurationUpdate(configuration: Configuration): void {
    this.synchronizeLanguage(configuration.language);
  }

  private synchronizeLanguage(language?: string): void {
    configureShellLocalization((key: string) => this.context.resourceManager.getStringByNameSync(key));
    try {
      const tag = language || i18n.System.getSystemLanguage();
      // This is a system-language input; the editor retains its existing Auto/manual preference.
      if (!nativePhotoCraft.setSystemLanguage(tag)) {
        hilog.warn(0x0000, 'PhotoCraft', 'Cannot synchronize system language: %{public}s',
          nativePhotoCraft.lastError());
      }
    } catch (error) {
      hilog.warn(0x0000, 'PhotoCraft', 'Cannot read system language: %{public}s',
        (error as BusinessError).message);
    }
  }

  onDestroy(): void {
    this.faultObserver?.dispose();
    this.faultObserver = undefined;
  }

  onWindowStageCreate(windowStage: window.WindowStage): void {
    windowStage.loadContent('pages/Index', (error: BusinessError) => {
      if (error.code) {
        hilog.error(0x0000, 'PhotoCraft', 'Cannot load editor: %{public}s', error.message);
      }
    });
  }
}
