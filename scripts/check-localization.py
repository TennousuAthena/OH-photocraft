#!/usr/bin/env python3
"""Check Chinese catalogs without requiring the native Rust/egui toolchain.

The Rust i18n tests remain authoritative for engine menus, blend mode and generated
preference labels. This check catches malformed catalog edits and missing literal
UI labels and static menus, including the OHOS overlay and native adapter.
"""

from collections import Counter
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
UI_SOURCE = ROOT / "vendor/photocraft-ui-egui/src"
NATIVE_SOURCE = ROOT / "apps/photocraft-ohos/src"


def unescape(value: str) -> str:
    """The catalog's three escapes; unknown escapes remain literal."""
    return re.sub(r"\\([nt\\])", lambda m: {"n": "\n", "t": "\t", "\\": "\\"}[m[1]], value)


def placeholders(value: str) -> Counter:
    return Counter(re.findall(r"\{([^{}]*)\}", value))


def literal_labels() -> set[str]:
    labels = set()
    for path in UI_SOURCE.rglob("*.rs"):
        if path.name == "lib.rs":
            continue
        # Follow the existing Rust coverage test: exclude a trailing test module,
        # while retaining production items after standalone #[cfg(test)] items.
        source = path.read_text().replace("\r\n", "\n").split("#[cfg(test)]\nmod ")[0]
        for value in re.findall(r'tl!\("((?:[^"\\]|\\.)*)"\)', source):
            labels.add(unescape(value.replace(r'\"', '"')))
    if len(labels) < 300:
        raise ValueError(f"literal scan found only {len(labels)} labels")
    for path in NATIVE_SOURCE.rglob("*.rs"):
        source = path.read_text().replace("\r\n", "\n").split("#[cfg(test)]\nmod ")[0]
        values = re.findall(r'i18n::t\(\s*"((?:[^"\\]|\\.)*)"\s*\)', source)
        # Native states and command labels translate one of several fixed match
        # arms. Error returns in these matches remain protocol/internal errors.
        for arms in re.findall(r'i18n::t\(match [^{]+\{(.*?)\}\s*\)', source, re.S):
            values.extend(re.findall(r'=>\s*"((?:[^"\\]|\\.)*)"', arms))
        # Recovery records keep English progress until the UI reads them.
        values.extend(re.findall(r'progress:\s*"((?:[^"\\]|\\.)*)"', source))
        labels.update(unescape(value.replace(r'\"', '"')) for value in values)
    return labels


def menu_labels() -> set[str]:
    labels = set()
    specifications = (
        ("menu_catalog.rs", "CATALOG", r'\(\s*&\[(?P<path>[^\]]*)\]\s*,\s*"(?P<label>(?:[^"\\]|\\.)*)"', 600),
        ("menus.rs", "UI_COMMANDS", r'\(\s*"(?:[^"\\]|\\.)*"\s*,\s*"(?P<label>(?:[^"\\]|\\.)*)"\s*,\s*&\[(?P<path>[^\]]*)\]', 40),
    )
    for filename, name, pattern, minimum in specifications:
        source = (UI_SOURCE / filename).read_text()
        declaration = re.search(rf'(?:static|const)\s+{name}\b[^=]*=\s*&?\[(.*?)\n\];', source, re.S)
        if declaration is None:
            raise ValueError(f"could not find {filename}:{name}")
        entries = list(re.finditer(pattern, declaration[1]))
        if len(entries) < minimum:
            raise ValueError(f"{filename}:{name}: menu scan found only {len(entries)} entries")
        for entry in entries:
            labels.add(unescape(entry["label"].replace(r'\"', '"')))
            labels.update(unescape(value.replace(r'\"', '"')) for value in re.findall(r'"((?:[^"\\]|\\.)*)"', entry["path"]))
    labels.discard("---")
    return labels


def check_catalog(language: str, labels: set[str]) -> list[str]:
    path = UI_SOURCE / "i18n" / f"{language}.tsv"
    failures = []
    entries = {}
    plain = set()
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        columns = line.split("\t")
        if len(columns) != 3 or not columns[1] or not columns[2]:
            failures.append(f"{language}:{number}: expected three nonempty source/translation columns")
            continue
        context, source, translation = map(unescape, columns)
        key = context, source
        if key in entries:
            failures.append(f"{language}:{number}: duplicate entry {key!r}")
        entries[key] = translation
        if not context:
            plain.add(source)
            if source.endswith("…") != translation.endswith("…"):
                failures.append(f"{language}:{number}: trailing ellipsis differs for {source!r}")
        if context == "@plural":
            forms = source.split("|")
            if len(forms) != 2 or len(translation.split("|")) != 1:
                failures.append(f"{language}:{number}: expected two English forms and one Chinese form")
                continue
            source = forms[1]
        if placeholders(source) != placeholders(translation):
            failures.append(f"{language}:{number}: placeholders differ for {source!r}")
    failures.extend(f"{language}: missing UI label {label!r}" for label in sorted(labels - plain))
    if not failures:
        print(f"{language}: {len(entries)} valid entries; all {len(labels)} required UI labels translated")
    return failures


def main() -> None:
    labels = literal_labels() | menu_labels()
    failures = [failure for language in ("zh-hans", "zh-hant") for failure in check_catalog(language, labels)]
    if failures:
        print("\n".join(failures), file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
