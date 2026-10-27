# Public launch checklist

Preparation does not change repository visibility or enable automation. The current branch must be reviewed and merged before publishing from `master`.

- [x] Apache-2.0 first-party licence and package metadata; retain third-party notices.
- [x] Contributor, conduct and security policies; concise README and static site source.
- [ ] Re-record the real voice exchange in a quiet environment, lightly clean the audio, then add captions, transcript and website/README embeds (see demo.md).
- [ ] Enable and verify GitHub private vulnerability reporting once the repository is public; current private-repo API returns 404. Replace the interim reporting instructions.
- [ ] Review full Git history and tracked assets for secrets, personal data and redistribution rights.
- [ ] Complete per-platform binary licence inventories and existing native acceptance/release gates before binary publication.
- [ ] Explicit owner decision to make the repository public and enable the intended Actions workflows.
- [ ] Configure Pages to deploy from `master` / `/docs`, then verify https://kabudu.github.io/notdot/ including mobile, media, captions and links. Pages deployment itself uses Actions infrastructure, even for branch publication.
- [ ] Set the repository homepage to the verified live site; verify README metadata and all links from the public default branch.

## Preview the website locally

Run `python3 -m http.server 8080 --directory docs` from the repository and open http://localhost:8080/. There is no Node dependency, build step, third-party JavaScript, analytics or remote font. The site uses relative asset paths so the `/notdot/` Pages prefix works. Original SVG artwork is Apache-2.0.

Run `python3 scripts/check_site.py` for structural/local-asset checks. This does not replace browser, keyboard, screen-reader or actual media checks.

Pages configuration reference: https://docs.github.com/en/pages/getting-started-with-github-pages/configuring-a-publishing-source-for-your-github-pages-site
