#!/usr/bin/env python3
"""Dependency-free structural and local-asset checks, not a browser audit."""
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urlsplit
root = Path(__file__).resolve().parents[1]
class Site(HTMLParser):
    def __init__(self):
        super().__init__(); self.ids = set(); self.refs = []; self.lang = False; self.main = False
    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if 'id' in attrs:
            assert attrs['id'] not in self.ids, f"duplicate id: {attrs['id']}"
            self.ids.add(attrs['id'])
        if tag == 'html': self.lang = attrs.get('lang') == 'en'
        if tag == 'main': self.main = True
        if tag == 'img': assert 'alt' in attrs, 'image missing alternative text'
        if tag == 'script': raise AssertionError('site must remain script-free')
        for key in ('href', 'src', 'poster'): 
            if key in attrs: self.refs.append(attrs[key])
s = Site(); s.feed((root / 'docs/index.html').read_text())
assert s.lang and s.main
for ref in s.refs:
    url = urlsplit(ref)
    if url.scheme or url.netloc: continue
    if url.path:
        assert not url.path.startswith('/'), f'root-relative path breaks project Pages: {ref}'
        assert (root / 'docs' / url.path).is_file(), f'missing local asset: {ref}'
    elif url.fragment: assert url.fragment in s.ids, f'missing anchor: {ref}'
assert (root / 'docs/.nojekyll').is_file()
print(f'Site structure and {len(s.refs)} references checked; native browser/media acceptance remains separate.')
