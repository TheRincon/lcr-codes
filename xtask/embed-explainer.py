#!/usr/bin/env python3
"""Build the publishable Artifact copy of docs/explainer.html.

`docs/explainer.html` is the source of truth: it links `docs/img/*.png`, so git stores
each figure once as a PNG rather than again as base64, and `just figures` can regenerate
them underneath it.

The Artifact host wants the opposite — a single self-contained page with no
`<html>/<head>/<body>` of its own, since it supplies those. This inlines every figure as
a data URI and strips the wrapper, so the two never drift apart by hand.

    python3 xtask/embed-explainer.py <out.html>
"""

import base64
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "docs" / "explainer.html"


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    out = pathlib.Path(sys.argv[1])
    html = SRC.read_text(encoding="utf-8")

    # Strip the standalone wrapper; the Artifact host provides its own.
    html = html.split("</style>\n</head>\n<body>\n\n", 1)[1]
    html = html.rsplit("\n</body>\n</html>", 1)[0]

    # Re-attach the page's own <title>, fonts and <style>, which live above the wrapper.
    head = SRC.read_text(encoding="utf-8")
    head = head.split('<meta name="viewport" content="width=device-width, initial-scale=1">\n', 1)[1]
    head = head.split("</style>\n", 1)[1]          # drop the host-reset <style>
    head = head.rsplit("</style>\n</head>", 1)[0] + "</style>\n"

    embedded = 0
    for m in sorted(set(re.findall(r'src="(img/[^"]+\.png)"', html))):
        data = (ROOT / "docs" / m).read_bytes()
        uri = "data:image/png;base64," + base64.b64encode(data).decode("ascii")
        html = html.replace(f'src="{m}"', f'src="{uri}"')
        embedded += 1

    if 'src="img/' in html:
        print("error: an image reference was not embedded", file=sys.stderr)
        return 1

    out.write_text(head + "\n" + html + "\n", encoding="utf-8")
    print(f"  embedded {embedded} figures -> {out} ({out.stat().st_size / 1e6:.2f} MB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
