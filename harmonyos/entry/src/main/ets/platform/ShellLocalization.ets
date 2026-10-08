/** The host supplies the current resource manager; pure transaction code stays platform independent. */
export type ShellStringResolver = (key: string) => string;

let shellStringResolver: ShellStringResolver | undefined = undefined;

export function configureShellLocalization(resolver: ShellStringResolver | undefined): void {
  shellStringResolver = resolver;
}

/** Resolve on every use so messages follow the current system locale, with an English fallback. */
export function shellString(key: string, fallback: string, ...values: (string | number)[]): string {
  let text = fallback;
  if (shellStringResolver !== undefined) {
    try {
      text = shellStringResolver(key);
    } catch (_) {
      // Missing resources must not prevent file recovery or replace the original operation failure.
    }
  }
  return text.replace(/\{(\d+)\}/g, (placeholder: string, index: string): string => {
    const position = Number(index);
    return position < values.length ? String(values[position]) : placeholder;
  });
}
