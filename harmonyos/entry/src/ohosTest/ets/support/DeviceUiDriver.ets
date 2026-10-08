import { abilityDelegatorRegistry, Component as SdkComponent, Driver, ON as SdkOn, On, UiWindow as SdkWindow } from '@kit.TestKit';
import { fileIo } from '@kit.CoreFileKit';
import { LayoutRelay } from './LayoutRelay';

export class DeviceSelector {
  idValue?: string;
  textValue?: string;
  typeValue?: string;
  focusedValue?: boolean;
  bundleName?: string;
  id(value: string): DeviceSelector { this.idValue = value; return this; }
  text(value: string): DeviceSelector { this.textValue = value; return this; }
  type(value: string): DeviceSelector { this.typeValue = value; return this; }
  focused(value: boolean): DeviceSelector { this.focusedValue = value; return this; }
  inWindow(value: string): DeviceSelector { this.bundleName = value; return this; }
}
export class DeviceOn {
  static id(value: string): DeviceSelector { return new DeviceSelector().id(value); }
  static text(value: string): DeviceSelector { return new DeviceSelector().text(value); }
  static type(value: string): DeviceSelector { return new DeviceSelector().type(value); }
}
interface UiAttributes { id?: string; text?: string; type?: string; focused?: string; bounds?: string; bundleName?: string; visible?: string; enabled?: string; }
interface UiNode { attributes: UiAttributes; children?: UiNode[]; }
interface NodeMatch { attributes: UiAttributes; bundleName: string; windowBounds: string; }
interface LayoutReadError { code?: number; message?: string; }
export interface DeviceWindowFilter { bundleName: string; }

function check(condition: boolean, message: string): void { if (!condition) { throw new Error(message); } }
function quote(value: string): string {
  check(!value.includes('\0') && !value.includes('\n') && !value.includes('\r'), 'UI input contains an invalid control character');
  return `'${value.replace(/'/g, "'\\''")}'`;
}
function bounds(value: string): number[] {
  const match = /^\[(-?[0-9]+),(-?[0-9]+)\]\[(-?[0-9]+),(-?[0-9]+)\]$/.exec(value);
  check(match !== null, 'UITest component bounds are unavailable');
  if (match === null) { return []; }
  const left = Number(match[1]); const top = Number(match[2]); const right = Number(match[3]); const bottom = Number(match[4]);
  check([left, top, right, bottom].every((value: number) => Number.isFinite(value) && Number.isInteger(value) && value >= 0 && value <= 32768) &&
    right > left && bottom > top, 'UITest component has invalid or invisible bounds');
  return [left, top, right, bottom];
}
function center(match: NodeMatch): number[] {
  const component = bounds(match.attributes.bounds ?? '');
  const window = bounds(match.windowBounds);
  check(component[0] >= window[0] && component[1] >= window[1] && component[2] <= window[2] && component[3] <= window[3],
    'UITest component is outside its actual bundle window bounds');
  return [Math.floor((component[0] + component[2]) / 2), Math.floor((component[1] + component[3]) / 2)];
}

export class DeviceComponent {
  private owner: DeviceUiDriver;
  private selector: DeviceSelector;
  private sdk?: SdkComponent;
  constructor(owner: DeviceUiDriver, selector: DeviceSelector, sdk?: SdkComponent) {
    this.owner = owner; this.selector = selector; this.sdk = sdk;
  }
  async click(): Promise<void> {
    if (this.sdk !== undefined) { await this.sdk.click(); return; }
    await this.owner.componentAction(this.selector, 'click');
  }
  async doubleClick(): Promise<void> {
    if (this.sdk !== undefined) { await this.sdk.doubleClick(); return; }
    await this.owner.componentAction(this.selector, 'doubleClick');
  }
  async inputText(text: string): Promise<void> {
    if (this.sdk !== undefined) { await this.sdk.inputText(text); return; }
    await this.owner.componentInput(this.selector, text);
  }
}
export class DeviceWindow {
  private owner: DeviceUiDriver;
  private bundleName: string;
  private sdk?: SdkWindow;
  constructor(owner: DeviceUiDriver, bundleName: string, sdk?: SdkWindow) {
    this.owner = owner; this.bundleName = bundleName; this.sdk = sdk;
  }
  async close(): Promise<void> {
    if (this.sdk !== undefined) { await this.sdk.close(); return; }
    // The platform window decoration is discovered from this bundle's actual layout.
    const close = await this.owner.waitForComponent(DeviceOn.id('EnhanceCloseBtn').type('Button').inWindow(this.bundleName), 10000);
    await close.click();
  }
}

