import unittest
from check_site import Site

class SiteRendererPolicy(unittest.TestCase):
    def test_only_the_deferred_local_renderer_is_allowed(self):
        Site().feed('<script src="site-assets/orb.js" defer></script>')
        for markup in [
            '<script src="https://example.test/analytics.js" defer></script>',
            '<script>sendData()</script>',
            '<script src="site-assets/orb.js"></script>',
            '<script src="site-assets/orb.js" defer>sendData()</script>',
            '<button onclick="sendData()">Open</button>',
            '<script src="site-assets/orb.js" defer></script><script src="site-assets/orb.js" defer></script>',
        ]:
            with self.subTest(markup=markup), self.assertRaises(AssertionError):
                Site().feed(markup)
