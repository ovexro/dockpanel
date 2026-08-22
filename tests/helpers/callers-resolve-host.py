#!/usr/bin/env python3
"""For one helper name, print every enclosing function that calls it, tagged
RESOLVED or UNRESOLVED by whether that caller derives its agent handle from the
row rather than from the request header.

Split into its own file rather than a heredoc so the pin suite that uses it can
stay readable, and so this can be run by hand while diagnosing a red arm.
"""
import re
import sys
import pathlib

target = sys.argv[1]
for path in sys.argv[2:]:
    src = pathlib.Path(path).read_text()
    src = re.sub(r'^\s*///.*$', '', src, flags=re.M)
    src = re.sub(r'^\s*//.*$', '', src, flags=re.M)
    fns = [(m.start(), m.group(1))
           for m in re.finditer(r'(?:pub )?(?:async )?fn ([a-z_0-9]+)\s*\(', src)]
    for i, (start, name) in enumerate(fns):
        end = fns[i + 1][0] if i + 1 < len(fns) else len(src)
        body = src[start:end]
        if name == target or f'{target}(' not in body:
            continue
        resolved = bool(re.search(
            r'agent_for_site_server\(|site_agent_for_caller\(|\bfor_server\(', body))
        print(f"{path}::{name}:{'RESOLVED' if resolved else 'UNRESOLVED'}")
