import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from stream_process import StreamError, stream_process


class StreamingProcessTests(unittest.TestCase):
    def run_child(self, source, temp, callback, **options):
        return stream_process([sys.executable, '-u', '-c', source], cwd=Path(temp), env=os.environ.copy(),
            output_path=Path(temp) / 'instrument.txt', on_line=callback, timeout=options.pop('timeout', 3), **options)

    def test_callback_is_serviced_while_child_is_waiting_and_raw_output_is_preserved(self):
        with tempfile.TemporaryDirectory() as temp:
            ack = Path(temp) / 'owned-reply.json'
            marker = 'PHOTOCRAFT_LAYOUT_REQUEST ' + json.dumps({'ticket': 'layout-1'})
            source = f'''import pathlib,time,json
print({marker!r},flush=True)
path=pathlib.Path({str(ack)!r})
deadline=time.monotonic()+2
while not path.exists() and time.monotonic()<deadline: time.sleep(0.01)
assert json.loads(path.read_text()) == {{'ok': True, 'ticket': 'layout-1'}}
print('actual callback acknowledged',flush=True)
'''
            lines = []
            def callback(line):
                lines.append(line)
                if line.startswith('PHOTOCRAFT_LAYOUT_REQUEST '):
                    ack.write_text(json.dumps({'ok': True, 'ticket': 'layout-1'}))
            result = self.run_child(source, temp, callback)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(lines, [marker, 'actual callback acknowledged'])
            self.assertEqual((Path(temp) / 'instrument.txt').read_text(), result.stdout)

    def test_timeout_with_partial_unterminated_line_preserves_evidence_and_kills_child(self):
        with tempfile.TemporaryDirectory() as temp:
            with self.assertRaises(StreamError) as caught:
                self.run_child("import os,time; os.write(1,b'partial PID evidence'); time.sleep(10)", temp,
                               lambda line: None, timeout=0.1)
            self.assertIn('timed out', str(caught.exception))
            self.assertEqual(caught.exception.stdout, 'partial PID evidence')
            self.assertEqual((Path(temp) / 'instrument.txt').read_text(), caught.exception.stdout)

    def test_fragmented_utf8_lines_and_last_line_are_delivered_once(self):
        with tempfile.TemporaryDirectory() as temp:
            lines = []
            source = "import os,time; payload='中文😀\\nlast'.encode(); os.write(1,payload[:2]); time.sleep(.01); os.write(1,payload[2:])"
            result = self.run_child(source, temp, lines.append)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(lines, ['中文😀', 'last'])

    def test_oversized_line_output_and_callback_failure_fail_without_hanging(self):
        with tempfile.TemporaryDirectory() as temp:
            for options in ({'line_limit': 4}, {'output_limit': 4}):
                with self.subTest(options=options), self.assertRaises(StreamError):
                    self.run_child("print('123456789')", temp, lambda line: None, **options)
            def rejected(line):
                raise ValueError('Malformed layout callback')
            with self.assertRaisesRegex(StreamError, 'Malformed layout callback'):
                self.run_child("import time; print('request',flush=True); time.sleep(10)", temp, rejected)


if __name__ == '__main__':
    unittest.main()
