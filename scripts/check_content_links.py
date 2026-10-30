#!/usr/bin/env python3
"""Check inline repository-local Markdown file links; external URLs/anchors are separate."""
from pathlib import Path
import re
from urllib.parse import unquote, urlsplit
from ci_changes import ROOT_CONTENT


def check_links(root):
    count = 0
    for source in [*(root / name for name in sorted(ROOT_CONTENT)), *sorted((root / 'docs').rglob('*.md'))]:
        if not source.is_file():
            continue
        text = source.read_text(encoding='utf-8')
        text = re.sub(r'```.*?```|~~~.*?~~~', '', text, flags=re.S)
        text = re.sub(r'`[^`\n]*`', '', text)
        for raw in re.findall(r'\]\(\s*(<[^>\n]+>|[^\s()]+)(?:\s+["\'][^\n]*["\'])?\s*\)', text):
            url = urlsplit(raw.strip('<>'))
            if url.scheme or url.netloc or not url.path:
                continue
            target = (source.parent / unquote(url.path)).resolve()
            if not target.is_relative_to(root) or not target.exists():
                raise ValueError(f'Missing or nonportable local link in {source.relative_to(root)}: {raw}')
            count += 1
    return count


if __name__ == '__main__':
    count = check_links(Path(__file__).resolve().parents[1])
    print(f'Checked {count} local Markdown file links.')
