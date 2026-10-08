"""Contract tests for result parsing, argument boundaries, and stage isolation."""
import argparse
import copy
from contextlib import nullcontext, redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
import warnings
from unittest.mock import Mock, patch
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from test_runner import Runner, PHOTOCRAFT_DEVICE_CASES, TEST_INTERFACE_MARKERS, TestError, aa_arguments, inspect_hap_interfaces, local_host_packages, main, node_test_files, parse_cache_inventory, parse_failure_evidence, parse_host_output, parse_instrument_output, parse_isolation_evidence, parse_query_files, parse_runtime_evidence, parse_scope, parse_storage_roots_metadata, parse_testmode, parse_ui_driver_metadata, remote_hdc_arguments, require_hap_interface_mode, requested_device_cases, validate_device_case_inventory
from build_device_tests import BuildError, _run, prepare_stage, reuse_artifacts, source_fingerprint


def transcript(codes=(0,), *, total=None, failed=0, errors=0, passed=None, skipped=0, report_code=0):
    lines = []
    for index, code in enumerate(codes):
        for state in (1, code):
            lines += ['OHOS_REPORT_STATUS: class=PhotoCraft', f'OHOS_REPORT_STATUS: test=case{index}',
                      f'OHOS_REPORT_STATUS_CODE: {state}']
        lines.append('OHOS_REPORT_STATUS: consuming=12')
    total = len(codes) if total is None else total
    passed = codes.count(0) if passed is None else passed
    lines += [f'OHOS_REPORT_RESULT: stream=Tests run: {total}, Failure: {failed}, Error: {errors}, Pass: {passed}, Ignore: {skipped}',
              f'OHOS_REPORT_CODE: {report_code}']
    return '\n'.join(lines)


def storage_fingerprint(*, entries=7, size=120):
    labels = ('application.files', 'application.cache', 'entry.files', 'entry.cache')
    return {'digest': 'a' * 64, 'entries': entries, 'bytes': size,
            'roots': [{'labels': [label], 'pathDigest': str(index + 1) * 64, 'digest': str(index + 5) * 64,
                       'entries': entries if index == 0 else 0, 'bytes': size if index == 0 else 0}
                      for index, label in enumerate(labels)]}


def write_hap_fixture(path, markers=(), *, native=True, native_bytes=None):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as archive:
        archive.writestr('ets/modules.abc', b'ArkTS runner fixture')
        if native:
            binary = native_bytes if native_bytes is not None else b'\x7fELF\0fixture\0' + b'\0'.join(marker.encode() for marker in markers)
            archive.writestr('libs/arm64-v8a/libphotocraft.so', binary)
    return path


class HapInterfaceTests(unittest.TestCase):
    def test_actual_zip_native_markers_and_sha_are_inspected(self):
        import hashlib
        with tempfile.TemporaryDirectory() as temp:
            hap = write_hap_fixture(Path(temp) / 'ordinary.hap')
            evidence = inspect_hap_interfaces(hap, 'libphotocraft.so')
            require_hap_interface_mode(evidence, device_tests=False)
            self.assertEqual(evidence['hapSha256'], hashlib.sha256(hap.read_bytes()).hexdigest())
            self.assertFalse(any(evidence['testInterfaceMarkers'].values()))
            hap = write_hap_fixture(Path(temp) / 'device.hap', TEST_INTERFACE_MARKERS)
            evidence = inspect_hap_interfaces(hap, 'libphotocraft.so')
            self.assertTrue(all(evidence['testInterfaceMarkers'].values()))
            require_hap_interface_mode(evidence, device_tests=True)

    def test_one_export_rejects_ordinary_and_missing_export_rejects_device(self):
        with tempfile.TemporaryDirectory() as temp:
            for marker in TEST_INTERFACE_MARKERS:
                with self.subTest(marker=marker):
                    hap = write_hap_fixture(Path(temp) / 'contaminated.hap', [marker])
                    evidence = inspect_hap_interfaces(hap, 'libphotocraft.so')
                    with self.assertRaisesRegex(TestError, 'Ordinary restoration HAP contains'):
                        require_hap_interface_mode(evidence, device_tests=False)
                    hap = write_hap_fixture(Path(temp) / 'missing.hap', [item for item in TEST_INTERFACE_MARKERS if item != marker])
                    evidence = inspect_hap_interfaces(hap, 'libphotocraft.so')
                    with self.assertRaisesRegex(TestError, 'missing one or more'):
                        require_hap_interface_mode(evidence, device_tests=True)

    def test_missing_duplicate_non_elf_and_invalid_zip_fail(self):
        with tempfile.TemporaryDirectory() as temp:
            hap = write_hap_fixture(Path(temp) / 'missing.hap', native=False)
            with self.assertRaisesRegex(TestError, 'exactly one'):
                inspect_hap_interfaces(hap, 'libphotocraft.so')
            hap = write_hap_fixture(Path(temp) / 'duplicate.hap')
            with warnings.catch_warnings():
                warnings.simplefilter('ignore', UserWarning)
                with zipfile.ZipFile(hap, 'a') as archive:
                    archive.writestr('libs/arm64-v8a/libphotocraft.so', b'\x7fELF duplicate')
            with self.assertRaisesRegex(TestError, 'exactly one'):
                inspect_hap_interfaces(hap, 'libphotocraft.so')
            hap = write_hap_fixture(Path(temp) / 'non-elf.hap', native_bytes=b'not an ELF')
            with self.assertRaisesRegex(TestError, 'not an ELF'):
                inspect_hap_interfaces(hap, 'libphotocraft.so')
            hap.write_bytes(b'not a ZIP')
            with self.assertRaisesRegex(TestError, 'Cannot inspect'):
                inspect_hap_interfaces(hap, 'libphotocraft.so')

    def test_interface_marker_crossing_stream_chunk_is_not_missed(self):
        with tempfile.TemporaryDirectory() as temp:
            binary = b'\x7fELF' + b'x' * (1024 * 1024 - 6) + b'testSubmit'
            hap = write_hap_fixture(Path(temp) / 'chunked.hap', native_bytes=binary)
            self.assertTrue(inspect_hap_interfaces(hap, 'libphotocraft.so')['testInterfaceMarkers']['testSubmit'])


