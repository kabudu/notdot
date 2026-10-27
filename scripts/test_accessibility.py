"""Static accessibility contracts; not a substitute for VoiceOver/browser QA."""
import colorsys
from html.parser import HTMLParser
from pathlib import Path
import re
import unittest
ROOT = Path(__file__).resolve().parents[1]
class Markup(HTMLParser):
    def __init__(self, text):
        super().__init__(); self.tags=[]; self.feed(text)
    def handle_starttag(self, tag, attrs):
        self.tags.append((tag, dict(attrs)))
class Accessibility(unittest.TestCase):
    def setUp(self):
        self.markup=Markup((ROOT/'index.html').read_text())
    def test_labels_references_are_unique_and_resolve_and_dialogs_have_names(self):
        ids=[a['id'] for _,a in self.markup.tags if 'id'in a]
        self.assertEqual(len(ids),len(set(ids)))
        for tag,a in self.markup.tags:
            if tag=='dialog':self.assertIn('aria-labelledby',a)
            for attr in ('aria-controls','aria-labelledby','aria-describedby'):
                for ref in a.get(attr,'').split():self.assertIn(ref,ids,(a.get('id'),attr,ref))
        orb=next(a for _,a in self.markup.tags if a.get('id')=='orb')
        self.assertEqual(orb['role'],'button');self.assertIn('Enter',orb['aria-keyshortcuts'])
        status=next(a for _,a in self.markup.tags if a.get('id')=='orb-status')
        self.assertEqual(status['aria-live'],'polite')
    def test_normal_save_text_contrast_exceeds_wcag_aa_for_every_theme_hue(self):
        css=(ROOT/'style.css').read_text()
        rule=re.search(r'\.settings-footer \.primary-action\{([^}]+)',css)[1]
        saturation,lightness=map(lambda s:float(s)/100,re.search(r'background:hsl\(var\(--hue\),(\d+)%,(\d+)%\)',rule).groups())
        colour=re.search(r':root\{[^}]*color:#([0-9a-f]{6})',css)[1]
        fg=tuple(int(colour[i:i+2],16)/255 for i in (0,2,4))
        def luminance(rgb):
            return sum(w*(v/12.92 if v<=.04045 else ((v+.055)/1.055)**2.4) for v,w in zip(rgb,(.2126,.7152,.0722)))
        for hue in range(361):
            a,b=sorted((luminance(fg),luminance(colorsys.hls_to_rgb(hue/360,lightness,saturation))))
            self.assertGreaterEqual((b+.05)/(a+.05),4.5,f'hue {hue}')
    def test_switch_knob_is_distinguishable_from_selected_track_for_all_hues(self):
        css=(ROOT/'style.css').read_text()
        rule=re.search(r'\.switch-row input:checked\+\.switch-track\{([^}]+)',css)[1]
        sat,light=map(lambda s:float(s)/100,re.search(r'background:hsl\(var\(--hue\),(\d+)%,(\d+)%\)',rule).groups())
        knob=re.search(r'\.switch-track::after\{[^}]*background:#([0-9a-f]{6})',css)[1]
        rgb=tuple(int(knob[i:i+2],16)/255 for i in (0,2,4))
        def lum(c):return sum(w*(v/12.92 if v<=.04045 else ((v+.055)/1.055)**2.4) for v,w in zip(c,(.2126,.7152,.0722)))
        for hue in range(361):
            a,b=sorted((lum(rgb),lum(colorsys.hls_to_rgb(hue/360,light,sat))))
            self.assertGreaterEqual((b+.05)/(a+.05),3.0,f'hue {hue}')
    def test_production_markup_has_no_conversation_composer(self):
        self.assertFalse(any(t=='textarea' or a.get('id')in ('composer','text','reply') for t,a in self.markup.tags))
        self.assertIn('prefers-reduced-motion',(ROOT/'style.css').read_text())
