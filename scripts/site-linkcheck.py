#!/usr/bin/env python3
"""Check a built mdBook for broken relative links, images and #anchors.

    scripts/site-linkcheck.py site/book

External (http, https, mailto) links aren't fetched. Exits 1 on any broken link.
"""
import os
import re
import sys
from html.parser import HTMLParser
from urllib.parse import unquote, urldefrag

SKIP_PAGES = ("print.html", "404.html", "toc.html")


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links, self.ids = [], set()

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if a.get("id"):
            self.ids.add(a["id"])
        for k in ("href", "src"):
            if a.get(k):
                self.links.append(a[k])


def main(root):
    pages = {}
    for d, _, files in os.walk(root):
        for f in files:
            if f.endswith(".html"):
                path = os.path.normpath(os.path.join(d, f))
                page = Page()
                with open(path, encoding="utf-8") as fh:
                    page.feed(fh.read())
                pages[path] = page

    broken = 0
    for path, page in sorted(pages.items()):
        if path.endswith(SKIP_PAGES):
            continue
        for link in page.links:
            if re.match(r"^(https?:|mailto:|javascript:|data:)", link):
                continue
            url, frag = urldefrag(link)
            target = (
                os.path.normpath(os.path.join(os.path.dirname(path), unquote(url)))
                if url
                else path
            )
            where = os.path.relpath(path, root)
            if not os.path.exists(target):
                print(f"missing: {where} -> {link}")
                broken += 1
            elif frag and target in pages and frag not in pages[target].ids:
                print(f"no such anchor: {where} -> {link}")
                broken += 1
    print(f"{len(pages)} pages checked, {broken} broken links")
    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "site/book"))
