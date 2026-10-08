#!/usr/bin/env python3
"""Fail-closed host and aa instrument-test runner, with JSON and JUnit reports."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import time
import uuid
import xml.etree.ElementTree as ET
import zipfile

from build_device_tests import BuildError, build, clone_file, json5_load, reuse_artifacts
from layout_file_relay import FileLayoutRelay
from stream_process import StreamError, stream_process


class TestError(RuntimeError):
    pass


STORAGE_ROOT_LABELS = {'application.files', 'application.cache', 'entry.files', 'entry.cache'}
STORAGE_ROOT_FIELDS = {'application.files': 'applicationFilesDir', 'application.cache': 'applicationCacheDir',
                       'entry.files': 'entryFilesDir', 'entry.cache': 'entryCacheDir'}
TEST_INTERFACE_MARKERS = ('testSubmit', 'testPoll', 'testSnapshot', 'craft_device_tests_abi_v1')
PHOTOCRAFT_APP_NAME = 'PhotoCraft'
PHOTOCRAFT_RUST_PACKAGE = 'photocraft-ohos'
PHOTOCRAFT_NATIVE_LIBRARY = 'libphotocraft.so'
PHOTOCRAFT_DEVICE_CASES = {
    'PhotoCraftCore': ('new_edit_undo_redo', 'dialog_cancel_and_two_documents', 'synthetic_and_actual_keyboard',
                      'synthetic_and_actual_chinese_text', 'background_foreground'),
    'PhotoCraftFiles': ('pcraft_system_roundtrip', 'psd_system_roundtrip', 'png_system_roundtrip',
                       'system_picker_cancel', 'two_documents_alternate_system_save', 'recent_reimports_fresh_external_bytes'),
    'PhotoCraftRecovery': ('seed_autosave', 'verify_autosave'),
    'PhotoCraftClose': ('cancel_window_close', 'save_window_close'),
}
PHOTOCRAFT_DEVICE_SUITES = set(PHOTOCRAFT_DEVICE_CASES)


def requested_device_cases(scope: str) -> set[tuple[str, str]]:
    expected = set()
    for value in scope.split(','):
        suite, separator, case = value.partition('#')
        names = PHOTOCRAFT_DEVICE_CASES.get(suite)
        if names is None or separator and case not in names:
            raise TestError('Scope does not select a registered PhotoCraft device suite/case: ' + value)
        selected = {(suite, name) for name in ((case,) if separator else names)}
        if expected & selected:
            raise TestError('Device scope selects duplicate registered cases.')
        expected.update(selected)
    return expected


def validate_device_case_inventory(scope: str, cases: list[dict]) -> dict:
    try:
        expected = requested_device_cases(scope)
    except TestError as error:
        return {'passed': False, 'expected': [], 'defects': [str(error)]}
    actual = [(case['suite'], case['name']) for case in cases]
    defects = []
    for suite, name in sorted(expected - set(actual)):
        defects.append(f'Missing registered case: {suite}#{name}.')
    for suite, name in sorted(set(actual) - expected):
        defects.append(f'Unexpected case outside requested inventory: {suite}#{name}.')
    if len(actual) != len(set(actual)):
        defects.append('Repeated device case in the instrument result.')
    return {'passed': not defects, 'expected': [{'suite': suite, 'name': name} for suite, name in sorted(expected)], 'defects': defects}


def inspect_hap_interfaces(hap: Path, library_name: str) -> dict:
    native_path = f'libs/arm64-v8a/{library_name}'
    try:
        with hap.open('rb') as stream:
            hap_hash = hashlib.file_digest(stream, 'sha256').hexdigest()
        with zipfile.ZipFile(hap) as archive:
            candidates = [item for item in archive.infolist() if item.filename == native_path]
            if len(candidates) != 1:
                raise TestError('HAP must contain exactly one expected arm64 native library.')
            native_hash = hashlib.sha256()
            markers = {marker: False for marker in TEST_INTERFACE_MARKERS}
            overlap = b''
            first_chunk = True
            with archive.open(candidates[0]) as stream:
                while chunk := stream.read(1024 * 1024):
                    if first_chunk and not chunk.startswith(b'\x7fELF'):
                        raise TestError('HAP native library is not an ELF binary.')
                    first_chunk = False
                    native_hash.update(chunk)
                    window = overlap + chunk
                    for marker in markers:
                        markers[marker] = markers[marker] or marker.encode() in window
                    overlap = window[-max(len(marker) for marker in markers):]
            if first_chunk:
                raise TestError('HAP native library is empty.')
            return {'hapSha256': hap_hash, 'nativeLibrary': native_path,
                    'nativeSha256': native_hash.hexdigest(), 'nativeBytes': candidates[0].file_size,
                    'elf': True, 'testInterfaceMarkers': markers}
    except TestError:
        raise
    except (OSError, zipfile.BadZipFile, RuntimeError) as error:
        raise TestError(f'Cannot inspect signed HAP/native interface markers: {type(error).__name__}.') from None


def require_hap_interface_mode(evidence: dict, *, device_tests: bool):
    markers = evidence['testInterfaceMarkers']
    if device_tests and not all(markers.values()):
        raise TestError('Device-test main HAP is missing one or more native test interface markers.')
    if not device_tests and any(markers.values()):
        raise TestError('Ordinary restoration HAP contains a native test interface marker; installation refused.')


def storage_root_coverage(fingerprint: dict) -> list[tuple[tuple[str, ...], str, str, int, int]]:
    """Require both application and entry roots, merging aliases of one path."""
    roots = fingerprint.get('roots')
    if not isinstance(roots, list) or not roots or len(roots) > len(STORAGE_ROOT_LABELS):
        raise ValueError('Storage evidence has no complete application/entry root coverage.')
    labels_seen = set()
    paths_seen = set()
    normalized = []
    for root in roots:
        if not isinstance(root, dict):
            raise ValueError('Storage root metadata is not an object.')
        labels = root.get('labels')
        if not isinstance(labels, list) or not labels or any(not isinstance(label, str) for label in labels):
            raise ValueError('Storage root labels are invalid.')
        if len(set(labels)) != len(labels) or any(label not in STORAGE_ROOT_LABELS or label in labels_seen for label in labels):
            raise ValueError('Storage root labels are duplicated or unknown.')
        path_digest = root.get('pathDigest')
        if not isinstance(path_digest, str) or not re.fullmatch(r'[a-fA-F0-9]{64}', path_digest):
            raise ValueError('Storage root path digest is not a SHA256.')
        path_digest = path_digest.lower()
        if path_digest in paths_seen:
            raise ValueError('Storage roots for the same path must merge their labels.')
        labels_seen.update(labels)
        paths_seen.add(path_digest)
        digest = root.get('digest')
        if not isinstance(digest, str) or not re.fullmatch(r'[a-fA-F0-9]{64}', digest):
            raise ValueError('Storage root content digest is not a SHA256.')
        if any(type(root.get(key)) is not int or root[key] < 0 for key in ('entries', 'bytes')):
            raise ValueError('Storage root entry/byte count is invalid.')
        normalized.append((tuple(sorted(labels)), path_digest, digest.lower(), root['entries'], root['bytes']))
    if labels_seen != STORAGE_ROOT_LABELS:
        raise ValueError('Storage evidence does not cover both application and entry files/cache roots.')
    if sum(root[3] for root in normalized) != fingerprint.get('entries') or sum(root[4] for root in normalized) != fingerprint.get('bytes'):
        raise ValueError('Storage root counts do not match the aggregate fingerprint.')
    return sorted(normalized)


def parse_scope(value: str | None) -> tuple[str, str]:
    if value is None:
        return '', ''
    parts = value.split('#')
    if len(parts) > 2 or not parts[0] or (len(parts) == 2 and not parts[1]):
        raise argparse.ArgumentTypeError('Scope must be Suite or Suite#case.')
    if any('\n' in part or '\r' in part or '\x00' in part for part in parts):
        raise argparse.ArgumentTypeError('Scope must not contain control characters.')
    return parts[0], parts[1] if len(parts) == 2 else ''


def parse_testmode(output: str) -> dict:
    raw = output.strip()
    missing = 'Get parameter "persist.ace.testmode.enabled" fail! errNum is:1002!'
    if raw == missing:
        return {'raw': raw, 'readable': False, 'value': None, 'enabled': None}
    if raw in ('0', '1'):
        return {'raw': raw, 'readable': True, 'value': raw, 'enabled': raw == '1'}
    raise TestError('Cannot safely read/restore UiTest testmode value.')


def parse_isolation_evidence(output: str, run_id: str, scope: str) -> dict:
    expected = {selector.split('#', 1)[0] for selector in scope.split(',')}
    evidence = []
    defects = []
    for line in output.splitlines():
        marker = re.search(r'PHOTOCRAFT_ISOLATION\s+(\{.*\})\s*$', line)
        if not marker:
            continue
        try:
            item = json.loads(marker.group(1))
            if not isinstance(item, dict):
                raise ValueError('Evidence is not an object.')
            before, after = item.get('before'), item.get('after')
            if not isinstance(before, dict) or not isinstance(after, dict):
                raise ValueError('Evidence has no before/after fingerprint.')
            for fingerprint in (before, after):
                if not isinstance(fingerprint.get('digest'), str) or not re.fullmatch(r'[a-fA-F0-9]{64}', fingerprint['digest']):
                    raise ValueError('Storage digest is not a SHA256.')
                if any(type(fingerprint.get(key)) is not int or fingerprint[key] < 0 for key in ('entries', 'bytes')):
                    raise ValueError('Storage entry/byte count is invalid.')
            roots_before = storage_root_coverage(before)
            roots_after = storage_root_coverage(after)
            if item.get('runId') != run_id:
                raise ValueError('Storage evidence uses a different run ID.')
            if item.get('suite') not in expected:
                raise ValueError('Storage evidence uses an unrequested suite.')
            # Keep validated before/after metadata and diagnostic differences
            # even when the unchanged assertion fails.
            evidence.append(item)
            if roots_before != roots_after:
                raise ValueError('Ordinary storage root mapping or contents changed during device tests.')
            if item.get('unchanged') is not True or any(before[key] != after[key] for key in ('digest', 'entries', 'bytes')):
                raise ValueError('Ordinary storage changed during device tests.')
        except (ValueError, KeyError, TypeError) as error:
            defects.append(str(error))
    for suite in expected:
        count = sum(item.get('suite') == suite for item in evidence)
        if count != 1:
            defects.append(f'Expected exactly one ordinary-storage fingerprint for {suite}, found {count}.')
    return {'passed': not defects, 'evidence': evidence, 'defects': defects}


def local_host_packages(metadata: dict, app_name: str) -> dict:
    members = set(metadata.get('workspace_members', []))
    local = [package for package in metadata.get('packages', [])
             if package.get('source') is None and package.get('id') in members]
    def one(name: str, required: bool):
        matches = [package['id'] for package in local if package.get('name') == name]
        if len(matches) > 1 or required and not matches:
            raise TestError(f'Cannot select an unambiguous local workspace package for {name}.')
        return matches[0] if matches else None
    return {'app': one(app_name, True), 'platform': one('craft-ohos-platform', True), 'facade': one('eframe', False)}


def node_test_files(project: Path) -> list[Path]:
    files = set(project.glob('tests/**/*.test.cjs'))
    for name in ('test-file-contracts.cjs', 'test-group-publication.cjs'):
        script = project / 'scripts' / name
        if script.is_file():
            files.add(script)
    return sorted(files)


def parse_runtime_evidence(output: str, run_id: str, scope: str) -> dict:
    expected = {selector.split('#', 1)[0] for selector in scope.split(',')}
    records = []
    defects = []
    for line in output.splitlines():
        marker = re.search(r'PHOTOCRAFT_RUNTIME\s+(\{.*\})\s*$', line)
        if not marker:
            continue
        try:
            item = json.loads(marker.group(1))
            if not isinstance(item, dict) or item.get('runId') != run_id or item.get('suite') not in expected:
                raise ValueError('Runtime PID evidence does not match run ID and scope.')
            if type(item.get('pid')) is not int or not 0 < item['pid'] <= 2147483647:
                raise ValueError('Runtime PID evidence has an invalid PID.')
            records.append(item)
        except (ValueError, TypeError) as error:
            defects.append(str(error))
    return {'records': records, 'defects': defects}


def parse_storage_roots_metadata(output: str, run_id: str, scope: str, isolation: dict) -> dict:
    expected = {selector.split('#', 1)[0] for selector in scope.split(',')}
    records = []
    defects = []
    for line in output.splitlines():
        marker = re.search(r'PHOTOCRAFT_STORAGE_ROOTS\s+(\{.*\})\s*$', line)
        if not marker:
            continue
        try:
            item = json.loads(marker.group(1))
            if not isinstance(item, dict) or item.get('runId') != run_id or item.get('suite') not in expected:
                raise ValueError('Storage root metadata does not match run ID and scope.')
            path_hashes = {}
            for label, field in STORAGE_ROOT_FIELDS.items():
                path = item.get(field)
                if not isinstance(path, str) or not Path(path).is_absolute() or any(char in path for char in '\0\n\r'):
                    raise ValueError('Storage root metadata has an invalid absolute path.')
                path_hashes[label] = hashlib.sha256(path.encode()).hexdigest()
            for evidence in isolation['evidence']:
                if evidence['suite'] != item['suite']:
                    continue
                for fingerprint in (evidence['before'], evidence['after']):
                    for root in fingerprint['roots']:
                        if any(path_hashes[label] != root['pathDigest'].lower() for label in root['labels']):
                            raise ValueError('Storage root path metadata differs from the isolation fingerprint.')
            if any(record['suite'] == item['suite'] for record in records):
                raise ValueError('Duplicate storage root metadata for one suite.')
            records.append(item)
        except (ValueError, KeyError, TypeError) as error:
            defects.append(str(error))
    return {'records': records, 'defects': defects}


def parse_cache_inventory(output: str, run_id: str, scope: str) -> dict:
    expected = {selector.split('#', 1)[0] for selector in scope.split(',')}
    records = []
    defects = []
    for line in output.splitlines():
        marker = re.search(r'PHOTOCRAFT_CACHE_INVENTORY\s+(\{.*\})\s*$', line)
        if not marker:
            continue
        try:
            item = json.loads(marker.group(1))
            if not isinstance(item, dict) or item.get('runId') != run_id or item.get('suite') not in expected:
                raise ValueError('Cache metadata does not match run ID and scope.')
            labels = item.get('rootLabels')
            if not isinstance(labels, list) or not labels or any(not isinstance(label, str) for label in labels):
                raise ValueError('Cache inventory has no root labels.')
            if len(set(labels)) != len(labels) or any(label not in STORAGE_ROOT_LABELS for label in labels) or not any(label.endswith('.cache') for label in labels):
                raise ValueError('Cache inventory root labels are duplicated, unknown, or do not cover cache.')
            relative = item.get('relativePath')
            if not isinstance(relative, str) or not relative or '\x00' in relative or Path(relative).is_absolute() or '..' in Path(relative).parts:
                raise ValueError('Cache inventory path is not a safe relative path.')
            if type(item.get('size')) is not int or item['size'] < 0:
                raise ValueError('Cache inventory size is invalid.')
            if any(not isinstance(item.get(key), str) or not re.fullmatch(r'[a-fA-F0-9]{64}', item[key]) for key in ('hash', 'pathDigest')):
                raise ValueError('Cache inventory hash is invalid.')
            records.append(item)
        except (ValueError, TypeError) as error:
            defects.append(str(error))
    return {'records': records, 'defects': defects}


def parse_failure_evidence(output: str, run_id: str, scope: str) -> dict:
    expected = {selector.split('#', 1)[0] for selector in scope.split(',')}
    records = []
    defects = []
    paths_seen = set()
    for line in output.splitlines():
        marker = re.search(r'PHOTOCRAFT_FAILURE_EVIDENCE\s+(\{.*\})\s*$', line)
        if not marker:
            continue
        try:
            item = json.loads(marker.group(1))
            if not isinstance(item, dict) or item.get('runId') != run_id or item.get('suite') not in expected:
                raise ValueError('Failure evidence does not match run ID and scope.')
            if not isinstance(item.get('label'), str) or '\0' in item['label']:
                raise ValueError('Failure evidence label is invalid.')
            stamps = set()
            paths = []
            for field, suffix in (('screenPath', 'png'), ('layoutPath', 'json')):
                if field not in item:
                    continue
                path = item[field]
                match = re.fullmatch(r'/data/local/tmp/PhotoCraftTest-' + re.escape(run_id) + r'-([0-9]{1,20})-failure\.' + suffix,
                                     path) if isinstance(path, str) else None
                if not match or path in paths_seen:
                    raise ValueError('Failure evidence path is not a unique file owned by this phase.')
                stamps.add(match.group(1))
                paths.append(path)
            if not paths or len(stamps) != 1:
                raise ValueError('Failure evidence has no capture or its timestamp prefixes differ.')
            paths_seen.update(paths)
            records.append({**item, 'timestamp': next(iter(stamps))})
        except (ValueError, TypeError) as error:
            defects.append(str(error))
    return {'records': records, 'defects': defects}


def parse_query_files(output: str, run_id: str, bundle: str) -> dict:
    records = []
    defects = []
    paths_seen = set()
    for line in output.splitlines():
        marker = re.search(r'PHOTOCRAFT_QUERY_FILE\s+(\{.*\})\s*$', line)
        if not marker:
            continue
        try:
            item = json.loads(marker.group(1))
            if not isinstance(item, dict) or item.get('runId') != run_id:
                raise ValueError('Query file metadata does not match the current run ID.')
            if item.get('bundleName') not in (bundle, 'com.huawei.hmos.filemanager'):
                raise ValueError('Query file metadata uses an unrequested bundle.')
            path = item.get('path')
            pattern = r'/data/local/tmp/PhotoCraftTest-' + re.escape(run_id) + r'-[0-9]{1,20}-[0-9]{1,20}-query\.json'
            if not isinstance(path, str) or not re.fullmatch(pattern, path) or path in paths_seen:
                raise ValueError('Query path is not a unique file owned by this phase.')
            paths_seen.add(path)
            records.append(item)
        except (ValueError, TypeError) as error:
            defects.append(str(error))
    return {'records': records, 'defects': defects}


def parse_ui_driver_metadata(output: str, run_id: str, scope: str) -> dict:
    expected = {selector.split('#', 1)[0] for selector in scope.split(',')}
    records = []
    defects = []
    for line in output.splitlines():
        marker = re.search(r'PHOTOCRAFT_UI_DRIVER\s+(\{.*\})\s*$', line)
        if not marker:
            continue
        try:
            item = json.loads(marker.group(1))
            if not isinstance(item, dict) or item.get('runId') != run_id or item.get('suite') not in expected:
                raise ValueError('UI driver metadata does not match run ID and scope.')
            if item.get('backend') not in ('uitest-cli', 'uitest-sdk') or type(item.get('available')) is not bool:
                raise ValueError('UI driver backend metadata is invalid.')
            if item.get('phase') != 'after-ability-start' or not isinstance(item.get('error'), str):
                raise ValueError('UI driver metadata has an invalid phase/error.')
            if item['backend'] == 'uitest-sdk' and item['available'] is not True:
                raise ValueError('UI driver metadata claims an unavailable SDK backend.')
            records.append(item)
        except (ValueError, TypeError) as error:
            defects.append(str(error))
    return {'records': records, 'defects': defects}


def parse_instrument_output(output: str) -> dict:
    """Validate official Hypium protocol, not aa/hdc's often-successful exit code."""
    cases: list[dict] = []
    fields: dict[str, str] = {}
    started: set[tuple[str, str]] = set()
    completed: set[tuple[str, str]] = set()
    defects: list[str] = []
    summary = None
    final_code = None
    for line in output.splitlines():
        match = re.search(r'OHOS_REPORT_STATUS:\s*([^=]+)=(.*)', line)
        if match:
            key, value = match.group(1).strip(), match.group(2).strip()
            fields[key] = value
            if key == 'consuming' and cases:
                try:
                    cases[-1]['durationMs'] = float(value)
                except ValueError:
                    defects.append('Invalid case duration.')
        match = re.search(r'OHOS_REPORT_STATUS_CODE:\s*(-?\d+)', line)
        if match:
            code = int(match.group(1))
            identity = fields.get('class', ''), fields.get('test', '')
            if not all(identity):
                defects.append('Case status has no suite/test identity.')
                continue
            if code == 1:
                if identity in started:
                    defects.append(f'Duplicate case start: {identity[0]}#{identity[1]}.')
                started.add(identity)
            elif code in (0, -1, -2, -3):
                if identity not in started:
                    defects.append(f'Case completion without start: {identity[0]}#{identity[1]}.')
                if identity in completed:
                    defects.append(f'Duplicate case completion: {identity[0]}#{identity[1]}.')
                completed.add(identity)
                reason = fields.get('stream', '').strip()
                stack = fields.get('stack', '').strip()
                message = reason + ('\n' + stack if reason and stack and reason != stack else '') if reason else stack
                cases.append({'suite': identity[0], 'name': identity[1],
                    'status': {0: 'passed', -1: 'error', -2: 'failed', -3: 'skipped'}[code],
                    'durationMs': 0, 'message': message, 'stack': stack})
                fields.pop('stack', None)
                fields.pop('stream', None)
            else:
                defects.append(f'Unknown case status {code}.')
        match = re.search(r'OHOS_REPORT_RESULT:\s*stream=Tests run:\s*(\d+),\s*Failure:\s*(\d+),\s*Error:\s*(\d+),\s*Pass:\s*(\d+)(?:,\s*Ignore:\s*(\d+))?', line)
        if match:
            if summary is not None:
                defects.append('Multiple final summaries.')
            total, failed, errors, passed, skipped = (int(value or '0') for value in match.groups())
            summary = {'total': total, 'failed': failed, 'errors': errors, 'passed': passed, 'skipped': skipped}
        match = re.search(r'OHOS_REPORT_CODE:\s*(-?\d+)', line)
        if match:
            if final_code is not None:
                defects.append('Multiple final report codes.')
            final_code = int(match.group(1))
    if summary is None or final_code is None:
        defects.append('Missing final test result or report code (timeout/crash/runner startup failure).')
    else:
        if summary['total'] <= 0 or summary['passed'] <= 0:
            defects.append('Zero tests executed or zero tests passed.')
        if summary['failed'] or summary['errors'] or summary['skipped']:
            defects.append('Failure, error, or ignored cases prevent device acceptance.')
        if final_code != 0:
            defects.append(f'Final report code was {final_code}.')
        counts = {status: sum(case['status'] == status for case in cases)
                  for status in ('passed', 'failed', 'error', 'skipped')}
        if len(cases) != summary['total'] or sum(summary[k] for k in ('passed', 'failed', 'errors', 'skipped')) != summary['total']:
            defects.append('Case count does not agree with final summary.')
        if any(counts[a] != summary[b] for a, b in
               (('passed', 'passed'), ('failed', 'failed'), ('error', 'errors'), ('skipped', 'skipped'))):
            defects.append('Case statuses do not agree with final summary.')
    if started != completed:
        defects.append('One or more started cases never completed.')
    if any(case['status'] in ('failed', 'error', 'skipped') for case in cases):
        defects.append('At least one case did not pass.')
    return {'passed': not defects, 'summary': summary, 'cases': cases, 'defects': defects}


