#!/usr/bin/env python3
"""Updates TRUSTED_CONTRIBUTORS in upstream-sync.sh with the full
list of upstream contributors to the ported directories."""
import re

# read the contributor list
contribs_raw = open("/tmp/contribs_str.txt").read().strip()
contribs = contribs_raw.rstrip(",")

# read the script
import os
path = os.path.join(os.path.dirname(__file__), "upstream-sync.sh")
src = open(path).read()

# replace the TRUSTED_CONTRIBUTORS default
pattern = re.compile(r'TRUSTED_CONTRIBUTORS="\$\{TRUSTED_CONTRIBUTORS:-[^"]*"')
new_line = f'TRUSTED_CONTRIBUTORS="${{TRUSTED_CONTRIBUTORS:-{contribs}"'
if pattern.search(src):
    src = pattern.sub(new_line, src, count=1)
    open(path, "w").write(src)
    count = contribs.count(",") + 1
    print(f"updated with {count} contributors")
    print(f"first few: {contribs[:120]}...")
else:
    print("pattern not found")
    # show what's there
    for line in src.split("\n"):
        if "TRUSTED" in line:
            print("  found:", line[:120])