class InstrumentParserTests(unittest.TestCase):
    def test_semantic_stream_failure_reason_and_stack_survive_json_and_junit(self):
        output = transcript((-1,), errors=1).replace('OHOS_REPORT_STATUS_CODE: -1',
            'OHOS_REPORT_STATUS: stream=Error: owned layout file unreadable\n'
            'OHOS_REPORT_STATUS: stack=at check (DeviceUiDriver.ets:26)\nOHOS_REPORT_STATUS_CODE: -1')
        result = parse_instrument_output(output)
        case = result['cases'][0]
        self.assertEqual(case['message'], 'Error: owned layout file unreadable\nat check (DeviceUiDriver.ets:26)')
        self.assertEqual(case['stack'], 'at check (DeviceUiDriver.ets:26)')
        with tempfile.TemporaryDirectory() as temp:
            args = argparse.Namespace(project=temp, command='device', scope=None, device=None)
            runner = Runner(args)
            runner.report['cases'] = result['cases']
            with redirect_stdout(io.StringIO()):
                runner.finish('failed')
            saved = json.loads((runner.report_dir / 'report.json').read_text())
            self.assertEqual(saved['cases'][0]['message'], case['message'])
            import xml.etree.ElementTree as ET
            self.assertEqual(ET.parse(runner.report_dir / 'junit.xml').find('.//error').text, case['message'])

    def test_positive_protocol_cannot_hide_a_missing_registered_suite_case(self):
        names = PHOTOCRAFT_DEVICE_CASES['PhotoCraftCore']
        cases = [{'suite': 'PhotoCraftCore', 'name': name} for name in names]
        self.assertTrue(validate_device_case_inventory('PhotoCraftCore', cases)['passed'])
        missing = validate_device_case_inventory('PhotoCraftCore', cases[:-1])
        self.assertFalse(missing['passed'])
        self.assertIn('Missing registered case: PhotoCraftCore#background_foreground.', missing['defects'])
        # The protocol itself permits a positive result for fewer cases; scope
        # inventory must be a separate, stronger guard.
        self.assertTrue(parse_instrument_output(transcript((0, 0, 0, 0)))['passed'])

    def test_exact_case_scope_duplicates_unknown_and_all_fifteen_are_checked(self):
        case = {'suite': 'PhotoCraftCore', 'name': 'new_edit_undo_redo'}
        self.assertTrue(validate_device_case_inventory('PhotoCraftCore#new_edit_undo_redo', [case])['passed'])
        self.assertFalse(validate_device_case_inventory('PhotoCraftCore#new_edit_undo_redo', [case, case])['passed'])
        self.assertFalse(validate_device_case_inventory('PhotoCraftCore#unknown', [case])['passed'])
        self.assertFalse(validate_device_case_inventory('PhotoCraftCore,PhotoCraftCore#new_edit_undo_redo', [case])['passed'])
        scope = ','.join(PHOTOCRAFT_DEVICE_CASES)
        all_cases = [{'suite': suite, 'name': name} for suite, name in requested_device_cases(scope)]
        self.assertEqual(len(all_cases), 15)
        self.assertTrue(validate_device_case_inventory(scope, all_cases)['passed'])
        self.assertFalse(validate_device_case_inventory(scope, all_cases[:-1])['passed'])

    def test_device_inventory_matches_the_registered_arkts_cases(self):
        import re
        folder = Path(__file__).resolve().parents[2] / 'harmonyos/entry/src/ohosTest/ets/test'
        registered = {}
        for path in folder.glob('*.test.ets'):
            suite = None
            for line in path.read_text().splitlines():
                match = re.search(r"describe\('([^']+)'", line)
                if match:
                    suite = match.group(1)
                    registered[suite] = []
                match = re.search(r"it\('([^']+)'", line)
                if match:
                    registered[suite].append(match.group(1))
        self.assertEqual(registered, {suite: list(names) for suite, names in PHOTOCRAFT_DEVICE_CASES.items()})

    def test_real_protocol_pass_and_duration(self):
        result = parse_instrument_output(transcript((0, 0)))
        self.assertTrue(result['passed'])
        self.assertEqual(len(result['cases']), 2)
        self.assertEqual(result['cases'][0]['durationMs'], 12)

    def test_error_with_success_report_code_fails(self):
        result = parse_instrument_output(transcript((-1,), errors=1, report_code=0))
        self.assertFalse(result['passed'])
        self.assertEqual(result['cases'][0]['status'], 'error')

    def test_failure_and_ignore_cannot_pass(self):
        for output in (transcript((-2,), failed=1, report_code=-1), transcript((-3,), skipped=1)):
            with self.subTest(output=output):
                self.assertFalse(parse_instrument_output(output)['passed'])

    def test_missing_zero_and_crash_fail(self):
        for output in ('', 'aa test finished successfully', transcript((), passed=0),
                       'OHOS_REPORT_STATUS: class=PhotoCraft\nOHOS_REPORT_STATUS: test=case0\nOHOS_REPORT_STATUS_CODE: 1',
                       transcript().replace('OHOS_REPORT_CODE: 0', '')):
            with self.subTest(output=output):
                self.assertFalse(parse_instrument_output(output)['passed'])

    def test_summary_cannot_hide_failure(self):
        self.assertFalse(parse_instrument_output(transcript((-2, 0), total=2, failed=0, passed=2))['passed'])

    def test_summary_count_disagreement_fails(self):
        self.assertFalse(parse_instrument_output(transcript((0,), total=2, passed=2))['passed'])

    def test_duplicate_and_completion_without_start_fail(self):
        output = transcript()
        duplicate = output.replace('OHOS_REPORT_RESULT:', 'OHOS_REPORT_STATUS: class=PhotoCraft\nOHOS_REPORT_STATUS: test=case0\nOHOS_REPORT_STATUS_CODE: 0\nOHOS_REPORT_RESULT:')
        self.assertFalse(parse_instrument_output(duplicate)['passed'])
        self.assertFalse(parse_instrument_output(output.replace('OHOS_REPORT_STATUS_CODE: 1', ''))['passed'])


class HostParserTests(unittest.TestCase):
    def test_rust_summaries_and_empty_doctests(self):
        output = 'test foo ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\ntest result: ok. 0 passed; 0 failed; 0 ignored;'
        self.assertTrue(parse_host_output(output, 'rust')['valid'])
        self.assertEqual(parse_host_output(output, 'rust')['total'], 1)
        self.assertFalse(parse_host_output('test result: ok. 0 passed; 0 failed; 0 ignored;', 'rust')['valid'])

    def test_node_requires_tap_summary_and_positive_pass(self):
        result = parse_host_output('ok 1 - bytes\n# tests 1\n# pass 1\n# fail 0\n', 'node')
        self.assertTrue(result['valid'])
        self.assertEqual(result['cases'][0]['name'], 'bytes')
        self.assertFalse(parse_host_output('ok 1 - bytes', 'node')['valid'])

    def test_native_tap_requires_exact_plan_and_real_case_results(self):
        output = 'TAP version 13\n1..2\nok 1 - chinese\nok 2 - emoji\n'
        self.assertTrue(parse_host_output(output, 'tap')['valid'])
        self.assertEqual(parse_host_output(output, 'tap')['total'], 2)
        for invalid in ('', 'native tests passed', '1..0', output.replace('1..2', '1..3'),
                        output.replace('ok 2', 'ok 1'), output + 'Bail out! crash\n'):
            self.assertFalse(parse_host_output(invalid, 'tap')['valid'])
        self.assertEqual(parse_host_output(output.replace('ok 2', 'not ok 2'), 'tap')['failed'], 1)
        self.assertEqual(parse_host_output(output + 'ok 3 - ignored # SKIP\n', 'tap')['failed'], 1)


