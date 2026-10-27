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

    def test_settings_are_native_disclosures_and_intelligence_has_no_visible_selects(self):
        sections=[a for t,a in self.markup.tags if t=='details' and 'settings-section' in a.get('class','').split()]
        self.assertEqual(len(sections),9)
        self.assertTrue(all('open' not in a for a in sections))
        for ident in ('model','effort'):
            control=next(a for t,a in self.markup.tags if a.get('id')==ident)
            self.assertIn('hidden',control)
            self.assertEqual(control['aria-hidden'],'true')
        title=next(a for t,a in self.markup.tags if a.get('id')=='intelligence-toggle')
        self.assertEqual(title['aria-controls'],'intelligence-picker')

    def test_canvas_focus_never_draws_a_rectangular_frame(self):
        css=(ROOT/'style.css').read_text()
        rules=re.findall(r'canvas:focus-visible\{([^}]+)',css)
        self.assertTrue(rules)
        for rule in rules:
            self.assertIn('outline:none',rule)
            self.assertIn('box-shadow:none',rule)
        self.assertTrue(any(a.get('id')=='panel-drag' for _,a in self.markup.tags))

    def test_voice_icons_share_the_same_coordinate_system_and_size(self):
        class Icons(HTMLParser):
            def __init__(self,text):
                super().__init__();self.button=None;self.icons={};self.feed(text)
            def handle_starttag(self,tag,attrs):
                attrs=dict(attrs)
                if tag=='button':self.button=attrs.get('id')
                if tag=='svg' and self.button in ('voice-toggle','preview-voice'):
                    self.icons[self.button]=(attrs.get('viewbox'),attrs.get('width'),attrs.get('height'))
            def handle_endtag(self,tag):
                if tag=='button':self.button=None
        icons=Icons((ROOT/'index.html').read_text()).icons
        self.assertEqual(icons['voice-toggle'],('0 0 24 24','16','16'))
        self.assertEqual(icons['voice-toggle'],icons['preview-voice'])
