#!/usr/bin/env python3
"""Print ONE branch of SiteDetail's `{site.ssl_enabled ? ( … ) : ( … )}` ternary.

⛔ WHY THIS EXISTS. The defect it guards is a control that is PRESENT in the file
and UNREACHABLE in a state: every SSL control lived in the `else` branch, so once
SSL was on the page offered nothing. Any arm that greps the whole file for the
upload form is therefore GREEN AT HEAD WITH THE DEFECT FULLY RESTORED — the
caller exists, it is simply in the wrong branch. Scoping to the branch is the
only thing that makes such an arm mean anything ([[feedback_pin_arm_scope]]).

Usage:  ssl-enabled-branch.py <file> {true|false}

Emits the branch with comments removed — `//` line comments AND `{/* … */}` JSX
blocks, which the calling suite's `subj()` does NOT strip. That matters here more
than usual: the true branch carries a long JSX comment that names the very
controls the arms assert, so an unstripped subject would satisfy them from prose
([[feedback_source_pin_prose_trap]]). Whitespace is squeezed last, so the output
is the flattened token stream the arms match against.

Exits 2 with a message on stderr if the ternary cannot be located, so a caller
that renames or restructures it gets a LOUD failure rather than an empty subject
every absence arm passes vacuously.
"""
import re
import sys

OPEN = "{site.ssl_enabled ? ("


def strip_comments(text: str) -> str:
    # JSX block comments first: they can span lines and can contain `//`.
    text = re.sub(r"\{/\*.*?\*/\}", "", text, flags=re.S)
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    text = re.sub(r"//.*$", "", text, flags=re.M)
    return text


def main() -> int:
    if len(sys.argv) != 3 or sys.argv[2] not in ("true", "false"):
        print("usage: ssl-enabled-branch.py <file> {true|false}", file=sys.stderr)
        return 2
    path, which = sys.argv[1], sys.argv[2]
    src = open(path, encoding="utf-8").read()

    start = src.find(OPEN)
    if start < 0:
        print(f"{path}: ternary opener {OPEN!r} not found", file=sys.stderr)
        return 2

    # Walk parens from the opener so the split point is the ternary's OWN `) : (`
    # and not one belonging to a nested ternary — of which this file has five.
    i = start + len(OPEN) - 1  # sits on the '(' of the opener
    depth = 0
    split = end = -1
    while i < len(src):
        c = src[i]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                rest = src[i:]
                if split < 0 and re.match(r"\)\s*:\s*\(", rest):
                    split = i
                    i += 1
                    continue
                end = i
                break
        i += 1

    if split < 0 or end < 0:
        print(f"{path}: could not resolve the ternary's branches", file=sys.stderr)
        return 2

    branch = src[start + len(OPEN) : split] if which == "true" else src[split:end]
    out = re.sub(r"\s+", "", strip_comments(branch))
    if not out:
        print(f"{path}: {which} branch is empty after stripping", file=sys.stderr)
        return 2
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
