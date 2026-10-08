"""Authenticated, bounded UiTest layout replies via the official debug file channel."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import tempfile
import time

MAX_LAYOUT_BYTES = 4 * 1024 * 1024
MAX_REPLY_BYTES = MAX_LAYOUT_BYTES + 65536
REQUEST_FIELDS = {'runId', 'bundleName', 'path', 'ticket', 'nonce', 'replyPath', 'readyPath'}


def strict_json(raw: bytes):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('Duplicate JSON field.')
            result[key] = value
        return result
    def constant(value):
        raise ValueError('Nonfinite JSON value.')
    return json.loads(raw.decode('utf-8'), object_pairs_hook=unique, parse_constant=constant)


def safe_storage_path(path: str) -> bool:
    return isinstance(path, str) and bool(re.fullmatch(r'/[A-Za-z0-9_./-]+', path)) and '//' not in path and all(
        part not in ('.', '..') for part in path.split('/'))


def validate_request(item: dict, run_id: str, bundle: str, cache: str | None) -> dict:
    if not isinstance(item, dict) or set(item) != REQUEST_FIELDS or item.get('runId') != run_id:
        raise ValueError('Layout request fields/run identity are invalid.')
    if item.get('bundleName') not in (bundle, 'com.huawei.hmos.filemanager'):
        raise ValueError('Layout request bundle is outside the tested app and picker.')
    if not isinstance(item.get('ticket'), str) or not re.fullmatch(r'layout-[1-9][0-9]{0,19}', item['ticket']):
        raise ValueError('Layout request ticket is invalid.')
    if not isinstance(item.get('nonce'), str) or not re.fullmatch(r'[a-f0-9]{64}', item['nonce']):
        raise ValueError('Layout request nonce is invalid.')
    pattern = r'/data/local/tmp/PhotoCraftTest-' + re.escape(run_id) + r'-[0-9]{1,20}-[0-9]{1,20}-query\.json'
    if not isinstance(item.get('path'), str) or not re.fullmatch(pattern, item['path']):
        raise ValueError('Layout source path is not owned by this run.')
    if not cache:
        raise ValueError('Layout reply has no previously verified native test cache root.')
    prefix = re.escape(cache) + r'/LayoutRelay-[0-9]{1,20}-[1-9][0-9]{0,19}'
    if not isinstance(item.get('replyPath'), str) or not re.fullmatch(prefix + r'/reply\.part', item['replyPath']):
        raise ValueError('Layout reply path is outside the verified native test cache.')
    if item.get('readyPath') != item['replyPath'].rsplit('/', 1)[0] + '/ready.json':
        raise ValueError('Layout ready path does not belong to the same reply directory.')
    return item


def validate_layout(raw: bytes, bundle: str) -> dict:
    if not 0 < len(raw) <= MAX_LAYOUT_BYTES:
        raise ValueError('Layout payload is empty or exceeds 4 MiB.')
    tree = strict_json(raw)
    pending = [(tree, 0)]
    count = 0
    scoped_window = False
    while pending:
        node, depth = pending.pop()
        count += 1
        if count > 100000 or depth > 128 or not isinstance(node, dict) or not isinstance(node.get('attributes'), dict):
            raise ValueError('Layout is not a bounded complete UiTest tree.')
        attrs = node['attributes']
        if any(not isinstance(key, str) or not isinstance(value, str) for key, value in attrs.items()):
            raise ValueError('Layout attributes must be actual UiTest string attributes.')
        if attrs.get('bundleName') == bundle and attrs.get('type') == 'root':
            scoped_window = True
        children = node.get('children', [])
        if not isinstance(children, list):
            raise ValueError('Layout children are not a node array.')
        pending.extend((child, depth + 1) for child in children)
    empty_root = set(tree) == {'attributes', 'children'} and tree['children'] == [] and \
        tree['attributes'].get('bounds', '') in ('', '[0,0][0,0]') and all(
            value == '' for key, value in tree['attributes'].items() if key != 'bounds')
    if not scoped_window and not empty_root:
        raise ValueError('Layout has no actual window for the requested bundle.')
    return tree


def require_transfer(output: str, size: int):
    finishes = re.findall(r'FileTransfer finish, Size:(\d+), File count\s*=\s*(\d+)\b', output)
    if len(finishes) != 1 or tuple(map(int, finishes[0])) != (size, 1) or re.search(r'\[Fail\]|Permission denied', output, re.I):
        raise ValueError('Debug file transfer did not confirm exactly one file with the expected byte size.')


class FileLayoutRelay:
    def __init__(self, run_id: str, bundle: str, scope: str, report_dir: Path, hdc):
        self.run_id, self.bundle, self.scope = run_id, bundle, scope
        self.report_dir, self.hdc = report_dir, hdc
        self.cache = None
        self.entry_cache = None
        self.entry_files = None
        self.records = []
        self.defects = []
        self.native_roots = []
        self.tickets = set()
        self.paths = set()
        self.reply_paths = set()
        self.last_layout = None
        self.temp = tempfile.TemporaryDirectory(prefix='.layout-relay-', dir=report_dir)

    def line(self, line: str):
        for marker in ('PHOTOCRAFT_STORAGE_ROOTS', 'PHOTOCRAFT_NATIVE_ROOTS', 'PHOTOCRAFT_LAYOUT_REQUEST', 'PHOTOCRAFT_LAYOUT_CLEANUP_ERROR'):
            if marker not in line:
                continue
            try:
                text = line.split(marker, 1)[1].strip()
                if len(text.encode('utf-8')) > 4096:
                    raise ValueError('Layout relay metadata exceeds its size bound.')
                item = strict_json(text.encode('utf-8'))
                if marker == 'PHOTOCRAFT_STORAGE_ROOTS':
                    suites = {selector.split('#')[0] for selector in self.scope.split(',')}
                    if not isinstance(item, dict) or item.get('runId') != self.run_id or item.get('suite') not in suites:
                        raise ValueError('Relay storage roots use a different run/suite.')
                    cache, files = item.get('entryCacheDir'), item.get('entryFilesDir')
                    if not safe_storage_path(cache) or not cache.endswith('/haps/entry/cache') or not safe_storage_path(files) or not files.endswith('/haps/entry/files'):
                        raise ValueError('Relay entry storage roots are not genuine module paths.')
                    if self.entry_cache and (cache != self.entry_cache or files != self.entry_files):
                        raise ValueError('Relay entry storage roots changed during the phase.')
                    self.entry_cache, self.entry_files = cache, files
                elif marker == 'PHOTOCRAFT_NATIVE_ROOTS':
                    if not isinstance(item, dict) or item.get('runId') != self.run_id or type(item.get('pid')) is not int or item['pid'] <= 0:
                        raise ValueError('Relay native roots use an invalid run/PID.')
                    expected_cache = f'{self.entry_cache}/PhotoCraftTestRuns/{self.run_id}/cache'
                    expected_files = f'{self.entry_files}/PhotoCraftTestRuns/{self.run_id}/files'
                    if not self.entry_cache or item.get('abilityCacheDir') != self.entry_cache or item.get('abilityFilesDir') != self.entry_files or \
                            item.get('nativeCache') != expected_cache or item.get('expectedCache') != expected_cache or \
                            item.get('nativeFiles') != expected_files or item.get('expectedFiles') != expected_files:
                        raise ValueError('Layout relay cache differs from the genuine ability/native root evidence.')
                    self.cache = expected_cache
                    self.native_roots.append(item)
                elif marker == 'PHOTOCRAFT_LAYOUT_REQUEST':
                    self.handle(validate_request(item, self.run_id, self.bundle, self.cache))
                else:
                    if not isinstance(item, dict) or item.get('runId') != self.run_id or not isinstance(item.get('ticket'), str) or \
                            not re.fullmatch(r'layout-[1-9][0-9]{0,19}', item['ticket']) or not isinstance(item.get('message'), str):
                        raise ValueError('Layout cleanup error metadata has invalid run/ticket/message fields.')
                    self.defects.append('App layout relay cleanup failed: ' + item['message'][:1000])
            except (ValueError, TypeError, KeyError, UnicodeError) as error:
                self.defects.append(str(error))

    def handle(self, item: dict):
        key = (item['nonce'], item['ticket'])
        if key in self.tickets or item['path'] in self.paths or item['replyPath'] in self.reply_paths:
            raise ValueError('Layout request reuses a ticket/source/reply path.')
        self.tickets.add(key)
        self.paths.add(item['path'])
        self.reply_paths.add(item['replyPath'])
        record = {key: value for key, value in item.items() if key != 'nonce'}
        record['nonceSha256'] = hashlib.sha256(item['nonce'].encode()).hexdigest()
        started = time.monotonic()
        deadline = started + 10
        def remaining():
            seconds = deadline - time.monotonic()
            if seconds <= 0:
                raise TimeoutError('Layout relay exceeded its 10-second deadline.')
            return seconds
        local = Path(self.temp.name)
        identity = {key: item[key] for key in ('runId', 'nonce', 'ticket', 'path')}
        try:
            source = local / 'query.json'
            try:
                output = self.hdc(['file', 'recv', item['path'], str(source)], timeout=remaining())
                if not source.is_file() or source.is_symlink() or source.stat().st_size > MAX_LAYOUT_BYTES:
                    raise ValueError('HDC did not receive a bounded regular layout file.')
                raw = source.read_bytes()
                require_transfer(output, len(raw))
                layout = validate_layout(raw, item['bundleName'])
                self.last_layout = raw
                record.update(sourceBytes=len(raw), sourceSha256=hashlib.sha256(raw).hexdigest())
                body = {**identity, 'ok': True, 'layout': layout}
            except Exception as error:
                record['error'] = str(error)[:1000]
                self.defects.append('Layout relay source failed: ' + record['error'])
                body = {**identity, 'ok': False, 'error': record['error']}
            payload = json.dumps(body, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
            if len(payload) > MAX_REPLY_BYTES:
                raise ValueError('Layout reply exceeds its bounded envelope size.')
            reply = local / 'reply.part'
            reply.write_bytes(payload)
            output = self.hdc(['file', 'send', '-b', self.bundle, str(reply), item['replyPath'].lstrip('/')], timeout=remaining())
            require_transfer(output, len(payload))
            ready_data = {**identity, 'payloadBytes': len(payload), 'payloadSha256': hashlib.sha256(payload).hexdigest()}
            ready = local / 'ready.json'
            ready.write_bytes(json.dumps(ready_data, separators=(',', ':')).encode('utf-8'))
            output = self.hdc(['file', 'send', '-b', self.bundle, str(ready), item['readyPath'].lstrip('/')], timeout=remaining())
            require_transfer(output, ready.stat().st_size)
            remaining()
            record.update(status='delivered' if body['ok'] else 'error-delivered', payloadBytes=len(payload),
                          payloadSha256=ready_data['payloadSha256'])
        except Exception as error:
            record.update(status='failed', error=str(error)[:1000])
            self.defects.append('Layout relay delivery failed: ' + record['error'])
        finally:
            record['durationMs'] = round((time.monotonic() - started) * 1000)
            self.records.append(record)
            for file in local.iterdir():
                file.unlink()

    def retain_failure_query(self, phase: str):
        if self.last_layout is not None:
            path = self.report_dir / f'{phase}-last-query.json'
            path.write_bytes(self.last_layout)
            return str(path)
        return None

    def close(self):
        self.temp.cleanup()