def parse_host_output(output: str, kind: str) -> dict:
    if kind == 'rust':
        summaries = re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;', output)
        total = sum(int(p) + int(f) + int(i) for _, p, f, i in summaries)
        failed = sum(int(f) for _, _, f, _ in summaries)
        cases = [{'suite': 'Rust', 'name': name, 'status': 'passed' if state == 'ok' else ('skipped' if state == 'ignored' else 'failed'), 'durationMs': 0}
                 for name, state in re.findall(r'^test (.+?) \.\.\. (ok|FAILED|ignored)\s*$', output, re.M)]
        return {'total': total, 'failed': failed, 'cases': cases, 'valid': bool(summaries) and total > 0}
    if kind == 'tap':
        plans = re.findall(r'^1\.\.(\d+)\s*$', output, re.M)
        cases = []
        numbers = []
        for line in output.splitlines():
            match = re.fullmatch(r'(ok|not ok)\s+(\d+)\s+-\s+(.+)', line)
            if match:
                state, number, name = match.groups()
                numbers.append(int(number))
                cases.append({'suite': 'Native', 'name': name,
                    'status': 'failed' if state == 'not ok' or re.search(r'#\s*(SKIP|TODO)\b', name, re.I) else 'passed', 'durationMs': 0})
        total = int(plans[0]) if len(plans) == 1 else 0
        valid = total > 0 and numbers == list(range(1, total + 1)) and not re.search(r'^Bail out!', output, re.M)
        return {'total': total, 'failed': sum(case['status'] != 'passed' for case in cases), 'cases': cases, 'valid': valid}
    totals = re.findall(r'^# tests (\d+)\s*$', output, re.M)
    failures = re.findall(r'^# fail (\d+)\s*$', output, re.M)
    passed = re.findall(r'^# pass (\d+)\s*$', output, re.M)
    cases = []
    for line in output.splitlines():
        match = re.match(r'^(ok|not ok) \d+ - (.+)$', line)
        if match:
            prefix, name = match.groups()
            cases.append({'suite': 'Node', 'name': name,
                'status': 'failed' if prefix == 'not ok' else ('skipped' if '# SKIP' in name else 'passed'), 'durationMs': 0})
        match = re.match(r'^\s+duration_ms:\s*([0-9.]+)', line)
        if match and cases:
            cases[-1]['durationMs'] = float(match.group(1))
    return {'total': int(totals[-1]) if totals else 0, 'failed': int(failures[-1]) if failures else 0,
            'cases': cases, 'valid': bool(totals and failures and passed) and int(passed[-1]) > 0,
            'embeddedContractCounts': [int(count) for count in re.findall(r'SUMMARY (\d+) production ArkTS contracts passed', output)]}


