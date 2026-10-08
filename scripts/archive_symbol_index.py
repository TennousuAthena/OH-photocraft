#!/usr/bin/env python3
"""Read a GNU ar symbol index without interpreting Rust LLVM bitcode members."""
from __future__ import annotations

import argparse
from pathlib import Path
import struct
import sys


def indexed_symbols(archive: Path) -> set[bytes]:
    with archive.open('rb') as stream:
        if stream.read(8) != b'!<arch>\n':
            raise ValueError('Not a regular ar archive.')
        while True:
            header = stream.read(60)
            if not header:
                raise ValueError('Archive has no GNU symbol index.')
            if len(header) != 60 or header[58:] != b'`\n':
                raise ValueError('Invalid archive member header.')
            try:
                size = int(header[48:58].strip())
            except ValueError:
                raise ValueError('Invalid archive member size.') from None
            if size < 0:
                raise ValueError('Negative archive member size.')
            name = header[:16].strip()
            if name in (b'/', b'/SYM64/'):
                if size > 64 * 1024 * 1024:
                    raise ValueError('Archive symbol index exceeds 64 MiB.')
                data = stream.read(size)
                width = 4 if name == b'/' else 8
                if len(data) != size or size < width:
                    raise ValueError('Truncated archive symbol index.')
                count = struct.unpack('>I' if width == 4 else '>Q', data[:width])[0]
                start = width * (count + 1)
                if start > len(data):
                    raise ValueError('Invalid archive symbol offsets.')
                names = data[start:].split(b'\0')
                if len(names) < count + 1 or any(not item for item in names[:count]):
                    raise ValueError('Invalid archive symbol names.')
                return set(names[:count])
            # Skip object members. Their bitcode version is irrelevant to the index.
            stream.seek(size + (size % 2), 1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    parser.add_argument('--contains', required=True)
    args = parser.parse_args()
    try:
        symbol = args.contains.encode('ascii')
        if not symbol or b'\0' in symbol:
            raise ValueError('Invalid symbol name.')
        print('present' if symbol in indexed_symbols(args.archive) else 'absent')
        return 0
    except (OSError, ValueError, UnicodeEncodeError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
