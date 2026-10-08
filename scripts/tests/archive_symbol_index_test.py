import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('archive_symbol_index', Path(__file__).parents[1] / 'archive_symbol_index.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def member(name, data):
    header = name.ljust(16) + b'0'.ljust(12) + b'0'.ljust(6) + b'0'.ljust(6) + b'644'.ljust(8) + str(len(data)).encode().ljust(10) + b'`\n'
    return header + data + (b'\n' if len(data) % 2 else b'')


class ArchiveIndexTests(unittest.TestCase):
    def read(self, data):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / 'native.a'
            archive.write_bytes(data)
            return module.indexed_symbols(archive)

    def test_gnu_index_does_not_read_incompatible_bitcode_or_false_marker(self):
        data = struct.pack('>II', 1, 100) + b'craft_initialize\0'
        archive = b'!<arch>\n' + member(b'/', data) + member(b'app.o/', b'LLVM999 craft_device_tests_abi_v1\0')
        self.assertEqual(self.read(archive), {b'craft_initialize'})

    def test_64_bit_index_finds_feature_marker(self):
        data = struct.pack('>QQQ', 2, 100, 200) + b'craft_initialize\0craft_device_tests_abi_v1\0'
        self.assertIn(b'craft_device_tests_abi_v1', self.read(b'!<arch>\n' + member(b'/SYM64/', data)))

    def test_missing_or_corrupt_indices_fail(self):
        for archive in (b'not an archive', b'!<arch>\n', b'!<arch>\n' + member(b'/', struct.pack('>I', 99))):
            with self.subTest(archive=archive):
                with self.assertRaises(ValueError):
                    self.read(archive)
