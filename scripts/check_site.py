#!/usr/bin/env python3
"""Dependency-free structural/media checks; browser and audio checks remain separate."""
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urlsplit
import re
root = Path(__file__).resolve().parents[1]
class Site(HTMLParser):
    def __init__(self):
        super().__init__(); self.ids=set(); self.refs=[]; self.lang=False; self.main=False; self.videos=[]; self.tracks=[]; self.in_script=False; self.scripts=0
    def handle_starttag(self, tag, attrs):
        attrs=dict(attrs)
        if 'id' in attrs:
            assert attrs['id'] not in self.ids, f"duplicate id: {attrs['id']}"
            self.ids.add(attrs['id'])
        if tag=='html': self.lang=attrs.get('lang')=='en'
        if tag=='main': self.main=True
        if tag=='img': assert 'alt' in attrs, 'image missing alternative text'
        assert not any(key.startswith('on') for key in attrs), 'inline event handlers are forbidden'
        if tag=='script':
            assert attrs.get('src')=='site-assets/orb.js' and 'defer' in attrs, 'only the deferred, local orb renderer is allowed'
            assert set(attrs)<= {'src','defer'}, 'unexpected script attributes'
            self.scripts+=1
            assert self.scripts<=1, 'duplicate renderer script'
            self.in_script=True
        if tag=='video':
            assert 'controls' in attrs and 'playsinline' in attrs
            assert 'autoplay' not in attrs and 'muted' not in attrs
            assert attrs.get('preload')=='metadata'
            self.videos.append(attrs)
        if tag=='track': self.tracks.append(attrs)
        for key in ('href','src','poster'):
            if key in attrs:self.refs.append(attrs[key])
    def handle_endtag(self, tag):
        if tag=='script': self.in_script=False
    def handle_data(self, data):
        if self.in_script: assert not data.strip(), 'inline scripts are forbidden'

def main():
    count=0
    for source in (root/'docs/index.html',):
        s=Site();s.feed(source.read_text());assert s.lang and s.main
        for ref in s.refs:
            url=urlsplit(ref)
            if url.scheme or url.netloc:continue
            if url.path:
                assert not url.path.startswith('/'), f'root-relative project Pages link: {ref}'
                target=(source.parent/url.path).resolve()
                assert target.is_relative_to(root/'docs'), f'asset outside Pages folder: {ref}'
                assert target.is_file(), f'missing asset: {ref}'
            elif url.fragment:assert url.fragment in s.ids, f'missing anchor: {ref}'
        count+=len(s.refs)
    assert (root/'docs/.nojekyll').is_file()
    print(f'Site structure and {count} references checked; browser/audio checks remain separate.')

if __name__=="__main__": main()