class DeviceEvidenceTests(unittest.TestCase):
    def evidence(self, suite='PhotoCraftCore', run_id='actual_run'):
        fingerprint = storage_fingerprint()
        return {'runId': run_id, 'suite': suite, 'before': copy.deepcopy(fingerprint), 'after': copy.deepcopy(fingerprint), 'unchanged': True}

    def line(self, evidence):
        return 'PHOTOCRAFT_ISOLATION ' + json.dumps(evidence)

    def test_testmode_exact_unavailable_error_allows_read_only_default_and_unknown_values_fail(self):
        result = parse_testmode('Get parameter "persist.ace.testmode.enabled" fail! errNum is:1002!\n')
        self.assertIs(result['readable'], False)
        self.assertIsNone(result['enabled'])
        self.assertIsNone(result['value'])
        for value in ('', 'garbage', 'Get parameter "persist.ace.testmode.enabled" fail! errNum is:1001!'):
            with self.assertRaises(TestError):
                parse_testmode(value)
        self.assertIs(parse_testmode('1')['enabled'], True)
        self.assertIs(parse_testmode('0')['readable'], True)

    def test_matching_run_and_suite_fingerprints_are_required(self):
        output = '\n'.join(self.line(self.evidence(suite)) for suite in ('PhotoCraftCore', 'PhotoCraftFiles'))
        self.assertTrue(parse_isolation_evidence(output, 'actual_run', 'PhotoCraftCore,PhotoCraftFiles')['passed'])
        self.assertFalse(parse_isolation_evidence('', 'actual_run', 'PhotoCraftCore#new_edit_undo_redo')['passed'])
        self.assertFalse(parse_isolation_evidence(self.line(self.evidence(run_id='wrong_run')), 'actual_run', 'PhotoCraftCore')['passed'])
        self.assertFalse(parse_isolation_evidence(self.line(self.evidence()), 'actual_run', 'PhotoCraftFiles')['passed'])

    def test_changed_hash_counts_or_false_claim_are_not_accepted(self):
        for key, value in (('digest', 'b' * 64), ('entries', 8), ('bytes', 121)):
            evidence = self.evidence()
            evidence['after'][key] = value
            self.assertFalse(parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')['passed'])
        evidence = self.evidence()
        evidence['unchanged'] = False
        self.assertFalse(parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')['passed'])

    def test_duplicate_or_malformed_proof_fails(self):
        line = self.line(self.evidence())
        self.assertFalse(parse_isolation_evidence(line + '\n' + line, 'actual_run', 'PhotoCraftCore')['passed'])
        evidence = self.evidence()
        evidence['before']['entries'] = True
        self.assertFalse(parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')['passed'])
        self.assertFalse(parse_isolation_evidence('PHOTOCRAFT_ISOLATION {broken}', 'actual_run', 'PhotoCraftCore')['passed'])

    def test_old_application_only_proof_cannot_satisfy_isolation(self):
        for roots in (None, [{'labels': ['application.files'], 'pathDigest': '1' * 64},
                             {'labels': ['application.cache'], 'pathDigest': '2' * 64}]):
            evidence = self.evidence()
            for phase in ('before', 'after'):
                if roots is None:
                    evidence[phase].pop('roots')
                else:
                    evidence[phase]['roots'] = roots
            self.assertFalse(parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')['passed'])

    def test_aliases_merge_labels_and_stable_root_mapping_is_required(self):
        evidence = self.evidence()
        original = storage_fingerprint()['roots']
        roots = [{**original[0], 'labels': ['application.files', 'entry.files']}, original[1], original[3]]
        evidence['before']['roots'] = roots
        evidence['after']['roots'] = list(reversed(roots))
        self.assertTrue(parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')['passed'])
        evidence['after']['roots'] = [{**root, 'pathDigest': '5' * 64} if index == 0 else root
                                     for index, root in enumerate(roots)]
        self.assertFalse(parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')['passed'])

    def test_duplicate_root_labels_paths_and_unknown_roots_fail(self):
        for roots in ([{'labels': ['application.files', 'application.files'], 'pathDigest': '1' * 64}],
                      [{'labels': ['ordinary.files'], 'pathDigest': '1' * 64}],
                      [{'labels': ['application.files', 'entry.files'], 'pathDigest': '1' * 64},
                       {'labels': ['application.cache', 'entry.cache'], 'pathDigest': '1' * 64}]):
            evidence = self.evidence()
            evidence['before']['roots'] = roots
            evidence['after']['roots'] = roots
            self.assertFalse(parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')['passed'])

    def test_per_root_change_cannot_hide_behind_equal_aggregate_digest(self):
        evidence = self.evidence()
        evidence['after']['roots'][0]['digest'] = 'f' * 64
        self.assertFalse(parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')['passed'])
        evidence = self.evidence()
        evidence['after']['roots'][0]['bytes'] += 1
        self.assertFalse(parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')['passed'])

    def test_failed_isolation_retains_hash_and_file_differences_for_diagnosis(self):
        evidence = self.evidence()
        evidence['after']['bytes'] += 42
        evidence['after']['roots'][0]['bytes'] += 42
        evidence['unchanged'] = False
        evidence['differences'] = [{'path': 'files/hiappevent/logs/event', 'afterBytes': 42}]
        parsed = parse_isolation_evidence(self.line(evidence), 'actual_run', 'PhotoCraftCore')
        self.assertFalse(parsed['passed'])
        self.assertEqual(parsed['evidence'], [evidence])

    def test_startup_pid_evidence_requires_exact_scope_run_and_integer_pid(self):
        item = {'runId': 'actual_run', 'suite': 'PhotoCraftCore', 'pid': 43490}
        line = 'PHOTOCRAFT_RUNTIME ' + json.dumps(item)
        self.assertEqual(parse_runtime_evidence(line, 'actual_run', 'PhotoCraftCore#case')['records'], [item])
        for field, value in (('runId', 'another_run'), ('suite', 'PhotoCraftFiles'), ('pid', True), ('pid', 0)):
            changed = {**item, field: value}
            result = parse_runtime_evidence('PHOTOCRAFT_RUNTIME ' + json.dumps(changed), 'actual_run', 'PhotoCraftCore')
            self.assertEqual(result['records'], [])
            self.assertTrue(result['defects'])

    def test_cache_inventory_only_retains_valid_metadata_for_this_phase(self):
        item = {'runId': 'actual_run', 'suite': 'PhotoCraftCore', 'relativePath': 'shader/cache.bin',
                'pathDigest': 'a' * 64, 'size': 81, 'hash': 'b' * 64, 'rootLabels': ['application.cache']}
        line = 'PHOTOCRAFT_CACHE_INVENTORY ' + json.dumps(item)
        self.assertEqual(parse_cache_inventory(line, 'actual_run', 'PhotoCraftCore')['records'], [item])
        for field, value in (('relativePath', '../normal.bin'), ('relativePath', '/normal.bin'),
                             ('size', True), ('hash', 'not-sha256'), ('runId', 'another_run'),
                             ('rootLabels', ['cache']), ('rootLabels', ['application.files'])):
            result = parse_cache_inventory('PHOTOCRAFT_CACHE_INVENTORY ' + json.dumps({**item, field: value}),
                                           'actual_run', 'PhotoCraftCore')
            self.assertEqual(result['records'], [])
            self.assertTrue(result['defects'])

    def test_root_paths_metadata_is_bound_to_each_declared_path_digest(self):
        import hashlib
        item = {'runId': 'actual_run', 'suite': 'PhotoCraftCore',
                'applicationFilesDir': '/data/storage/el2/base/files', 'applicationCacheDir': '/data/storage/el2/base/cache',
                'entryFilesDir': '/data/storage/el2/base/haps/entry/files', 'entryCacheDir': '/data/storage/el2/base/haps/entry/cache'}
        evidence = self.evidence()
        fields = ('applicationFilesDir', 'applicationCacheDir', 'entryFilesDir', 'entryCacheDir')
        for phase in ('before', 'after'):
            for index, field in enumerate(fields):
                evidence[phase]['roots'][index]['pathDigest'] = hashlib.sha256(item[field].encode()).hexdigest()
        isolation = {'evidence': [evidence]}
        line = 'PHOTOCRAFT_STORAGE_ROOTS ' + json.dumps(item)
        self.assertEqual(parse_storage_roots_metadata(line, 'actual_run', 'PhotoCraftCore', isolation)['records'], [item])
        for field, value in (('entryFilesDir', '/data/storage/el2/base/other/files'),
                             ('applicationCacheDir', 'relative/cache'), ('runId', 'wrong_run')):
            result = parse_storage_roots_metadata('PHOTOCRAFT_STORAGE_ROOTS ' + json.dumps({**item, field: value}),
                                                  'actual_run', 'PhotoCraftCore', isolation)
            self.assertTrue(result['defects'])
            self.assertEqual(result['records'], [])

    def test_failure_capture_paths_are_owned_by_exact_phase_and_share_prefix(self):
        item = {'runId': 'actual_run', 'suite': 'PhotoCraftCore', 'label': 'save menu timeout',
                'screenPath': '/data/local/tmp/PhotoCraftTest-actual_run-12345-failure.png',
                'layoutPath': '/data/local/tmp/PhotoCraftTest-actual_run-12345-failure.json'}
        line = 'PHOTOCRAFT_FAILURE_EVIDENCE ' + json.dumps(item)
        self.assertEqual(parse_failure_evidence(line, 'actual_run', 'PhotoCraftCore')['records'][0]['timestamp'], '12345')
        for field, value in (('runId', 'another_run'), ('suite', 'PhotoCraftFiles'),
                             ('screenPath', '/data/local/tmp/PhotoCraftTest-another_run-12345-failure.png'),
                             ('screenPath', '/data/local/tmp/../private/PhotoCraftTest-actual_run-12345-failure.png'),
                             ('layoutPath', '/data/local/tmp/PhotoCraftTest-actual_run-99999-failure.json'),
                             ('screenPath', '/data/local/tmp/PhotoCraftTest-actual_run-12345-failure.png; rm -rf /')):
            parsed = parse_failure_evidence('PHOTOCRAFT_FAILURE_EVIDENCE ' + json.dumps({**item, field: value}),
                                            'actual_run', 'PhotoCraftCore')
            self.assertEqual(parsed['records'], [])
            self.assertTrue(parsed['defects'])
        self.assertTrue(parse_failure_evidence(line + '\n' + line, 'actual_run', 'PhotoCraftCore')['defects'])

    def test_failure_marker_allows_one_capture_but_no_empty_capture(self):
        item = {'runId': 'actual_run', 'suite': 'PhotoCraftCore', 'label': 'failure',
                'layoutPath': '/data/local/tmp/PhotoCraftTest-actual_run-12345-failure.json'}
        self.assertEqual(len(parse_failure_evidence('PHOTOCRAFT_FAILURE_EVIDENCE ' + json.dumps(item),
                                                    'actual_run', 'PhotoCraftCore')['records']), 1)
        del item['layoutPath']
        self.assertTrue(parse_failure_evidence('PHOTOCRAFT_FAILURE_EVIDENCE ' + json.dumps(item),
                                              'actual_run', 'PhotoCraftCore')['defects'])


class ArgumentTests(unittest.TestCase):
    def test_scope_rejects_empty_or_multiple_hash(self):
        for value in ('#case', 'Suite#', 'Suite#case#extra', 'Suite\ncase'):
            with self.assertRaises(argparse.ArgumentTypeError):
                parse_scope(value)
        self.assertEqual(parse_scope('Suite#case'), ('Suite', 'case'))

    def test_aa_run_id_and_scope_are_individual_arguments(self):
        arguments = aa_arguments('app.photocraft', 'safe_run-123', 'PhotoCraft#save png')
        self.assertEqual(arguments[-3:], ['-s', 'class', 'PhotoCraft#save png'])
        self.assertEqual(arguments[arguments.index('photocraftTestRunId') + 1], 'safe_run-123')
        self.assertIn('false', arguments)
        self.assertEqual(arguments[arguments.index('-m') + 1], 'entry_test')

    def test_remote_shell_escaping_preserves_scope_and_empty_parameter(self):
        import shlex
        original = aa_arguments('app.photocraft', 'safe_run-123', 'Suite#case; echo injected')
        remote = remote_hdc_arguments(original)
        self.assertEqual(shlex.split(remote[1]), original[1:])
        self.assertEqual(shlex.split(remote_hdc_arguments(['shell', 'param', 'set', 'test', ''])[1]), ['param', 'set', 'test', ''])


class RolloutDiscoveryTests(unittest.TestCase):
    def metadata(self, facade=False):
        packages = [
            {'id': 'path+file:///project/apps/app#0.1.0', 'name': 'app-ohos', 'source': None},
            {'id': 'path+file:///project/apps/platform#0.1.0', 'name': 'craft-ohos-platform', 'source': None},
            {'id': 'registry+https://index#eframe@0.36.2', 'name': 'eframe', 'source': 'registry+https://index'}]
        members = [package['id'] for package in packages[:2]]
        if facade:
            packages.append({'id': 'path+file:///project/apps/local-facade#eframe@0.36.2', 'name': 'eframe', 'source': None})
            members.append(packages[-1]['id'])
        return {'packages': packages, 'workspace_members': members}

    def test_absent_or_registry_eframe_is_not_selected(self):
        packages = local_host_packages(self.metadata(), 'app-ohos')
        self.assertIsNone(packages['facade'])
        self.assertTrue(packages['app'].startswith('path+file:'))

    def test_local_facade_uses_exact_package_id_to_avoid_registry_ambiguity(self):
        packages = local_host_packages(self.metadata(facade=True), 'app-ohos')
        self.assertEqual(packages['facade'], 'path+file:///project/apps/local-facade#eframe@0.36.2')

    def test_print_standalone_contract_scripts_are_discovered(self):
        with tempfile.TemporaryDirectory() as temp:
            project = Path(temp)
            (project / 'scripts').mkdir()
            for name in ('test-file-contracts.cjs', 'test-group-publication.cjs'):
                (project / 'scripts' / name).write_text('')
            self.assertEqual(len(node_test_files(project)), 2)


class StageTests(unittest.TestCase):
    def test_signer_stream_keeps_only_safe_task_summaries(self):
        with tempfile.TemporaryDirectory() as temp:
            report = Path(temp)
            output = io.StringIO()
            with redirect_stdout(output):
                _run([sys.executable, '-c', 'print("signer --password confidential"); print("Finished :entry:default@PrepareRes"); print("Finished :entry:default@SignHap"); print("BUILD SUCCESSFUL")'],
                     report, os.environ.copy(), report, 'signed', private=True, timeout=5)
            self.assertNotIn('confidential', output.getvalue())
            self.assertNotIn('confidential', (report / 'signed.log').read_text())
            self.assertIn('SignHap', output.getvalue())
            self.assertNotIn('PrepareRes', output.getvalue())
            self.assertIn('PrepareRes', (report / 'signed.log').read_text())
            self.assertIn('BUILD SUCCESSFUL', (report / 'signed.log').read_text())

    def test_stage_enables_hooks_without_touching_normal_sources(self):
        with tempfile.TemporaryDirectory() as temp:
            project = Path(temp) / 'project'
            harmony = project / 'harmonyos'
            (harmony / 'entry/build').mkdir(parents=True)
            (harmony / 'entry/libs').mkdir()
            app = {'app': {'products': [{'name': 'default'}]}, 'modules': [{'name': 'entry', 'targets': [{'name': 'default'}]}]}
            module = {'buildOption': {'arkOptions': {'buildProfileFields': {'DEVICE_TESTS': False}}, 'externalNativeOptions': {'path': './CMakeLists.txt'}}, 'targets': [{'name': 'default'}]}
            (harmony / 'build-profile.json5').write_text(json.dumps(app))
            (harmony / 'entry/build-profile.json5').write_text(json.dumps(module))
            (harmony / 'entry/build/normal.hap').write_text('original')
            (harmony / 'entry/libs/libnormal.a').write_text('original')
            report = Path(temp) / 'report'
            report.mkdir()
            with patch('build_device_tests.json5_load', side_effect=lambda path, env: json.loads(path.read_text())):
                stage = prepare_stage(project, report, Path(temp) / 'feature.a', {})
            staged = json.loads((stage / 'entry/build-profile.json5').read_text())
            self.assertIs(staged['buildOption']['arkOptions']['buildProfileFields']['DEVICE_TESTS'], True)
            self.assertIn('-DPHOTOCRAFT_DEVICE_TESTS=ON', staged['buildOption']['externalNativeOptions']['arguments'])
            self.assertEqual(json.loads((harmony / 'entry/build-profile.json5').read_text()), module)
            self.assertEqual((harmony / 'entry/build/normal.hap').read_text(), 'original')
            self.assertFalse((stage / 'entry/build').exists())
            self.assertFalse((stage / 'entry/libs').exists())
            self.assertFalse(json.loads((stage / 'build-profile.json5').read_text())['app'].get('signingConfigs'))


class QueryFileTests(unittest.TestCase):
    def record(self, **fields):
        return {'runId': 'actual_run', 'bundleName': 'app.photocraft',
                'path': '/data/local/tmp/PhotoCraftTest-actual_run-1791437885660-0-query.json', **fields}

    def parse(self, *records):
        return parse_query_files('\n'.join('PHOTOCRAFT_QUERY_FILE ' + json.dumps(item) for item in records),
                                 'actual_run', 'app.photocraft')

    def test_only_current_run_app_and_picker_unique_paths_are_accepted(self):
        app = self.record()
        picker = self.record(bundleName='com.huawei.hmos.filemanager',
                             path='/data/local/tmp/PhotoCraftTest-actual_run-1791437885660-1-query.json')
        result = self.parse(app, picker)
        self.assertEqual(result, {'records': [app, picker], 'defects': []})

    def test_other_run_bundle_duplicate_and_unowned_paths_are_rejected(self):
        for fields in ({'runId': 'other'}, {'bundleName': 'some.other.app'},
                       {'path': '/data/local/tmp/PhotoCraftTest-other-1-0-query.json'},
                       {'path': '/data/local/tmp/PhotoCraftTest-actual_run-1-0-failure.json'},
                       {'path': '/data/local/tmp/PhotoCraftTest-actual_run-1-../query.json'},
                       {'path': '/data/local/tmp/PhotoCraftTest-actual_run-1-0-query.json; rm -rf /'},
                       {'path': '/data/local/tmp/PhotoCraftTest-actual_run-1-0-query.json\n'},
                       {'path': '/data/local/tmp/PhotoCraftTest-actual_run-1.5-0-query.json'}):
            with self.subTest(fields=fields):
                result = self.parse(self.record(**fields))
                self.assertEqual(result['records'], [])
                self.assertTrue(result['defects'])
        result = self.parse(self.record(), self.record())
        self.assertEqual(len(result['records']), 1)
        self.assertTrue(result['defects'])


class UiDriverMetadataTests(unittest.TestCase):
    def record(self, **fields):
        return {'runId': 'actual_run', 'suite': 'PhotoCraftCore', 'phase': 'after-ability-start',
                'available': False, 'backend': 'uitest-cli', 'error': 'SDK Driver.create returned null', **fields}

    def parse(self, item):
        return parse_ui_driver_metadata('PHOTOCRAFT_UI_DRIVER ' + json.dumps(item),
                                        'actual_run', 'PhotoCraftCore#new_edit_undo_redo')

    def test_null_sdk_with_cli_backend_is_valid_metadata(self):
        item = self.record()
        self.assertEqual(self.parse(item), {'records': [item], 'defects': []})
        self.assertEqual(self.parse(self.record(available=True, backend='uitest-sdk', error=''))['defects'], [])

    def test_driver_metadata_requires_current_scope_run_and_truthful_backend(self):
        for fields in ({'runId': 'other'}, {'suite': 'PhotoCraftFiles'}, {'phase': 'before-ability-start'},
                       {'available': 'false'}, {'backend': 'guessed'}, {'backend': 'uitest-sdk'}, {'error': None}):
            with self.subTest(fields=fields):
                result = self.parse(self.record(**fields))
                self.assertTrue(result['defects'])
                self.assertEqual(result['records'], [])


class DeviceSafetyTests(unittest.TestCase):
    def setup_runner(self, temp):
        project = Path(temp) / 'OH-photocraft'
        normal = project / 'harmonyos/entry/build/default/outputs/default/entry-default-signed.hap'
        write_hap_fixture(normal)
        args = argparse.Namespace(project=str(project), command='device', scope=None, device='SERIAL', case_timeout=60000, reuse_build=None)
        runner = Runner(args)
        runner.repo = Path(temp)
        runner.serial = 'SERIAL'
        runner.identity = {'bundle': 'app.photocraft', 'module': 'entry', 'ability': 'EntryAbility'}
        runner.report['capabilities'] = {'privateSigning': True, 'deviceSuite': True}
        runner.report['device'] = {'serial': 'SERIAL', 'api': 26}
        runner.normal_output_guard = Mock(return_value=nullcontext())
        runner.active_pid = Mock(return_value='')
        runner.hdc = Mock(return_value='operation successfully')
        runner.stream_instrument = Mock(side_effect=lambda argv, *, timeout, run_id, phase, scope:
                                         runner.hdc(argv, timeout=timeout, allow_failure=True))
        return runner

    def artifacts(self, temp):
        default = write_hap_fixture(Path(temp) / 'default.hap', TEST_INTERFACE_MARKERS)
        tests = write_hap_fixture(Path(temp) / 'ohosTest.hap', native=False)
        return {'default': {'path': str(default)}, 'ohosTest': {'path': str(tests)}}

    def test_active_ordinary_app_prevents_build_install_or_force_stop(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            runner.active_pid.return_value = '1234'
            with patch('test_runner.build') as builder:
                with self.assertRaisesRegex(TestError, 'still running'):
                    runner.device()
                builder.assert_not_called()
                runner.hdc.assert_not_called()

    def test_failed_test_restores_ordinary_package_and_only_removes_test_module(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            artifacts = self.artifacts(temp)
            runner.instrument = Mock(side_effect=TestError('assertion failed'))
            with patch('test_runner.build', return_value=artifacts):
                with self.assertRaisesRegex(TestError, 'assertion failed'):
                    runner.device()
            self.assertIs(runner.report['ordinaryPackage']['restored'], True)
            self.assertIs(runner.report['testModuleRemoved'], True)
            calls = [call.args[0] for call in runner.hdc.call_args_list]
            self.assertEqual(sum(argv[0] == 'install' for argv in calls), 3)
            self.assertIn(['shell', 'bm', 'uninstall', '-n', 'app.photocraft', '-m', 'entry_test', '-k'], calls)
            self.assertFalse(any('force-stop' in argv for argv in calls))

    def test_only_owned_started_test_process_can_be_stopped(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            runner.active_pid.side_effect = ['', '', '5678', '', '']
            runner.instrument = Mock()
            artifacts = self.artifacts(temp)
            with patch('test_runner.build', return_value=artifacts):
                runner.device()
            calls = [call.args[0] for call in runner.hdc.call_args_list]
            self.assertIn(['shell', 'aa', 'force-stop', 'app.photocraft'], calls)

    def test_native_interface_gate_refuses_contaminated_ordinary_before_any_install(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            normal = runner.project / 'harmonyos/entry/build/default/outputs/default/entry-default-signed.hap'
            write_hap_fixture(normal, ['testSnapshot'])
            with patch('test_runner.build', return_value=self.artifacts(temp)):
                with self.assertRaisesRegex(TestError, 'Ordinary restoration HAP contains'):
                    runner.device()
            runner.hdc.assert_not_called()
            self.assertTrue(runner.report['hapInterfaceEvidence']['ordinary']['testInterfaceMarkers']['testSnapshot'])

    def test_missing_native_exports_refuses_device_main_before_any_install(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            artifacts = self.artifacts(temp)
            write_hap_fixture(Path(artifacts['default']['path']), ['testSnapshot'])
            with patch('test_runner.build', return_value=artifacts):
                with self.assertRaisesRegex(TestError, 'missing one or more'):
                    runner.device()
            runner.hdc.assert_not_called()

    def test_captured_failure_files_are_received_then_only_owned_paths_removed(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            record = {'suite': 'PhotoCraftCore', 'label': 'document snapshot timeout', 'timestamp': '12345',
                      'screenPath': f'/data/local/tmp/PhotoCraftTest-{runner.run_id}-12345-failure.png',
                      'layoutPath': f'/data/local/tmp/PhotoCraftTest-{runner.run_id}-12345-failure.json'}
            def hdc(argv, **kwargs):
                if argv[:2] == ['file', 'recv']:
                    Path(argv[-1]).write_bytes(b'actual captured evidence fixture')
                return 'success'
            runner.hdc.side_effect = hdc
            transfers = runner.collect_case_failure_evidence('selected', {'records': [record]})
            self.assertEqual([item['status'] for item in transfers], ['saved', 'saved'])
            argv = [call.args[0] for call in runner.hdc.call_args_list]
            self.assertEqual([call[:2] for call in argv], [['file', 'recv'], ['shell', 'rm'], ['file', 'recv'], ['shell', 'rm']])
            self.assertEqual(argv[1], ['shell', 'rm', '-f', record['screenPath']])
            self.assertEqual(argv[3], ['shell', 'rm', '-f', record['layoutPath']])

    def test_receive_without_a_file_is_not_success_and_keeps_remote_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            record = {'suite': 'PhotoCraftCore', 'label': 'failure', 'timestamp': '12345',
                      'screenPath': f'/data/local/tmp/PhotoCraftTest-{runner.run_id}-12345-failure.png'}
            transfers = runner.collect_case_failure_evidence('selected', {'records': [record]})
            self.assertEqual(transfers[0]['status'], 'unavailable')
            self.assertEqual(runner.hdc.call_count, 1)

    def test_host_removes_shell_owned_query_and_verifies_absence_without_receiving_it(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            path = f'/data/local/tmp/PhotoCraftTest-{runner.run_id}-1791437885660-0-query.json'
            shell_owned = {path}
            def hdc(argv, **kwargs):
                if argv[:3] == ['shell', 'rm', '-f']:
                    shell_owned.remove(argv[-1])
                    return ''
                if argv[:3] == ['shell', 'sh', '-c']:
                    return 'PRESENT' if argv[-1] in shell_owned else 'REMOVED'
                self.fail('Unexpected query cleanup command: ' + repr(argv))
            runner.hdc.side_effect = hdc
            result = runner.cleanup_query_files({'records': [{'path': path, 'bundleName': 'app.photocraft'}]})
            self.assertEqual(result[0]['status'], 'removed')
            self.assertEqual(shell_owned, set())
            self.assertFalse(any(call.args[0][0] == 'file' for call in runner.hdc.call_args_list))
            self.assertEqual(runner.hdc.call_args_list[0].args[0], ['shell', 'rm', '-f', path])

    def test_query_cleanup_retains_original_protocol_error_and_reports_cleanup_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            path = f'/data/local/tmp/PhotoCraftTest-{runner.run_id}-1791437885660-0-query.json'
            output = 'PHOTOCRAFT_QUERY_FILE ' + json.dumps({'runId': runner.run_id, 'bundleName': 'app.photocraft', 'path': path})
            def hdc(argv, **kwargs):
                if argv[:3] == ['shell', 'aa', 'test']:
                    return output
                if argv[:3] == ['shell', 'rm', '-f']:
                    return 'rm: Permission denied'
                self.fail('Unexpected command: ' + repr(argv))
            runner.hdc.side_effect = hdc
            with self.assertRaises(TestError):
                runner.instrument_phase('selected', 'PhotoCraftCore#new_edit_undo_redo', 60000)
            metadata = runner.report['queryFilesMetadata'][0]
            self.assertTrue(metadata['cleanupFailed'])
            self.assertEqual(metadata['cleanup'][0]['status'], 'failed')
            defects = runner.report['instrumentResult']['defects']
            self.assertTrue(any('Missing final test result' in defect for defect in defects))
            self.assertTrue(any('Query cleanup failed' in defect for defect in defects))

    def test_query_marker_is_diagnostic_and_successful_cleanup_does_not_fail_a_case(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            path = f'/data/local/tmp/PhotoCraftTest-{runner.run_id}-1791437885660-0-query.json'
            fingerprint = storage_fingerprint()
            output = transcript().replace('class=PhotoCraft', 'class=PhotoCraftCore').replace('test=case0', 'test=new_edit_undo_redo')
            output += '\nPHOTOCRAFT_ISOLATION ' + json.dumps({'runId': runner.run_id, 'suite': 'PhotoCraftCore',
                                                            'before': fingerprint, 'after': fingerprint, 'unchanged': True})
            output += '\nPHOTOCRAFT_QUERY_FILE ' + json.dumps({'runId': runner.run_id, 'bundleName': 'app.photocraft', 'path': path})
            def hdc(argv, **kwargs):
                if argv[:3] == ['shell', 'aa', 'test']:
                    return output
                return 'REMOVED' if argv[:3] == ['shell', 'sh', '-c'] else ''
            runner.hdc.side_effect = hdc
            with patch('test_runner.time.monotonic', side_effect=[20, 22, 25]):
                runner.instrument_phase('selected', 'PhotoCraftCore#new_edit_undo_redo', 60000)
            self.assertTrue(runner.report['instrumentResult']['passed'])
            self.assertFalse(runner.report['queryFilesMetadata'][0]['cleanupFailed'])
            self.assertEqual(runner.report['phases'][0]['durationMs'], 5000)
            self.assertEqual(runner.report['phases'][0]['executionDurationMs'], 2000)
            self.assertEqual(runner.report['phases'][0]['postprocessDurationMs'], 3000)

    def test_manual_comparison_separates_aa_execution_from_query_cleanup_and_evidence_processing(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            runner.report['manualBaseline'] = {'seconds': 60, 'source': 'user-estimate', 'coverageNote': 'Different coverage.'}
            runner.report['phases'] = [{'name': 'aa-instrument-core-files', 'durationMs': 10000,
                                        'executionDurationMs': 4000, 'postprocessDurationMs': 6000}]
            with redirect_stdout(io.StringIO()):
                runner.finish('passed')
            self.assertEqual(runner.report['timings']['aaExecutionMs'], 4000)
            self.assertEqual(runner.report['timings']['devicePostprocessMs'], 6000)
            self.assertEqual(runner.report['comparison']['aaExecutionSeconds'], 4)

    def test_full_run_separates_autosave_seed_from_restart_verification(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            runner.hdc.return_value = '1'
            events = []
            def phase(name, scope, timeout, run_id=None):
                events.append((name, scope, timeout, run_id or runner.run_id))
                cases = [{'suite': suite, 'name': case, 'status': 'passed'} for suite, case in requested_device_cases(scope)]
                result = {'passed': True, 'summary': {'total': len(cases), 'failed': 0, 'errors': 0, 'passed': len(cases), 'skipped': 0}, 'cases': cases, 'defects': []}
                runner.report.setdefault('instrumentResults', []).append(result)
            runner.instrument_phase = Mock(side_effect=phase)
            runner.stop_owned_test_session = Mock(side_effect=lambda name: events.append(('stop', name)))
            runner.instrument()
            self.assertEqual(events[0], ('core-files', 'PhotoCraftCore,PhotoCraftFiles', 60000, runner.run_id))
            self.assertEqual(events[1], ('stop', 'core-to-recovery'))
            self.assertEqual(events[2], ('recovery-seed', 'PhotoCraftRecovery#seed_autosave', 180000, runner.run_id + '_recovery'))
            self.assertEqual(events[3], ('stop', 'recovery-restart'))
            self.assertEqual(events[4], ('recovery-verify', 'PhotoCraftRecovery#verify_autosave', 60000, runner.run_id + '_recovery'))
            self.assertEqual(events[5], ('stop', 'recovery-to-close'))
            self.assertEqual(events[6], ('close', 'PhotoCraftClose', 60000, runner.run_id + '_close'))
            self.assertEqual(runner.report['deviceAcceptance'], 'full')

    def test_scoped_autosave_seed_is_stage_only(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            runner.args.scope = 'PhotoCraftRecovery#seed_autosave'
            runner.hdc.return_value = '1'
            result = {'passed': True, 'summary': {'total': 1, 'failed': 0, 'errors': 0, 'passed': 1, 'skipped': 0}, 'cases': [], 'defects': []}
            runner.instrument_phase = Mock(side_effect=lambda *args: runner.report.update(instrumentResults=[result]))
            runner.stop_owned_test_session = Mock()
            runner.instrument()
            self.assertEqual(runner.report['deviceAcceptance'], 'stage-only')
            runner.stop_owned_test_session.assert_not_called()

    def test_unavailable_testmode_is_left_unchanged_for_real_ui_assertions(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            runner.args.scope = 'PhotoCraftCore#new_edit_undo_redo'
            property_value = ['Get parameter "persist.ace.testmode.enabled" fail! errNum is:1002!']
            def hdc(argv, **kwargs):
                if argv[:3] == ['shell', 'param', 'get']:
                    return property_value[0]
                if argv[:3] == ['shell', 'param', 'set']:
                    property_value[0] = argv[-1]
                    return 'success'
                return ''
            runner.hdc.side_effect = hdc
            result = {'passed': True, 'summary': {'total': 1, 'failed': 0, 'errors': 0, 'passed': 1, 'skipped': 0}, 'cases': [], 'defects': []}
            runner.instrument_phase = Mock(side_effect=lambda *args: runner.report.update(instrumentResults=[result]))
            runner.instrument()
            self.assertIsNone(runner.report['device']['testmodeBefore'])
            self.assertIs(runner.report['device']['testmodeReadable'], False)
            self.assertIsNone(runner.report['device']['testmodeAfter'])
            self.assertEqual(runner.report['device']['testmodeRestoration'], 'unchanged (parameter unavailable; no writes)')
            setters = [call.args[0][-1] for call in runner.hdc.call_args_list if call.args[0][:3] == ['shell', 'param', 'set']]
            self.assertEqual(setters, [])

    def test_failed_storage_assertion_captures_historical_pid_after_process_exit(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            fingerprint = storage_fingerprint(entries=1, size=12)
            evidence = {'runId': runner.run_id, 'suite': 'PhotoCraftCore', 'before': fingerprint,
                        'after': copy.deepcopy(fingerprint), 'unchanged': False,
                        'differences': [{'relativePath': 'files/hiappevent/event', 'beforeBytes': 12, 'afterBytes': 13}]}
            evidence['after']['bytes'] = 13
            evidence['after']['roots'][0]['bytes'] = 13
            output = transcript().replace('class=PhotoCraft', 'class=PhotoCraftCore').replace('test=case0', 'test=new_edit_undo_redo')
            output += '\nPHOTOCRAFT_RUNTIME ' + json.dumps({'runId': runner.run_id, 'suite': 'PhotoCraftCore', 'pid': 43490})
            output += '\nPHOTOCRAFT_ISOLATION ' + json.dumps(evidence)
            runner.hdc.return_value = output
            with self.assertRaisesRegex(TestError, 'Ordinary storage root mapping or contents changed'):
                runner.instrument_phase('selected', 'PhotoCraftCore#new_edit_undo_redo', 60000)
            self.assertEqual(runner.report['ordinaryStorageEvidence'][0]['evidence'], [evidence])
            runner.hdc.return_value = 'historical app log'
            runner.collect_hilog('failure')
            self.assertEqual((runner.report_dir / 'hilog-failure-43490.txt').read_text(), 'historical app log')
            runner.hdc.assert_called_with(['shell', 'hilog', '-x', '-P', '43490', '-T', 'PhotoCraft'], timeout=15)

    def test_transport_timeout_still_records_the_attempted_aa_phase_and_partial_pid(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            runner.save('timeout-output.log', 'PHOTOCRAFT_RUNTIME ' + json.dumps(
                {'runId': runner.run_id, 'suite': 'PhotoCraftCore', 'pid': 43490}))
            runner.hdc.side_effect = TestError('hdc timed out after 660s.')
            with self.assertRaisesRegex(TestError, 'Instrument transport failed'):
                runner.instrument_phase('selected', 'PhotoCraftCore#new_edit_undo_redo', 60000)
            self.assertEqual(runner.report['phases'][0]['status'], 'failed')
            self.assertEqual(runner.report['deviceExecution'], 'executed')
            self.assertEqual(runner.report['runtimeEvidence'][0]['records'][0]['pid'], 43490)
            self.assertFalse(runner.report['instrumentResult']['passed'])
            with redirect_stdout(io.StringIO()):
                runner.finish('failed')
            self.assertEqual(runner.report['timings']['aaExecutionStatus'], 'failed')

    def test_manual_estimate_does_not_report_speed_when_no_aa_phase_executed(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            runner.report['manualBaseline'] = {'seconds': 60, 'source': 'user-estimate', 'coverageNote': 'Different coverage.'}
            with redirect_stdout(io.StringIO()):
                runner.finish('failed', 'build fixture failed')
            self.assertEqual(runner.report['comparison']['executionStatus'], 'not-executed')
            self.assertIsNone(runner.report['comparison']['aaExecutionSeconds'])
            self.assertIsNone(runner.report['comparison']['wholeRunDifferenceFromEstimateSeconds'])

    def test_manual_estimate_separates_execution_and_overall_duration(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = self.setup_runner(temp)
            runner.report['manualBaseline'] = {'seconds': 60, 'source': 'user-estimate', 'coverageNote': 'Different coverage.'}
            runner.report['buildDurationMs'] = 20000
            runner.report['phases'] = [{'name': 'aa-instrument-core-files', 'durationMs': 3500}]
            runner.started -= 30
            with redirect_stdout(io.StringIO()):
                runner.finish('passed')
            self.assertEqual(runner.report['timings']['buildMs'], 20000)
            self.assertEqual(runner.report['comparison']['aaExecutionSeconds'], 3.5)
            self.assertGreaterEqual(runner.report['comparison']['wholeRunSeconds'], 30)
            self.assertEqual(runner.report['comparison']['baselineSource'], 'user-estimate')


class ReuseBuildTests(unittest.TestCase):
    def make_artifacts(self, temp):
        import hashlib
        project = Path(temp) / 'project'
        (project / 'apps/editor/src').mkdir(parents=True)
        (project / 'apps/editor/src/lib.rs').write_text('fn original() {}')
        artifacts = {'bundle': 'app.photocraft', 'signed': True, 'buildMode': 'debug-device-tests',
                     'sourceFingerprint': source_fingerprint(project)}
        for target in ('default', 'ohosTest'):
            hap = Path(temp) / f'{target}-signed.hap'
            hap.write_bytes(target.encode())
            artifacts[target] = {'path': str(hap), 'sha256': hashlib.sha256(hap.read_bytes()).hexdigest()}
        metadata = Path(temp) / 'build-artifacts.json'
        metadata.write_text(json.dumps(artifacts))
        return project, metadata, artifacts

    def test_signed_build_reuse_requires_current_sources_and_matching_hap_hash(self):
        with tempfile.TemporaryDirectory() as temp:
            project, metadata, artifacts = self.make_artifacts(temp)
            self.assertEqual(reuse_artifacts(metadata, project, 'app.photocraft'), artifacts)
            (project / 'apps/editor/src/lib.rs').write_text('fn changed() {}')
            with self.assertRaisesRegex(BuildError, 'stale'):
                reuse_artifacts(metadata, project, 'app.photocraft')
        with tempfile.TemporaryDirectory() as temp:
            project, metadata, artifacts = self.make_artifacts(temp)
            Path(artifacts['default']['path']).write_bytes(b'changed HAP')
            with self.assertRaisesRegex(BuildError, 'SHA256'):
                reuse_artifacts(metadata, project, 'app.photocraft')

    def test_unsigned_or_wrong_bundle_cannot_be_reused(self):
        with tempfile.TemporaryDirectory() as temp:
            project, metadata, artifacts = self.make_artifacts(temp)
            with self.assertRaisesRegex(BuildError, 'signed device-tests'):
                reuse_artifacts(metadata, project, 'app.other')
            artifacts['signed'] = False
            metadata.write_text(json.dumps(artifacts))
            with self.assertRaisesRegex(BuildError, 'signed device-tests'):
                reuse_artifacts(metadata, project, 'app.photocraft')


class CampaignTests(unittest.TestCase):
    def run_campaign(self, temp, *, fail_at=None, acceptance='full', baseline=None):
        created = []
        def factory(args):
            index = len(created) + 1
            runner = Mock()
            runner.run_id = f'run{index}'
            runner.report_dir = Path(temp) / runner.run_id
            runner.report_dir.mkdir()
            runner.report = {}
            runner.args = args
            def device():
                if index == fail_at:
                    raise TestError('real assertion failure fixture')
                runner.report['deviceAcceptance'] = acceptance
            runner.device.side_effect = device
            def finish(status, error=''):
                runner.report.update(status=status, durationMs=100, error=error)
                (runner.report_dir / 'report.json').write_text(json.dumps(runner.report))
            runner.finish.side_effect = finish
            created.append(runner)
            return runner
        argv = ['--project', temp, 'device', '--repeat', '3']
        if baseline is not None:
            argv += ['--manual-baseline-seconds', str(baseline)]
        with patch('test_runner.Runner', side_effect=factory), redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            result = main(argv)
        return result, created, json.loads((Path(temp) / 'run1/campaign.json').read_text())

    def test_three_full_runs_reuse_first_signed_build_and_accept(self):
        with tempfile.TemporaryDirectory() as temp:
            result, runners, campaign = self.run_campaign(temp)
            self.assertEqual(result, 0)
            self.assertEqual(len(runners), 3)
            self.assertIsNone(runners[0].args.reuse_build)
            self.assertEqual(runners[1].args.reuse_build, str(Path(temp) / 'run1/report.json'))
            self.assertEqual(runners[2].args.reuse_build, runners[1].args.reuse_build)
            self.assertIs(campaign['fullDeviceAcceptance'], True)

    def test_campaign_stops_on_failure_and_partial_runs_never_accept(self):
        with tempfile.TemporaryDirectory() as temp:
            result, runners, campaign = self.run_campaign(temp, fail_at=2)
            self.assertEqual(result, 1)
            self.assertEqual(len(runners), 2)
            self.assertIs(campaign['allPassed'], False)
            self.assertIs(campaign['fullDeviceAcceptance'], False)
        with tempfile.TemporaryDirectory() as temp:
            result, runners, campaign = self.run_campaign(temp, acceptance='partial')
            self.assertEqual(result, 0)
            self.assertIs(campaign['allPassed'], True)
            self.assertIs(campaign['fullDeviceAcceptance'], False)

    def test_failed_campaign_before_aa_has_no_speed_comparison(self):
        with tempfile.TemporaryDirectory() as temp:
            result, runners, campaign = self.run_campaign(temp, fail_at=1, baseline=60)
            self.assertEqual(result, 1)
            self.assertEqual(campaign['comparison']['executedRuns'], 0)
            self.assertIsNone(campaign['comparison']['aaDifferenceFromManualEstimateSeconds'])
            self.assertIsNone(campaign['comparison']['wholeRunDifferenceFromManualEstimateSeconds'])


class AllScopeTests(unittest.TestCase):
    def test_device_filter_does_not_filter_any_host_regressions(self):
        with tempfile.TemporaryDirectory() as temp:
            args = argparse.Namespace(project=temp, command='all', scope='PhotoCraftCore#new_edit_undo_redo', device=None)
            runner = Runner(args)
            runner.package = 'fixture-app'
            runner.host_packages = {'app': 'local-app', 'platform': 'local-platform', 'facade': None}
            runner.env['CRAFT_NODE'] = 'node-fixture'
            runner.phase = Mock()
            fixture = Path(temp) / 'tests/publication_transaction.test.cjs'
            with patch('test_runner.node_test_files', return_value=[fixture]):
                runner.host()
            calls = {call.args[0]: call.args[1] for call in runner.phase.call_args_list}
            self.assertEqual(set(calls), {'rust-host', 'node-host'})
            self.assertNotIn('new_edit_undo_redo', calls['rust-host'])
            self.assertNotIn('--test-name-pattern', calls['node-host'])
            self.assertEqual(runner.args.scope, 'PhotoCraftCore#new_edit_undo_redo')
            self.assertEqual(runner.suite, 'PhotoCraftCore')
            self.assertIsNone(runner.report['hostConfiguration']['scope'])

    def test_all_rejects_host_scopes_before_creating_any_runner(self):
        for scope in ('Rust', 'Unit#case', 'Node', 'Platform'):
            with self.subTest(scope=scope), patch('test_runner.Runner') as runner, redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    main(['--project', '/fixture', 'all', '--scope', scope])
                self.assertEqual(error.exception.code, 2)
                runner.assert_not_called()

    def test_device_unknown_registered_case_is_rejected_before_build_or_install(self):
        for command in ('device', 'all'):
            with self.subTest(command=command), patch('test_runner.Runner') as runner, redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    main(['--project', '/fixture', command, '--scope', 'PhotoCraftCore#missing_case'])
                self.assertEqual(error.exception.code, 2)
                runner.assert_not_called()


if __name__ == '__main__':
    unittest.main()
