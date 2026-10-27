"""Check the exact, documented Linux safety backport, not an audit suppression."""
import hashlib
import json
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
class VendorProvenance(unittest.TestCase):
    def test_glib_matches_upstream_except_documented_ffi_output_pointer_fix(self):
        root = ROOT / 'vendor/glib'
        provenance = json.loads((root / 'UPSTREAM.json').read_text())
        for name, original in provenance['upstream_files'].items():
            expected = provenance['patched_files'].get(name, original)
            self.assertEqual(hashlib.sha256((root / name).read_bytes()).hexdigest(), expected, name)
        changed = set(provenance['patched_files'])
        self.assertEqual(changed, {'src/variant_iter.rs'})
        patched = (root / 'src/variant_iter.rs').read_text()
        self.assertIn('let mut p: *mut libc::c_char', patched)
        self.assertIn('&mut p,', patched)
        self.assertIn('glib = { path = "vendor/glib" }', (ROOT / 'Cargo.toml').read_text())
