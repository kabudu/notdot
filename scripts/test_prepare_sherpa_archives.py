import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from prepare_sherpa_archives import prepare

class NativeArchiveAcceptance(unittest.TestCase):
    def fixture(self,root):
        name='sherpa-onnx-v2.0.0-linux-x64-static-lib.tar.bz2'
        payload=b'verified public fixture'
        manifest=root/'manifest.json'
        manifest.write_text(json.dumps({'version':'2.0.0','targets':{'test':{'archive':name,'bytes':len(payload),'sha256':hashlib.sha256(payload).hexdigest()}}}))
        return name,payload,manifest
    def test_verified_cache_is_shared_without_another_download(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);name,payload,manifest=self.fixture(root);calls=[]
            def fetch(url,limit):calls.append((url,limit));return payload
            prepare('test',root/'cache',manifest,fetch)
            prepare('test',root/'cache',manifest,fetch)
            self.assertEqual((root/'cache'/name).read_bytes(),payload)
            self.assertEqual(len(calls),1)
    def test_bad_bytes_are_rejected_before_cargo_can_reuse_them(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);name,payload,manifest=self.fixture(root)
            with self.assertRaisesRegex(ValueError,'SHA-256'):
                prepare('test',root/'cache',manifest,lambda url,limit:b'x'*len(payload))
            self.assertFalse((root/'cache'/name).exists())
    def test_corrupt_existing_archive_is_replaced_only_after_verification(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);name,payload,manifest=self.fixture(root);(root/'cache').mkdir();(root/'cache'/name).write_bytes(b'partial')
            prepare('test',root/'cache',manifest,lambda url,limit:payload)
            self.assertEqual((root/'cache'/name).read_bytes(),payload)
    def test_unknown_target_does_not_create_cache(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);_,_,manifest=self.fixture(root)
            with self.assertRaises(KeyError):prepare('unknown',root/'cache',manifest)
            self.assertFalse((root/'cache').exists())
