import copy
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from layout_file_relay import FileLayoutRelay, MAX_LAYOUT_BYTES, require_transfer, validate_layout, validate_request
from stream_process import stream_process

RUN = 'relay-run'
BUNDLE = 'moe.kiwi.photocraft'
ENTRY = '/data/storage/el2/base/haps/entry'
CACHE = f'{ENTRY}/cache/PhotoCraftTestRuns/{RUN}/cache'
FIXTURE = Path(__file__).resolve().parents[2] / 'tests/fixtures/uitest-photocraft.layout.json'


def request(**values):
    directory = f'{CACHE}/LayoutRelay-1791437885660-1'
    return {'runId': RUN, 'bundleName': BUNDLE, 'path': f'/data/local/tmp/PhotoCraftTest-{RUN}-1791437885660-0-query.json',
            'ticket': 'layout-1', 'nonce': 'a' * 64, 'replyPath': directory + '/reply.part',
            'readyPath': directory + '/ready.json', **values}


def transfer(size):
    return f'FileTransfer finish, Size:{size}, File count = 1, time:5ms rate:17.20kB/s\n'


def bind_roots(relay):
    relay.line('PHOTOCRAFT_STORAGE_ROOTS ' + json.dumps({'runId': RUN, 'suite': 'PhotoCraftCore',
        'entryFilesDir': ENTRY + '/files', 'entryCacheDir': ENTRY + '/cache'}))
    relay.line('PHOTOCRAFT_NATIVE_ROOTS ' + json.dumps({'runId': RUN, 'pid': 1234,
        'nativeCache': CACHE, 'expectedCache': CACHE, 'nativeFiles': f'{ENTRY}/files/PhotoCraftTestRuns/{RUN}/files',
        'expectedFiles': f'{ENTRY}/files/PhotoCraftTestRuns/{RUN}/files',
        'abilityFilesDir': ENTRY + '/files', 'abilityCacheDir': ENTRY + '/cache'}))


class RequestValidationTests(unittest.TestCase):
    def test_wrong_run_bundle_ticket_nonce_source_and_reply_paths_never_pass_authentication(self):
        self.assertEqual(validate_request(request(), RUN, BUNDLE, CACHE), request())
        bad = [{'runId': 'wrong'}, {'bundleName': 'foreign.app'}, {'ticket': 'layout-0'}, {'ticket': 'layout-1;bad'},
               {'nonce': 'a' * 63}, {'nonce': 'G' * 64}, {'path': '/data/local/tmp/ordinary.json'},
               {'path': request()['path'] + ';rm'}, {'replyPath': request()['replyPath'].replace(RUN, 'other-run')},
               {'readyPath': CACHE + '/ordinary.json'}, {'port': 12345}]
        for values in bad:
            with self.subTest(values=values), self.assertRaises(ValueError):
                validate_request(request(**values), RUN, BUNDLE, CACHE)
        with self.assertRaises(ValueError):
            validate_request(request(), RUN, BUNDLE, None)

    def test_layout_bound_utf8_complete_tree_and_bundle_are_required(self):
        raw = FIXTURE.read_bytes()
        self.assertIsInstance(validate_layout(raw, BUNDLE), dict)
        for invalid in (b'', b'{}', b'not JSON', b'\xff', b'[]', b'0' * (MAX_LAYOUT_BYTES + 1),
                        raw.replace(BUNDLE.encode(), b'foreign.application'), b'{"attributes":{},"attributes":{}}'):
            with self.subTest(length=len(invalid)), self.assertRaises((ValueError, UnicodeError)):
                validate_layout(invalid, BUNDLE)

    def test_only_scoped_empty_root_can_prove_the_application_window_is_absent(self):
        self.assertEqual(validate_layout(b'{"attributes":{},"children":[]}', BUNDLE), {'attributes': {}, 'children': []})
        actual_empty = FIXTURE.with_name('uitest-empty.layout.json').read_bytes()
        self.assertEqual(validate_layout(actual_empty, BUNDLE), json.loads(actual_empty))
        for tree in ({'attributes': {'bundleName': 'foreign.app'}, 'children': []},
                     {'attributes': {'text': 'foreign window content'}, 'children': []},
                     {'attributes': {'bounds': '[0,0][2776,1775]'}, 'children': []},
                     {'attributes': {}, 'children': [{'attributes': {'text': 'unscoped content'}}]}):
            with self.subTest(tree=tree), self.assertRaises(ValueError):
                validate_layout(json.dumps(tree).encode(), BUNDLE)

    def test_hdc_exit_success_text_alone_is_not_a_transfer_receipt(self):
        require_transfer(transfer(86), 86)
        for output in ('success', transfer(85), transfer(86).replace('count = 1', 'count = 2'),
                       transfer(86) + '[Fail]Error opening file', transfer(86) * 2):
            with self.subTest(output=output), self.assertRaises(ValueError):
                require_transfer(output, 86)