/** Official UiTest SDK where available; CLI uses the same platform service with scoped layout and real OS input. */
export class DeviceUiDriver {
  readonly backend: string;
  private sequence: number = 0;
  private busy: boolean = false;
  private commandActive: boolean = false;
  private ownBundle: string;
  private runId: string;
  private sdk?: Driver;
  private isolatedCache?: string;
  private relay?: LayoutRelay;
  constructor(ownBundle: string, runId: string, sdk?: Driver, isolatedCache?: string) {
    this.ownBundle = ownBundle; this.runId = runId; this.sdk = sdk;
    check(/^[A-Za-z0-9_.]+$/.test(ownBundle), 'Invalid tested bundle name');
    check(/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(runId), 'Invalid UITest run namespace');
    if (isolatedCache !== undefined) {
      check(/^\/[A-Za-z0-9_./-]+$/.test(isolatedCache) && !isolatedCache.includes('//') &&
        !isolatedCache.split('/').some((part: string) => part === '.' || part === '..') &&
        isolatedCache.endsWith(`/haps/entry/cache/PhotoCraftTestRuns/${runId}/cache`),
        'UITest layout copies require the validated entry-module test cache');
    }
    this.isolatedCache = isolatedCache;
    this.backend = sdk === undefined || sdk === null ? 'uitest-cli' : 'uitest-sdk';
  }
  private bundle(selector: DeviceSelector): string {
    const bundle = selector.bundleName ?? this.ownBundle;
    check(bundle === this.ownBundle || bundle === 'com.huawei.hmos.filemanager', 'UITest selector is outside the application and system picker');
    return bundle;
  }
  private sdkSelector(selector: DeviceSelector): On {
    let result = SdkOn;
    if (selector.idValue !== undefined) { result = result.id(selector.idValue); }
    if (selector.textValue !== undefined) { result = result.text(selector.textValue); }
    if (selector.typeValue !== undefined) { result = result.type(selector.typeValue); }
    if (selector.focusedValue !== undefined) { result = result.focused(selector.focusedValue); }
    return result.inWindow(this.bundle(selector));
  }
  private async command(command: string): Promise<string> {
    check(!this.commandActive, 'Official UITest commands must be serial');
    this.commandActive = true;
    try {
      const result = await abilityDelegatorRegistry.getAbilityDelegator().executeShellCommand(command);
      const words = command.split(' ');
      const category = words[0] === 'uitest' ? words.slice(0, words[1] === 'uiInput' ? 3 : 2).join(' ') : 'copy-owned-layout';
      const inputCommand = words[0] === 'uitest' && words[1] === 'uiInput';
      // Official uiInput prints exception.message yet can return zero: successful input emits no text.
      if (result.exitCode !== 0 || (inputCommand && result.stdResult.trim().length > 0)) {
        await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_UI_COMMAND_ERROR ${JSON.stringify({
          runId: this.runId, category: category, exitCode: result.exitCode, stdout: result.stdResult.slice(0, 1000) })}\n`);
        throw new Error(`Official UITest ${category} failed (${result.exitCode}): ${result.stdResult}`);
      }
      return result.stdResult;
    } finally { this.commandActive = false; }
  }
  private async reportReadError(stage: string, error: LayoutReadError): Promise<void> {
    try {
      await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_UI_LAYOUT_READ ${JSON.stringify({
        runId: this.runId, stage: stage, code: error.code ?? -1, message: error.message ?? 'Unknown file API error' })}\n`);
    } catch (_printError) { /* Diagnostics must preserve the actual file/command failure. */ }
  }
  private async readLayout(path: string, bundle: string): Promise<string> {
    try { return await fileIo.readText(path, { length: 4 * 1024 * 1024 }); }
    catch (error) { await this.reportReadError('direct-read', error as LayoutReadError); }
    check(this.isolatedCache !== undefined, 'Owned layout is not readable and no validated isolated cache was provided');
    if (this.relay === undefined) { this.relay = new LayoutRelay(this.runId, this.ownBundle, this.isolatedCache as string); }
    return await this.relay.read(path, bundle);
  }
  async close(): Promise<void> { if (this.relay !== undefined) { await this.relay.close(); } }
  private async layout(bundle: string): Promise<UiNode> {
    check(!this.busy, 'UITest layout operations must be serial');
    this.busy = true;
    const path = `/data/local/tmp/PhotoCraftTest-${this.runId}-${Date.now()}-${this.sequence++}-query.json`;
    try {
      // Claim this fresh path before invoking the daemon so the host can clean up even if the command fails.
      await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_QUERY_FILE ${JSON.stringify({
        runId: this.runId, bundleName: bundle, path: path })}\n`);
      const output = await this.command(`uitest dumpLayout -b ${bundle} -p ${path}`);
      const outputKind = output.trim().length === 0 ? 'empty' :
        output.trim() === `DumpLayout saved to:${path}` ? 'saved-owned-path' : 'unexpected';
      await abilityDelegatorRegistry.getAbilityDelegator().print(`PHOTOCRAFT_UI_LAYOUT_DUMP ${JSON.stringify({
        runId: this.runId, bundleName: bundle, exitCode: 0, outputKind: outputKind, outputLength: output.length })}\n`);
      // Some API 26 observers omit the tool's message. A fresh, complete, validated JSON read is required below.
      check(outputKind !== 'unexpected', 'UITest returned an unexpected layout output message');
      const serialized = await this.readLayout(path, bundle);
      check(serialized.length > 0 && serialized.length <= 4 * 1024 * 1024, 'Owned layout JSON is empty or exceeds the size limit');
      const root = JSON.parse(serialized) as UiNode;
      check(root.attributes !== undefined, 'Owned layout is not a complete UITest JSON tree');
      return root;
    } finally { this.busy = false; }
  }
  private async matches(selector: DeviceSelector): Promise<NodeMatch[]> {
    const bundle = this.bundle(selector);
    const root = await this.layout(bundle);
    const matches: NodeMatch[] = [];
    const walk = (node: UiNode, inheritedBundle: string, inheritedBounds: string): void => {
      const attributes = node.attributes;
      check(attributes !== undefined, 'UITest layout node has no attributes');
      if (attributes.visible === 'false') { return; }
      const currentBundle = attributes.bundleName || inheritedBundle;
      const windowBounds = attributes.bundleName && attributes.type === 'root' ? (attributes.bounds ?? '') : inheritedBounds;
      if (currentBundle === bundle && (selector.idValue === undefined || attributes.id === selector.idValue) &&
        (selector.textValue === undefined || attributes.text === selector.textValue) &&
        (selector.typeValue === undefined || attributes.type === selector.typeValue) &&
        (selector.focusedValue === undefined || attributes.focused === String(selector.focusedValue))) {
        matches.push({ attributes: attributes, bundleName: currentBundle, windowBounds: windowBounds });
      }
      for (const child of node.children ?? []) { walk(child, currentBundle, windowBounds); }
    };
    walk(root, '', '');
    return matches;
  }
  async findComponents(selector: DeviceSelector): Promise<DeviceComponent[]> {
    if (this.backend === 'uitest-sdk') {
      const components = await (this.sdk as Driver).findComponents(this.sdkSelector(selector));
      return components.map((component: SdkComponent) => new DeviceComponent(this, selector, component));
    }
    return (await this.matches(selector)).map((_match: NodeMatch) => new DeviceComponent(this, selector));
  }
  async findComponent(selector: DeviceSelector): Promise<DeviceComponent | undefined> {
    if (this.backend === 'uitest-sdk') {
      const component = await (this.sdk as Driver).findComponent(this.sdkSelector(selector));
      return component === undefined || component === null ? undefined : new DeviceComponent(this, selector, component);
    }
    const matches = await this.matches(selector);
    check(matches.length <= 1, 'UITest selector must match a unique component');
    return matches.length === 0 ? undefined : new DeviceComponent(this, selector);
  }
  async waitForComponent(selector: DeviceSelector, timeout: number): Promise<DeviceComponent> {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
      const component = await this.findComponent(selector);
      if (component !== undefined) { return component; }
      await new Promise<void>((resolve: () => void) => { setTimeout(resolve, 100); });
    }
    throw new Error('UITest timed out waiting for the scoped component');
  }
  async componentAction(selector: DeviceSelector, action: string): Promise<void> {
    check(action === 'click' || action === 'doubleClick', 'Unsupported UITest component action');
    const matches = await this.matches(selector);
    check(matches.length === 1, 'UITest action requires one currently visible matched component');
    check(matches[0].attributes.enabled !== 'false', 'UITest action cannot target a disabled component');
    const point = center(matches[0]);
    await this.command(`uitest uiInput ${action} ${point[0]} ${point[1]}`);
  }
  async componentInput(selector: DeviceSelector, text: string): Promise<void> {
    check(text.length > 0 && text.length <= 128, 'UITest fixture text exceeds the input limit');
    const quoted = quote(text);
    const matches = await this.matches(selector);
    check(matches.length === 1 && matches[0].attributes.type === 'TextInput', 'UITest text input requires one actual TextInput');
    check(matches[0].attributes.enabled !== 'false', 'UITest action cannot target a disabled component');
    const point = center(matches[0]);
    await this.command(`uitest uiInput click ${point[0]} ${point[1]}`);
    await this.command('uitest uiInput keyEvent 2072 2017'); // Actual Ctrl+A selects the filename/folder editor contents.
    await this.command(`uitest uiInput text ${quoted}`);
  }
  async triggerKey(code: number): Promise<void> {
    check(Number.isInteger(code) && code >= 0 && code <= 10000, 'Invalid UITest key code');
    if (this.backend === 'uitest-sdk') { await (this.sdk as Driver).triggerKey(code); return; }
    await this.command(`uitest uiInput keyEvent ${code}`);
  }
  async pressBack(): Promise<void> {
    if (this.backend === 'uitest-sdk') { await (this.sdk as Driver).pressBack(); return; }
    await this.command('uitest uiInput keyEvent Back');
  }
  async findWindow(filter: DeviceWindowFilter): Promise<DeviceWindow | undefined> {
    check(filter.bundleName === this.ownBundle, 'Window operations are restricted to the tested application');
    if (this.backend === 'uitest-sdk') {
      const window = await (this.sdk as Driver).findWindow(filter);
      return window === undefined || window === null ? undefined : new DeviceWindow(this, filter.bundleName, window);
    }
    const matches = await this.matches(new DeviceSelector().inWindow(filter.bundleName));
    return matches.length === 0 ? undefined : new DeviceWindow(this, filter.bundleName);
  }
}