def aa_arguments(bundle: str, run_id: str, scope: str | None, case_timeout: int = 60000) -> list[str]:
    argv = ['shell', 'aa', 'test', '-b', bundle, '-m', 'entry_test', '-s', 'unittest',
            'OpenHarmonyTestRunner', '-s', 'timeout', str(case_timeout), '-s', 'coverage', 'false',
            '-s', 'photocraftTestRunId', run_id, '-w', '600000']
    if scope:
        argv += ['-s', 'class', scope]
    return argv


def remote_hdc_arguments(argv: list[str]) -> list[str]:
    # hdc joins shell arguments before sending them to /system/bin/sh.
    return ['shell', shlex.join(argv[1:])] if argv and argv[0] == 'shell' else argv


@contextmanager
def device_lock(repo: Path, serial: str):
    directory = repo / '.cache/tests/locks'
    directory.mkdir(parents=True, exist_ok=True)
    filename = hashlib.sha256(serial.encode()).hexdigest()[:24] + '.lock'
    with (directory / filename).open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise TestError('Another test run owns this device.') from None
        yield


class Runner:
    def __init__(self, args: argparse.Namespace):
        self.args = args
        self.project = Path(args.project).resolve()
        self.env = os.environ.copy()
        self.repo = Path(__file__).resolve().parent.parent
        self.run_id = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ') + '-' + uuid.uuid4().hex[:8]
        self.report_dir = self.project / 'logs/tests' / self.run_id
        self.report_dir.mkdir(parents=True, mode=0o700)
        self.report_dir.chmod(0o700)
        self.started = time.monotonic()
        self.report = {'schemaVersion': 1, 'runId': self.run_id, 'application': PHOTOCRAFT_APP_NAME,
            'command': args.command, 'scope': args.scope, 'startedAt': datetime.now(timezone.utc).isoformat(),
            'status': 'running', 'phases': [], 'cases': [], 'device': None, 'artifacts': {},
            'normalOutputsModified': None}
        baseline = getattr(args, 'manual_baseline_seconds', None)
        if baseline is not None:
            self.report['manualBaseline'] = {'seconds': baseline, 'source': 'user-estimate',
                'coverageNote': 'The full automated suite includes a real one-minute autosave wait and cold restarts; the manual estimate is not a matched-coverage measurement.'}
        self.suite, self.case = parse_scope(args.scope)
        self.identity = None
        self.serial = None

    def save(self, name: str, text: str):
        (self.report_dir / name).write_text(text, encoding='utf-8')

    def command(self, argv: list[str], *, timeout: int = 30, cwd: Path | None = None,
                env: dict | None = None, output_path: Path | None = None) -> subprocess.CompletedProcess:
        try:
            if output_path:
                with output_path.open('w', encoding='utf-8') as stream:
                    result = subprocess.run(argv, cwd=cwd or self.project, env=env or self.env,
                        stdout=stream, stderr=subprocess.STDOUT, text=True, timeout=timeout)
                result.stdout = output_path.read_text(encoding='utf-8', errors='replace')
            else:
                result = subprocess.run(argv, cwd=cwd or self.project, env=env or self.env,
                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=timeout)
        except subprocess.TimeoutExpired as error:
            if error.stdout:
                text = error.stdout.decode(errors='replace') if isinstance(error.stdout, bytes) else error.stdout
                self.save('timeout-output.log', text)
            raise TestError(f'{Path(argv[0]).name} timed out after {timeout}s.') from None
        except OSError as error:
            raise TestError(f'{Path(argv[0]).name}: {error.strerror}.') from None
        return result

    def phase(self, name: str, argv: list[str], *, kind: str | None = None, timeout: int = 1800,
              env: dict | None = None):
        print(f'{name}: running', flush=True)
        start = time.monotonic()
        result = self.command(argv, timeout=timeout, env=env, output_path=self.report_dir / (name + '.log'))
        parsed = parse_host_output(result.stdout, kind) if kind else None
        success = result.returncode == 0 and (parsed is None or parsed['valid'] and parsed['failed'] == 0)
        phase = {'name': name, 'status': 'passed' if success else 'failed', 'exitCode': result.returncode,
                 'durationMs': round((time.monotonic() - start) * 1000), 'log': name + '.log'}
        if parsed:
            phase['testCount'] = parsed['total']
            if parsed.get('embeddedContractCounts'):
                phase['embeddedContractCounts'] = parsed['embeddedContractCounts']
            for case in parsed['cases']:
                case['suite'] = name + '/' + case['suite']
            self.report['cases'] += parsed['cases']
        else:
            self.report['cases'].append({'suite': 'Infrastructure', 'name': name, 'status': phase['status'],
                                        'durationMs': phase['durationMs']})
        self.report['phases'].append(phase)
        print(f'{name}: {phase["status"]} ({phase["durationMs"] / 1000:.1f}s)', flush=True)
        if not success:
            raise TestError(f'{name} failed or returned no test results; see {name}.log.')

    def check(self):
        required = ['cargo', 'rustc', 'python3']
        missing = [name for name in required if not shutil.which(name)]
        for name in ('CRAFT_NODE',):
            if not Path(self.env.get(name, '')).is_file():
                missing.append(name)
        if missing:
            raise TestError('Missing host tools: ' + ', '.join(missing))
        app = json5_load(self.project / 'harmonyos/AppScope/app.json5', self.env)['app']
        module = json5_load(self.project / 'harmonyos/entry/src/main/module.json5', self.env)['module']
        self.identity = {'bundle': app['bundleName'], 'module': module['name'], 'ability': module['mainElement']}
        for value in self.identity.values():
            if not isinstance(value, str) or not re.fullmatch(r'[A-Za-z][A-Za-z0-9_.]*', value):
                raise TestError('Invalid application identifier in manifest.')
        self.report['identity'] = self.identity
        manifest = self.project / 'apps' / PHOTOCRAFT_RUST_PACKAGE / 'Cargo.toml'
        match = re.search(r'^name\s*=\s*"([A-Za-z0-9_-]+)"', manifest.read_text(), re.M)
        if not match or match.group(1) != PHOTOCRAFT_RUST_PACKAGE:
            raise TestError('Expected the PhotoCraft Rust application package in Cargo manifest.')
        self.package = match.group(1)
        metadata = self.command(['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1',
                                 '--manifest-path', str(self.project / 'Cargo.toml')])
        if metadata.returncode:
            self.save('cargo-metadata-error.log', metadata.stdout)
            raise TestError('Cargo workspace metadata failed; see cargo-metadata-error.log.')
        # Cargo warnings can precede stdout when stderr is merged.
        metadata_json = next((line for line in metadata.stdout.splitlines() if line.lstrip().startswith('{')), None)
        if metadata_json is None:
            raise TestError('Cargo metadata did not return workspace packages.')
        self.host_packages = local_host_packages(json.loads(metadata_json), self.package)
        self.report['hostRustPackages'] = self.host_packages
        signing = (self.project / '.signing/build-profile.json5').is_file()
        tests = (self.project / 'harmonyos/entry/src/ohosTest/module.json5').is_file()
        self.report['capabilities'] = {'host': True, 'privateSigning': signing, 'deviceSuite': tests,
            'deviceStatus': 'available' if signing and tests else 'not-executed',
            'deviceReason': '' if signing and tests else ('No private signing profile.' if not signing else 'Device suites have not been ported.')}
        self.phase('runner-contract', ['python3', '-m', 'unittest', 'discover', '-s', str(self.repo / 'scripts/tests'), '-p', '*_test.py', '-v'])
        print(f'{PHOTOCRAFT_APP_NAME}: host available; device {self.report["capabilities"]["deviceStatus"]}', flush=True)
        if self.args.device:
            self.select_device()

    def host(self):
        env = self.env.copy()
        env['CARGO_TARGET_DIR'] = env.get('CRAFT_TEST_HOST_TARGET_DIR', str(self.project / 'target/host'))
        env['CARGO_INCREMENTAL'] = '0'
        config = self.report['hostConfiguration'] = {
            'targetDirectory': env['CARGO_TARGET_DIR'], 'incremental': False,
            'buildJobs': env.get('CARGO_BUILD_JOBS'),
            'profileOverrides': {key: value for key, value in env.items() if key.startswith('CARGO_PROFILE_')}}
        # all --scope filters the device phase. It still completes the full host
        # regressions, even when the device suite has no host filename alias.
        suite, case = ('', '') if self.args.command == 'all' else (self.suite, self.case)
        config['scope'] = None if self.args.command == 'all' else self.args.scope
        if env.get('CRAFT_FONTS_DIR'):
            manifest = Path(env['CRAFT_FONTS_DIR']) / 'fonts/manifest.txt'
            config['fontInput'] = {'directory': env['CRAFT_FONTS_DIR'], 'required': bool(env.get('CRAFT_FONTS_REQUIRED')),
                                   'manifestAvailable': manifest.is_file()}
            if manifest.is_file():
                config['fontInput']['manifestSha256'] = hashlib.sha256(manifest.read_bytes()).hexdigest()
        selected = []
        app = self.host_packages['app']
        platform = self.host_packages['platform']
        facade = self.host_packages['facade']
        defaults = [app, platform] + ([facade] if facade else [])
        aliases = {'Rust': defaults, 'Platform': [platform], PHOTOCRAFT_APP_NAME: [app],
                   self.package: [app], 'craft-ohos-platform': [platform]}
        if facade:
            aliases.update(Eframe=[facade], eframe=[facade])
        elif suite in ('Eframe', 'eframe'):
            raise TestError('This app has no local eframe facade to test.')
        if not suite or suite in aliases:
            packages = aliases.get(suite, defaults)
            argv = ['cargo', 'test', '--locked', '--manifest-path', str(self.project / 'Cargo.toml')]
            for package in packages:
                argv += ['-p', package]
            if case:
                argv += [case]
            self.phase('rust-host', argv, kind='rust', env=env)
            selected.append('rust')
        node_tests = node_test_files(self.project)
        if suite and suite != 'Node':
            node_tests = [path for path in node_tests if path.name.removesuffix('.test.cjs').removesuffix('.cjs') == suite]
        if node_tests:
            jobs = env.get('CRAFT_TEST_HOST_JOBS', '4')
            if not jobs.isdigit() or not 1 <= int(jobs) <= 64:
                raise TestError('CRAFT_TEST_HOST_JOBS must be an integer from 1 to 64.')
            argv = [env['CRAFT_NODE'], '--test', '--test-reporter=tap', f'--test-concurrency={jobs}']
            if case:
                argv += ['--test-name-pattern', re.escape(case)]
            argv += [str(path) for path in node_tests]
            self.phase('node-host', argv, kind='node', env=env, timeout=180)
            selected.append('node')
        if not suite or suite in ('Native', 'audio_contract', 'font_config_abi'):
            if (self.project / 'scripts/test-audio-contract.sh').is_file():
                if suite != 'font_config_abi':
                    self.phase('native-audio-contract', ['bash', str(self.project / 'scripts/test-audio-contract.sh')], timeout=180)
                    selected.append('audio')
                if suite != 'audio_contract':
                    self.phase('native-sdk-contract', ['bash', str(self.project / 'scripts/check-native.sh')], timeout=180)
                    selected.append('native')
        if not suite or suite in ('Native', 'native_ime', 'native-ime'):
            script = self.project / 'scripts/test-native-ime.sh'
            if script.is_file():
                self.phase('native-ime-contract', ['bash', str(script)], kind='tap', timeout=180)
                selected.append('native-ime')
        if not selected:
            raise TestError('Host scope selected no suites. Use Rust, Platform, PhotoCraft, Eframe, Node, or a Node test filename.')

    def hdc(self, argv: list[str], *, timeout: int = 15, allow_failure: bool = False) -> str:
        result = self.command([self.env['CRAFT_HDC'], '-t', self.serial, *remote_hdc_arguments(argv)], timeout=timeout)
        if result.returncode and not allow_failure:
            raise TestError(f'HDC command {argv[0]} failed: {result.stdout.strip()[:500]}')
        return result.stdout

    def stream_instrument(self, argv: list[str], *, timeout: int, run_id: str, phase: str, scope: str) -> str:
        relay = FileLayoutRelay(run_id, self.identity['bundle'], scope, self.report_dir, self.hdc)
        self._last_layout_relay = relay
        try:
            result = stream_process([self.env['CRAFT_HDC'], '-t', self.serial, *remote_hdc_arguments(argv)],
                cwd=self.project, env=self.env, timeout=timeout, output_path=self.report_dir / f'instrument-{phase}.txt',
                on_line=relay.line)
            if result.returncode:
                self.save('timeout-output.log', result.stdout)
                raise TestError(f'AA HDC process exited with status {result.returncode}.')
            return result.stdout
        except StreamError as error:
            self.save('timeout-output.log', error.stdout)
            raise TestError(str(error)) from None
        finally:
            self.report.setdefault('layoutRelayMetadata', []).append({'phase': phase, 'runId': run_id,
                'transport': 'hdc-debug-file', 'records': relay.records, 'defects': relay.defects,
                'nativeRoots': relay.native_roots})
            relay.close()

    def select_device(self):
        result = self.command([self.env['CRAFT_HDC'], 'list', 'targets'])
        self.save('targets.txt', result.stdout)
        if result.returncode or 'Connect server failed' in result.stdout:
            raise TestError('Cannot connect to HDC server; authorize host/device connectivity.')
        targets = [line.strip() for line in result.stdout.splitlines()
                   if re.fullmatch(r'[A-Za-z0-9_.:-]+', line.strip()) and line.strip() != '[Empty]']
        selected = self.args.device or self.env.get('HDC_DEVICE')
        if selected and selected not in targets:
            raise TestError('--device/HDC_DEVICE is not a connected device.')
        if not selected:
            if len(targets) != 1:
                raise TestError('Select --device SERIAL: expected one connected device, found ' + str(len(targets)) + '.')
            selected = targets[0]
        self.serial = selected
        api = self.hdc(['shell', 'param', 'get', 'const.ohos.apiversion']).strip()
        if not api.isdigit():
            raise TestError('Device API query did not return a numeric value.')
        self.report['device'] = {'serial': selected, 'api': int(api)}
        print(f'Device: {selected}, API {api}', flush=True)

    def active_pid(self) -> str:
        output = self.hdc(['shell', 'pidof', self.identity['bundle']], allow_failure=True).strip()
        if not output:
            return ''
        if not re.fullmatch(r'\d+(?:\s+\d+)*', output):
            raise TestError('Cannot reliably determine whether the ordinary app is running.')
        return output

    def collect_failure(self):
        if not self.serial:
            return
        self.collect_hilog('failure')
        # Capture only this app's layout. Screenshots are failure evidence only.
        for kind, suffix, command in [('layout', '.json', 'dumpLayout'), ('screen', '.png', 'screenCap')]:
            remote = f'/data/local/tmp/{self.run_id}-{kind}{suffix}'
            try:
                argv = ['shell', 'uitest', command, '-p', remote]
                if kind == 'layout':
                    argv += ['-b', self.identity['bundle']]
                text = self.hdc(argv, timeout=20)
                self.save(kind + '-capture.txt', text)
                self.hdc(['file', 'recv', remote, str(self.report_dir / (kind + suffix))], timeout=30)
                self.hdc(['shell', 'rm', '-f', remote], timeout=10)
            except TestError as error:
                self.save(kind + '-capture-error.txt', str(error))

    def collect_hilog(self, outcome: str):
        pids = set()
        runtime = self.report.get('runtimeEvidence', [])
        if runtime:
            pids.update(item['pid'] for item in runtime[-1]['records'])
        try:
            current = self.active_pid()
            if re.fullmatch(r'\d+', current):
                pids.add(int(current))
        except TestError:
            pass
        if not pids:
            self.save('hilog-unavailable.txt', 'No current or validated startup app PID; no unfiltered log query was issued.\n')
        for pid in sorted(pids):
            try:
                self.save(f'hilog-{outcome}-{pid}.txt', self.hdc(
                    ['shell', 'hilog', '-x', '-P', str(pid), '-T', PHOTOCRAFT_APP_NAME], timeout=15))
                self.report['device']['pid'] = pid
            except TestError as error:
                self.save(f'hilog-{outcome}-{pid}-error.txt', str(error))

    def collect_case_failure_evidence(self, phase: str, evidence: dict):
        transfers = []
        for item in evidence['records']:
            for field, suffix in (('screenPath', 'png'), ('layoutPath', 'json')):
                if field not in item:
                    continue
                remote = item[field]
                destination = self.report_dir / f'{phase}-failure-{item["timestamp"]}.{suffix}'
                transfer = {'suite': item['suite'], 'label': item['label'], 'remotePath': remote, 'path': str(destination)}
                try:
                    self.hdc(['file', 'recv', remote, str(destination)], timeout=30)
                    if not destination.is_file() or not destination.stat().st_size:
                        raise TestError('Failure evidence receive did not create a non-empty local file.')
                    self.hdc(['shell', 'rm', '-f', remote], timeout=10)
                    transfer['status'] = 'saved'
                except TestError as error:
                    transfer.update(status='unavailable', error=str(error))
                transfers.append(transfer)
        return transfers

    @contextmanager
    def normal_output_guard(self):
        paths = ['harmonyos/build-profile.json5', 'harmonyos/entry/build-profile.json5',
                 'harmonyos/entry/libs/arm64-v8a/libphotocraft_ohos.a',
                 'harmonyos/entry/build/default/outputs/default/entry-default-signed.hap']
        cached = Path(self.env.get('CRAFT_TEST_DEVICE_TARGET_DIR', str(self.project / 'target')))
        files = [self.project / path for path in paths]
        files += [cached / self.env['CRAFT_RUST_TARGET'] / ('release/libphotocraft_ohos' + suffix)
                  for suffix in ('.a', '.rlib', '.d')]
        def fingerprints():
            result = {}
            for path in files:
                if path.is_file():
                    with path.open('rb') as stream:
                        result[str(path)] = hashlib.file_digest(stream, 'sha256').hexdigest()
                else:
                    result[str(path)] = None
            return result
        baseline = fingerprints()
        self.report['normalOutputFingerprintsBefore'] = baseline
        try:
            yield
        finally:
            final = fingerprints()
            self.report['normalOutputFingerprintsAfter'] = final
            modified = final != baseline
            self.report['normalOutputsModified'] = modified
            if modified:
                raise TestError('Ordinary build output changed during isolated test build; inspect fingerprint report.')

    def device(self):
        if not self.report['capabilities']['privateSigning'] or not self.report['capabilities']['deviceSuite']:
            self.report['deviceExecution'] = 'not-executed'
            raise TestError('Device tests not executed: ' + self.report['capabilities']['deviceReason'])
        if not self.serial:
            self.select_device()
        with device_lock(self.repo, self.serial), self.normal_output_guard():
            pid = self.active_pid()
            if pid:
                raise TestError(f'Ordinary {PHOTOCRAFT_APP_NAME} is still running (PID {pid}). Save and exit it before device tests.')
            build_started = time.monotonic()
            if self.args.reuse_build:
                self.report['artifacts'] = reuse_artifacts(Path(self.args.reuse_build).resolve(), self.project, self.identity['bundle'])
                self.report['reusedSignedBuild'] = str(Path(self.args.reuse_build).resolve())
                self.report['buildDurationMs'] = 0
                self.report['buildValidationDurationMs'] = round((time.monotonic() - build_started) * 1000)
            else:
                self.report['artifacts'] = build(self.project, self.report_dir, self.env, self.identity['bundle'])
                self.report['buildDurationMs'] = round((time.monotonic() - build_started) * 1000)
            # Building may take minutes. Check again immediately before replacing the installed app.
            if self.active_pid():
                raise TestError('The ordinary app was opened during the build. Save and exit it before retrying.')
            original_hap = self.project / 'harmonyos/entry/build/default/outputs/default/entry-default-signed.hap'
            if not original_hap.is_file():
                raise TestError('Device tests not executed: an ordinary signed HAP is required for restoration.')
            snapshot_hap = self.report_dir / 'ordinary-entry-signed.hap'
            clone_file(original_hap, snapshot_hap)
            self.report['ordinaryPackage'] = {'path': str(original_hap), 'snapshot': str(snapshot_hap),
                'sha256': hashlib.sha256(snapshot_hap.read_bytes()).hexdigest(), 'restored': False}
            native_library = PHOTOCRAFT_NATIVE_LIBRARY
            interfaces = self.report['hapInterfaceEvidence'] = {}
            interfaces['ordinary'] = inspect_hap_interfaces(snapshot_hap, native_library)
            require_hap_interface_mode(interfaces['ordinary'], device_tests=False)
            interfaces['deviceTests'] = inspect_hap_interfaces(Path(self.report['artifacts']['default']['path']), native_library)
            require_hap_interface_mode(interfaces['deviceTests'], device_tests=True)
            # The separate entry_test HAP contains the ArkTS runner and no native
            # app library. Test NAPI exports must be in the staged main HAP.
            self.report['testModuleNativeInterfaceGate'] = 'uses validated device-test main HAP'
            main_installed = False
            test_installed = False
            test_started = False
            try:
                for target in ('default', 'ohosTest'):
                    hap = self.report['artifacts'][target]['path']
                    installed = self.hdc(['install', hap], timeout=90)
                    self.save(f'install-{target}.txt', installed)
                    if not re.search(r'successfully|success', installed, re.I):
                        raise TestError(f'Installing {target} did not report success.')
                    if target == 'default':
                        main_installed = True
                    else:
                        test_installed = True
                test_started = True
                self.instrument()
            finally:
                if main_installed:
                    restoration_errors = []
                    try:
                        if test_started and self.active_pid():
                            stopped = self.hdc(['shell', 'aa', 'force-stop', self.identity['bundle']])
                            self.save('owned-test-session-stop.txt', stopped)
                            for attempt in range(20):
                                if not self.active_pid():
                                    break
                                time.sleep(0.1)
                        if self.active_pid():
                            raise TestError('Test process did not exit; ordinary HAP restoration was not attempted.')
                        installed = self.hdc(['install', str(snapshot_hap)], timeout=90)
                        self.save('restore-ordinary-package.txt', installed)
                        if not re.search(r'successfully|success', installed, re.I):
                            raise TestError('Ordinary signed HAP installation did not report success.')
                        self.report['ordinaryPackage']['restored'] = True
                    except TestError as error:
                        restoration_errors.append(str(error))
                    if test_installed:
                        try:
                            removed = self.hdc(['shell', 'bm', 'uninstall', '-n', self.identity['bundle'], '-m', 'entry_test', '-k'], timeout=60)
                            self.save('remove-test-module.txt', removed)
                            if not re.search(r'successfully|success', removed, re.I):
                                raise TestError('Removing only entry_test did not report success.')
                            self.report['testModuleRemoved'] = True
                        except TestError as error:
                            self.report['testModuleRemoved'] = False
                            restoration_errors.append(str(error))
                    if restoration_errors:
                        raise TestError('Package restoration incomplete: ' + '; '.join(restoration_errors))

    def instrument(self):
        before = parse_testmode(self.hdc(['shell', 'param', 'get', 'persist.ace.testmode.enabled']))
        original = before['value']
        self.report['device']['testmodeBefore'] = original
        self.report['device']['testmodeBeforeRaw'] = before['raw']
        self.report['device']['testmodeReadable'] = before['readable']
        self.report['device']['testmodeState'] = 'readable' if before['readable'] else 'unavailable'
        self.report['device']['testmodeAfter'] = original
        self.report['device']['testmodeRestoration'] = 'unchanged (parameter unavailable; no writes)' if not before['readable'] else 'unchanged (already enabled)'
        changed = before['readable'] and not before['enabled']
        try:
            if changed:
                self.hdc(['shell', 'param', 'set', 'persist.ace.testmode.enabled', '1'])
                if self.hdc(['shell', 'param', 'get', 'persist.ace.testmode.enabled']).strip() != '1':
                    raise TestError('Enabling UiTest testmode failed.')
            timeout = self.args.case_timeout
            if self.args.scope:
                self.instrument_phase('selected', self.args.scope, timeout)
                self.report['deviceAcceptance'] = 'stage-only' if self.args.scope.endswith('#seed_autosave') else 'partial'
            else:
                self.instrument_phase('core-files', 'PhotoCraftCore,PhotoCraftFiles', timeout)
                self.stop_owned_test_session('core-to-recovery')
                recovery_run = self.run_id + '_recovery'
                self.instrument_phase('recovery-seed', 'PhotoCraftRecovery#seed_autosave', max(timeout, 180000), recovery_run)
                self.stop_owned_test_session('recovery-restart')
                self.instrument_phase('recovery-verify', 'PhotoCraftRecovery#verify_autosave', timeout, recovery_run)
                self.stop_owned_test_session('recovery-to-close')
                close_run = self.run_id + '_close'
                # Both cases share one beforeAll: cancellation leaves its owned
                # dirty document for the final real save-and-close case.
                self.instrument_phase('close', 'PhotoCraftClose', timeout, close_run)
                inventory = validate_device_case_inventory(','.join(PHOTOCRAFT_DEVICE_CASES),
                    [case for result in self.report['instrumentResults'] for case in result['cases']])
                self.report['fullDeviceInventory'] = inventory
                if not inventory['passed']:
                    raise TestError('Full device acceptance inventory: ' + '; '.join(inventory['defects']))
                self.report['deviceAcceptance'] = 'full'
            self.aggregate_instrument_result()
            self.collect_hilog('success')
        except (TestError, BuildError):
            self.collect_failure()
            raise
        finally:
            if changed:
                # hdc joins shell arguments; quote the empty value explicitly.
                self.hdc(['shell', 'param', 'set', 'persist.ace.testmode.enabled', original])
                restored = self.hdc(['shell', 'param', 'get', 'persist.ace.testmode.enabled']).strip()
                self.report['device']['testmodeAfter'] = restored
                self.report['device']['testmodeRestoration'] = 'exact-value'
                if restored != original:
                    raise TestError('Failed to restore the original UiTest testmode value.')

    def cleanup_query_files(self, metadata: dict) -> list[dict]:
        cleanup = []
        for item in metadata['records']:
            path = item['path']
            result = {'path': path, 'bundleName': item['bundleName']}
            try:
                output = self.hdc(['shell', 'rm', '-f', path], timeout=10)
                if output.strip():
                    raise TestError('Query removal returned unexpected output: ' + output.strip())
                verification = self.hdc(['shell', 'sh', '-c',
                    'if [ -e "$1" ]; then printf PRESENT; else printf REMOVED; fi',
                    'photocraft-query-cleanup', path], timeout=10).strip()
                if verification != 'REMOVED':
                    raise TestError('Query file absence was not confirmed: ' + verification)
                result['status'] = 'removed'
            except TestError as error:
                result.update(status='failed', error=str(error))
            cleanup.append(result)
        return cleanup

    def instrument_phase(self, name: str, scope: str, case_timeout: int, run_id: str | None = None):
        print(f'aa instrument {name}: running', flush=True)
        start = time.monotonic()
        actual_run_id = run_id or self.run_id
        transport_error = None
        try:
            output = self.stream_instrument(aa_arguments(self.identity['bundle'], actual_run_id, scope, case_timeout),
                timeout=660, run_id=actual_run_id, phase=name, scope=scope)
        except TestError as error:
            transport_error = str(error)
            partial_output = self.report_dir / 'timeout-output.log'
            output = partial_output.read_text(encoding='utf-8', errors='replace') if partial_output.is_file() else ''
            self.save(f'instrument-{name}-transport-error.txt', transport_error + '\n')
        execution_finished = time.monotonic()
        filename = f'instrument-{name}.txt'
        self.save(filename, output)
        parsed = parse_instrument_output(output)
        relay_metadata = next((item for item in reversed(self.report.get('layoutRelayMetadata', []))
                               if item['phase'] == name and item['runId'] == actual_run_id), None)
        if relay_metadata:
            parsed['layoutRelayMetadata'] = relay_metadata
            parsed['passed'] = parsed['passed'] and not relay_metadata['defects']
            parsed['defects'] += relay_metadata['defects']
        if transport_error:
            parsed['passed'] = False
            parsed['defects'].append('Instrument transport failed: ' + transport_error)
        query_files = parse_query_files(output, actual_run_id, self.identity['bundle'])
        query_files['cleanup'] = self.cleanup_query_files(query_files)
        query_files['cleanupFailed'] = any(item['status'] != 'removed' for item in query_files['cleanup'])
        self.report.setdefault('queryFilesMetadata', []).append({'phase': name, 'runId': actual_run_id, **query_files})
        parsed['queryFilesMetadata'] = query_files
        if query_files['defects'] or query_files['cleanupFailed']:
            parsed['passed'] = False
            parsed['defects'] += query_files['defects']
            parsed['defects'] += ['Query cleanup failed: ' + item['error'] for item in query_files['cleanup'] if item['status'] != 'removed']
        failure_evidence = parse_failure_evidence(output, actual_run_id, scope)
        failure_evidence['transfers'] = self.collect_case_failure_evidence(name, failure_evidence)
        self.report.setdefault('deviceFailureEvidence', []).append({'phase': name, 'runId': actual_run_id, **failure_evidence})
        parsed['failureEvidence'] = failure_evidence
        if failure_evidence['records'] or failure_evidence['defects']:
            parsed['passed'] = False
            parsed['defects'] += failure_evidence['defects'] or ['A device failure was captured during the case.']
        ui_driver = parse_ui_driver_metadata(output, actual_run_id, scope)
        self.report.setdefault('uiDriverMetadata', []).append({'phase': name, 'runId': actual_run_id, **ui_driver})
        backends = sorted({item['backend'] for phase in self.report['uiDriverMetadata'] for item in phase['records']})
        self.report['driverBackend'] = backends[0] if len(backends) == 1 else backends or None
        parsed['uiDriverMetadata'] = ui_driver
        parsed['passed'] = parsed['passed'] and not ui_driver['defects']
        parsed['defects'] += ui_driver['defects']
        runtime = parse_runtime_evidence(output, actual_run_id, scope)
        self.report.setdefault('runtimeEvidence', []).append({'phase': name, 'runId': actual_run_id, **runtime})
        parsed['runtime'] = runtime
        parsed['passed'] = parsed['passed'] and not runtime['defects']
        parsed['defects'] += runtime['defects']
        inventory = parse_cache_inventory(output, actual_run_id, scope)
        self.report.setdefault('cacheInventory', []).append({'phase': name, 'runId': actual_run_id, **inventory})
        parsed['cacheInventory'] = inventory
        case_inventory = validate_device_case_inventory(scope, parsed['cases'])
        parsed['caseInventory'] = case_inventory
        parsed['passed'] = parsed['passed'] and case_inventory['passed']
        parsed['defects'] += case_inventory['defects']
        isolation = parse_isolation_evidence(output, actual_run_id, scope)
        parsed['isolation'] = isolation
        parsed['passed'] = parsed['passed'] and isolation['passed']
        parsed['defects'] += isolation['defects']
        self.report.setdefault('ordinaryStorageEvidence', []).append({'phase': name, 'runId': actual_run_id, **isolation})
        root_metadata = parse_storage_roots_metadata(output, actual_run_id, scope, isolation)
        self.report.setdefault('storageRootsMetadata', []).append({'phase': name, 'runId': actual_run_id, **root_metadata})
        parsed['storageRootsMetadata'] = root_metadata
        parsed['passed'] = parsed['passed'] and not root_metadata['defects']
        parsed['defects'] += root_metadata['defects']
        parsed['phase'] = name
        parsed['scope'] = scope
        parsed['caseTimeoutMs'] = case_timeout
        parsed['runId'] = actual_run_id
        if not parsed['passed'] and relay_metadata:
            relay_metadata['lastQuery'] = self._last_layout_relay.retain_failure_query(name)
        if relay_metadata:
            self._last_layout_relay.last_layout = None
        self.report.setdefault('instrumentResults', []).append(parsed)
        self.report['cases'] += parsed['cases']
        phase_finished = time.monotonic()
        self.report['phases'].append({'name': f'aa-instrument-{name}', 'scope': scope,
            'status': 'passed' if parsed['passed'] else 'failed', 'durationMs': round((phase_finished - start) * 1000),
            'executionDurationMs': round((execution_finished - start) * 1000),
            'postprocessDurationMs': round((phase_finished - execution_finished) * 1000), 'log': filename})
        self.report['deviceExecution'] = 'executed'
        self.aggregate_instrument_result()
        if not parsed['passed']:
            raise TestError(f'{name}: ' + '; '.join(parsed['defects']))
        print(f'aa instrument {name}: passed ({parsed["summary"]["passed"]} cases)', flush=True)

    def aggregate_instrument_result(self):
        results = self.report['instrumentResults']
        self.report['instrumentResult'] = {'passed': all(result['passed'] for result in results),
            'summary': {key: sum((result['summary'] or {}).get(key, 0) for result in results)
                        for key in ('total', 'failed', 'errors', 'passed', 'skipped')},
            'cases': [case for result in results for case in result['cases']],
            'defects': [defect for result in results for defect in result['defects']]}

    def stop_owned_test_session(self, name: str):
        pid = self.active_pid()
        if pid:
            stopped = self.hdc(['shell', 'aa', 'force-stop', self.identity['bundle']])
            self.save(f'owned-test-session-stop-{name}.txt', stopped)
            for attempt in range(30):
                if not self.active_pid():
                    break
                time.sleep(0.1)
            if self.active_pid():
                raise TestError('Owned test process did not stop for the recovery phase.')
        self.report.setdefault('ownedRestarts', []).append({'phase': name, 'pidBefore': pid, 'stopped': True})

    def finish(self, status: str, error: str = ''):
        self.report['status'] = status
        self.report['finishedAt'] = datetime.now(timezone.utc).isoformat()
        self.report['durationMs'] = round((time.monotonic() - self.started) * 1000)
        aa_phases = [phase for phase in self.report['phases'] if phase['name'].startswith('aa-instrument-')]
        aa_ms = sum(phase.get('executionDurationMs', phase['durationMs']) for phase in aa_phases)
        self.report['timings'] = {'buildMs': self.report.get('buildDurationMs'),
            'buildValidationMs': self.report.get('buildValidationDurationMs', 0), 'aaExecutionMs': aa_ms,
            'devicePostprocessMs': sum(phase.get('postprocessDurationMs', 0) for phase in aa_phases),
            'wholeRunMs': self.report['durationMs'], 'aaExecutionStatus': 'not-executed' if not aa_phases else status}
        baseline = self.report.get('manualBaseline')
        if baseline:
            seconds = baseline['seconds']
            self.report['comparison'] = {'baselineSource': 'user-estimate', 'manualEstimateSeconds': seconds,
                'executionStatus': 'not-executed' if not aa_phases else status,
                'scope': self.args.scope,
                'aaExecutionSeconds': round(aa_ms / 1000, 3) if aa_phases else None,
                'aaDifferenceFromEstimateSeconds': round(aa_ms / 1000 - seconds, 3) if aa_phases else None,
                'wholeRunSeconds': round(self.report['durationMs'] / 1000, 3),
                'wholeRunDifferenceFromEstimateSeconds': round(self.report['durationMs'] / 1000 - seconds, 3) if aa_phases else None,
                'coverageNote': baseline['coverageNote']}
        if error:
            self.report['error'] = error
            self.report['cases'].append({'suite': 'Runner', 'name': 'execution', 'status': 'error', 'durationMs': 0, 'message': error})
        self.save('report.json', json.dumps(self.report, ensure_ascii=False, indent=2) + '\n')
        cases = self.report['cases']
        suite = ET.Element('testsuite', name=PHOTOCRAFT_APP_NAME, tests=str(len(cases)),
            failures=str(sum(c['status'] == 'failed' for c in cases)), errors=str(sum(c['status'] == 'error' for c in cases)),
            skipped=str(sum(c['status'] == 'skipped' for c in cases)), time=str(self.report['durationMs'] / 1000))
        for case in cases:
            item = ET.SubElement(suite, 'testcase', classname=case['suite'], name=case['name'], time=str(case.get('durationMs', 0) / 1000))
            if case['status'] in ('failed', 'error', 'skipped'):
                child = ET.SubElement(item, {'failed': 'failure', 'error': 'error', 'skipped': 'skipped'}[case['status']])
                child.text = case.get('message', '')
        ET.ElementTree(suite).write(self.report_dir / 'junit.xml', encoding='utf-8', xml_declaration=True)
        print(f'Report: {self.report_dir}', flush=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', required=True, help=argparse.SUPPRESS)
    parser.add_argument('command', choices=('check', 'host', 'device', 'all'), nargs='?', default='check')
    parser.add_argument('--scope', help='Device Suite#case; host Rust/Platform/PhotoCraft/Eframe/Node or Node filename#case')
    parser.add_argument('--device', help='HDC serial; required when multiple devices are connected')
    parser.add_argument('--case-timeout', type=int, default=60000, help='Hypium case timeout in milliseconds (default: 60000)')
    parser.add_argument('--reuse-build', help='Reuse validated signed test HAPs from a build-artifacts.json/report.json or its run directory')
    parser.add_argument('--repeat', type=int, default=1, help='Repeat device tests with fresh isolated run ids, reusing one signed build (default: 1)')
    parser.add_argument('--manual-baseline-seconds', type=float, help='Optional user estimate for the manual workflow; report build, aa, and whole-run differences separately')
    args = parser.parse_args(argv)
    if not 1 <= args.case_timeout <= 600000:
        parser.error('--case-timeout must be between 1 and 600000 milliseconds.')
    if not 1 <= args.repeat <= 100 or args.repeat > 1 and args.command not in ('device', 'all'):
        parser.error('--repeat must be between 1 and 100 and is supported by device/all.')
    if args.manual_baseline_seconds is not None and not 0 < args.manual_baseline_seconds < 86400:
        parser.error('--manual-baseline-seconds must be positive and below 86400.')
    try:
        parse_scope(args.scope)
    except argparse.ArgumentTypeError as error:
        parser.error(str(error))
    if args.command == 'all' and args.scope and any(selector.split('#', 1)[0] not in PHOTOCRAFT_DEVICE_SUITES for selector in args.scope.split(',')):
        parser.error('all --scope selects device suites (PhotoCraftCore/Files/Recovery/Close); use host --scope for Rust/Platform/Node or host cases.')
    if args.command in ('device', 'all') and args.scope:
        try:
            requested_device_cases(args.scope)
        except TestError as error:
            parser.error(str(error))
    runner = Runner(args)
    campaign_dir = runner.report_dir
    campaign = {'requestedRuns': args.repeat, 'runs': [], 'allPassed': False, 'fullDeviceAcceptance': False}
    def record_campaign():
        campaign['runs'].append({'runId': runner.run_id, 'status': runner.report['status'],
            'deviceAcceptance': runner.report.get('deviceAcceptance'), 'report': str(runner.report_dir / 'report.json'),
            'durationMs': runner.report['durationMs'], 'timings': runner.report.get('timings')})
        campaign['allPassed'] = len(campaign['runs']) == args.repeat and all(run['status'] == 'passed' for run in campaign['runs'])
        campaign['fullDeviceAcceptance'] = args.repeat >= 3 and campaign['allPassed'] and all(
            run['deviceAcceptance'] == 'full' for run in campaign['runs'])
        if args.manual_baseline_seconds is not None:
            baseline = args.manual_baseline_seconds
            aa_seconds = sum((run['timings'] or {}).get('aaExecutionMs', 0) for run in campaign['runs']) / 1000
            whole_seconds = sum(run['durationMs'] for run in campaign['runs']) / 1000
            executed_runs = sum((run['timings'] or {}).get('aaExecutionStatus') not in (None, 'not-executed') for run in campaign['runs'])
            campaign['manualBaseline'] = {'secondsPerRun': baseline, 'source': 'user-estimate',
                'coverageNote': 'The automated full suite also waits for real one-minute autosave and cold restarts; coverage differs from the manual estimate.'}
            campaign['comparison'] = {'executedRuns': executed_runs,
                'totalAAExecutionSeconds': round(aa_seconds, 3) if executed_runs else None,
                'totalWholeRunSeconds': round(whole_seconds, 3),
                'aaDifferenceFromManualEstimateSeconds': round(aa_seconds - baseline * executed_runs, 3) if executed_runs else None,
                'wholeRunDifferenceFromManualEstimateSeconds': round(whole_seconds - baseline * len(campaign['runs']), 3) if executed_runs else None}
        if args.repeat > 1:
            (campaign_dir / 'campaign.json').write_text(json.dumps(campaign, indent=2) + '\n')
    try:
        runner.check()
        if args.command in ('host', 'all'):
            runner.host()
        if args.command in ('device', 'all'):
            for index in range(args.repeat):
                runner.report['repetitionIndex'] = index + 1
                runner.report['requestedRepetitions'] = args.repeat
                runner.device()
                runner.finish('passed')
                record_campaign()
                if index + 1 < args.repeat:
                    repeated = argparse.Namespace(**vars(args))
                    repeated.command = 'device'
                    repeated.reuse_build = str(campaign_dir / 'report.json')
                    runner = Runner(repeated)
                    runner.check()
            if args.repeat > 1:
                print(f'Campaign: {campaign_dir / "campaign.json"}', flush=True)
        else:
            runner.finish('passed')
        return 0
    except (TestError, BuildError, ValueError, KeyError, OSError) as error:
        print(str(error), file=sys.stderr)
        runner.finish('failed', str(error))
        record_campaign()
        return 1
    except KeyboardInterrupt:
        runner.finish('failed', 'Interrupted; partial results are not accepted.')
        record_campaign()
        return 130


if __name__ == '__main__':
    sys.exit(main())