class FileRelayTests(unittest.TestCase):
    def setup_relay(self, temp, *, layout=None, on_ready=None):
        calls = []
        device_files = {}
        raw = FIXTURE.read_bytes() if layout is None else layout
        def hdc(argv, **options):
            calls.append(argv)
            self.assertGreater(options['timeout'], 0)
            self.assertLessEqual(options['timeout'], 10)
            if argv[:2] == ['file', 'recv']:
                Path(argv[-1]).write_bytes(raw)
                return transfer(len(raw))
            self.assertEqual(argv[:4], ['file', 'send', '-b', BUNDLE])
            payload = Path(argv[-2]).read_bytes()
            device_files[argv[-1]] = payload
            if argv[-1].endswith('/ready.json'):
                ready = json.loads(payload)
                part = device_files[argv[-1].rsplit('/', 1)[0] + '/reply.part']
                self.assertEqual(ready['payloadBytes'], len(part))
                self.assertEqual(ready['payloadSha256'], hashlib.sha256(part).hexdigest())
                body = json.loads(part)
                for field in ('runId', 'nonce', 'ticket', 'path'):
                    self.assertEqual(ready[field], body[field])
                if on_ready:
                    on_ready(body, ready)
            return transfer(len(payload))
        relay = FileLayoutRelay(RUN, BUNDLE, 'PhotoCraftCore', Path(temp), hdc)
        self.addCleanup(relay.close)
        bind_roots(relay)
        return relay, calls, device_files

    def test_ready_is_published_only_after_authenticated_body_and_actual_callback(self):
        with tempfile.TemporaryDirectory() as temp:
            callbacks = []
            relay, calls, files = self.setup_relay(temp, on_ready=lambda body, ready: callbacks.append((body, ready)))
            relay.line('PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request()))
            self.assertEqual(relay.defects, [])
            self.assertEqual(relay.records[0]['status'], 'delivered')
            self.assertEqual(len(callbacks), 1)
            body, ready = callbacks[0]
            self.assertTrue(body['ok'])
            self.assertEqual(body['layout'], json.loads(FIXTURE.read_text()))
            self.assertEqual([argv[:2] for argv in calls], [['file', 'recv'], ['file', 'send'], ['file', 'send']])
            self.assertTrue(calls[1][-1].endswith('/reply.part'))
            self.assertTrue(calls[2][-1].endswith('/ready.json'))
            self.assertTrue(all(path.startswith(CACHE.lstrip('/') + '/') for path in files))
            self.assertEqual(list(Path(relay.temp.name).iterdir()), [])

    def test_authenticated_host_callback_is_serviced_before_aa_child_can_complete(self):
        with tempfile.TemporaryDirectory() as temp:
            ack = Path(temp) / 'device-ack.json'
            def on_ready(body, ready):
                self.assertTrue(body['ok'])
                self.assertEqual(body['runId'], RUN)
                self.assertEqual(body['nonce'], request()['nonce'])
                self.assertEqual(body['ticket'], request()['ticket'])
                self.assertEqual(body['path'], request()['path'])
                ack.write_text(json.dumps({'authenticated': True}))
            relay, _calls, _files = self.setup_relay(temp, on_ready=on_ready)
            marker = 'PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request())
            source = f'''import pathlib,time,json
print({marker!r},flush=True)
path=pathlib.Path({str(ack)!r}); deadline=time.monotonic()+2
while not path.exists() and time.monotonic()<deadline: time.sleep(.01)
assert json.loads(path.read_text()) == {{'authenticated': True}}
print('AA continued after actual authenticated callback',flush=True)
'''
            result = stream_process([sys.executable, '-u', '-c', source], cwd=Path(temp), env=os.environ.copy(),
                timeout=3, output_path=Path(temp) / 'aa.txt', on_line=relay.line)
            self.assertEqual(result.returncode, 0)
            self.assertIn('AA continued after actual authenticated callback', result.stdout)
            self.assertNotIn('"layout":', result.stdout)
            self.assertEqual(relay.defects, [])

    def test_invalid_metadata_unbound_roots_and_replayed_ticket_never_do_device_io(self):
        with tempfile.TemporaryDirectory() as temp:
            relay, calls, _files = self.setup_relay(temp)
            for values in ({'runId': 'wrong'}, {'ticket': 'wrong'}, {'nonce': 'wrong'}, {'path': '/ordinary'},
                           {'replyPath': '/ordinary/cache/reply.part'}):
                relay.line('PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request(**values)))
            relay.line('PHOTOCRAFT_LAYOUT_REQUEST {malformed JSON}')
            self.assertEqual(calls, [])
            self.assertEqual(len(relay.defects), 6)
            relay.line('PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request()))
            count = len(calls)
            relay.line('PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request()))
            self.assertEqual(len(calls), count)
            self.assertIn('reuses', relay.defects[-1])
            relay.cache = None
            relay.line('PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request(ticket='layout-2')))
            self.assertEqual(len(calls), count)

    def test_oversize_malformed_and_absent_layout_are_error_replies_not_passes(self):
        for raw in (b'{}', b'invalid JSON', b'x' * (MAX_LAYOUT_BYTES + 1)):
            with self.subTest(size=len(raw)), tempfile.TemporaryDirectory() as temp:
                relay, _calls, files = self.setup_relay(temp, layout=raw)
                relay.line('PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request()))
                self.assertTrue(relay.defects)
                self.assertEqual(relay.records[0]['status'], 'error-delivered')
                body = json.loads(next(value for path, value in files.items() if path.endswith('/reply.part')))
                self.assertFalse(body['ok'])
                self.assertIn('error', body)
                self.assertNotIn('layout', body)

    def test_transfer_timeout_or_incorrect_body_receipt_never_publishes_ready(self):
        with tempfile.TemporaryDirectory() as temp:
            relay, calls, _files = self.setup_relay(temp)
            original = relay.hdc
            def no_body(argv, **options):
                if argv[:2] == ['file', 'send']:
                    raise TimeoutError('Send timed out')
                return original(argv, **options)
            relay.hdc = no_body
            relay.line('PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request()))
            self.assertEqual(relay.records[0]['status'], 'failed')
            self.assertTrue(relay.defects)
            self.assertFalse(any(argv[-1].endswith('/ready.json') for argv in calls))
            self.assertEqual(list(Path(relay.temp.name).iterdir()), [])

    def test_wrong_body_byte_receipt_cannot_publish_ready_even_when_hdc_claims_exit_success(self):
        with tempfile.TemporaryDirectory() as temp:
            relay, calls, _files = self.setup_relay(temp)
            original = relay.hdc
            attempted = []
            def incomplete_body(argv, **options):
                attempted.append(argv)
                if argv[:2] == ['file', 'send']:
                    return transfer(Path(argv[-2]).stat().st_size - 1)
                return original(argv, **options)
            relay.hdc = incomplete_body
            relay.line('PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request()))
            self.assertEqual(relay.records[0]['status'], 'failed')
            self.assertFalse(any(argv[-1].endswith('/ready.json') for argv in attempted))

    def test_total_relay_deadline_prevents_late_body_or_ready_publication(self):
        with tempfile.TemporaryDirectory() as temp:
            relay, calls, _files = self.setup_relay(temp)
            with patch('layout_file_relay.time.monotonic', side_effect=[0, 0, 11, 11]):
                relay.line('PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps(request()))
            self.assertEqual(relay.records[0]['status'], 'failed')
            self.assertIn('10-second deadline', relay.records[0]['error'])
            self.assertEqual(len(calls), 1)
            self.assertEqual(calls[0][:2], ['file', 'recv'])

    def test_app_cleanup_error_remains_an_infrastructure_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            relay, calls, _files = self.setup_relay(temp)
            relay.line('PHOTOCRAFT_LAYOUT_CLEANUP_ERROR ' + json.dumps({'runId': RUN, 'ticket': 'layout-1',
                                                                      'code': 13900001, 'message': 'owned directory cleanup denied'}))
            self.assertIn('App layout relay cleanup failed: owned directory cleanup denied', relay.defects)
            self.assertEqual(calls, [])

    def test_native_cache_evidence_cannot_switch_to_normal_data(self):
        with tempfile.TemporaryDirectory() as temp:
            relay, calls, _files = self.setup_relay(temp)
            relay.line('PHOTOCRAFT_NATIVE_ROOTS ' + json.dumps({'runId': RUN, 'pid': 123,
                                                               'nativeCache': ENTRY + '/cache'}))
            self.assertTrue(relay.defects)
            self.assertEqual(calls, [])


if __name__ == '__main__':
    unittest.main()
