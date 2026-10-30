#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('collector', Path(__file__).with_name('collect_linux_distribution.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def elf(text=b'code', dynamic=b'oldpath', build_id=b'build-id'):
    names = b'\0.text\0.dynamic\0.note.gnu.build-id\0.shstrtab\0'
    values = [(1, 1, 6, text), (7, 6, 3, dynamic), (16, 7, 2, build_id), (35, 3, 0, names)]
    header = bytearray(64)
    header[:6] = b'\x7fELF\x02\x01'
    sections = [bytes(64)]
    payload = bytearray()
    for name, kind, flags, data in values:
        sections.append(struct.pack('<IIQQQQIIQQ', name, kind, flags, 0, 64 + len(payload), len(data), 0, 0, 1, 0))
        payload.extend(data)
    struct.pack_into('<Q', header, 40, 64 + len(payload))
    struct.pack_into('<HHH', header, 58, 64, 5, 4)
    return bytes(header) + payload + b''.join(sections)


class Sections(unittest.TestCase):
    def compare(self, a, b):
        with tempfile.TemporaryDirectory() as directory:
            left, right = Path(directory) / 'a', Path(directory) / 'b'
            left.write_bytes(a)
            right.write_bytes(b)
            return module.load_sections(left), module.load_sections(right)

    def test_rpath_table_change_is_explicitly_tolerated(self):
        a, b = self.compare(elf(), elf(dynamic=b'new-long-path'))
        self.assertIsNotNone(a)
        self.assertEqual(a, b)

    def test_changed_program_code_rejected(self):
        a, b = self.compare(elf(), elf(text=b'evil'))
        self.assertNotEqual(a, b)

    def test_changed_build_id_rejected(self):
        a, b = self.compare(elf(), elf(build_id=b'other-id'))
        self.assertNotEqual(a, b)

    def test_non_elf_is_not_provenance(self):
        a, _ = self.compare(b'not ELF', elf())
        self.assertIsNone(a)


if __name__ == '__main__':
    unittest.main()
