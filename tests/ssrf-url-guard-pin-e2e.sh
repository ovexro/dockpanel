#!/usr/bin/env bash
# ssrf-url-guard-pin-e2e.sh — s383
#
# One SSRF guard, parsed by a real URL parser, in one place.
#
# `validate_url_not_internal` used to extract the host by string surgery — strip
# the scheme by byte offset, `split('/').next()`, then `split(':').next()`. For a
# userinfo authority that reads the WRONG host: `http://192.0.2.1:x@127.0.0.1/`
# tested `192.0.2.1` (public → PASS) while every HTTP client dials `127.0.0.1`.
# Proven live on v2.135.0: an ordinary account pointed a monitor at
# `http://example.com:x@127.0.0.1:9999/` and read an internal-only service's
# status code back, plus a keyword oracle over its body. A second, hand-rolled
# copy with a strictly weaker classifier sat in `routes/extensions.rs`.
#
#   §A  THE HAND-ROLLED HOST EXTRACTION IS GONE, in BOTH files. A grep over raw
#       source matches the comment that documents its removal, so every arm reads
#       COMMENT-STRIPPED, WHITESPACE-COLLAPSED source and asserts the token
#       SEQUENCE `split('/').next()…split(':').next()` is absent. rustfmt is a
#       second author with commit rights, so whitespace is squeezed from both
#       sides before matching.
#   §B  THE ONE PARSER IS PRESENT (positive control). `url::Url::parse` is the
#       host authority in `helpers.rs`, and `extensions.rs` DELEGATES to the
#       shared helper rather than re-parsing. An arm that only forbids the old
#       shape passes vacuously if the whole function is deleted.
#   §C  THE tcp/ping LANE IS GUARDED TOO. `validate_host_not_internal` exists and
#       is called at create, update and check time — the internal port-scanner
#       lane the HTTP guard alone left open.
#   §D  CONTEXT. Arms green at BOTH tags so a harness measuring nothing cannot
#       read as a pass.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  SSRF URL guard — source pins (s383)"
echo "=============================================="
echo

PASS=0; FAIL=0; SKIP=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip block, line and doc comments so an arm cannot match the prose that
# documents the very code it forbids.
code() {
  perl -0777 -pe '
    s{/\*.*?\*/}{}gs;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}
# Collapse ALL whitespace so rustfmt line-wrapping cannot hide a token sequence.
flat() { code "$1" | tr -d ' \t\n'; }

HELPERS=panel/backend/src/helpers.rs
EXT=panel/backend/src/routes/extensions.rs
MON=panel/backend/src/routes/monitors.rs
UPT=panel/backend/src/services/uptime.rs

for f in "$HELPERS" "$EXT" "$MON" "$UPT"; do
  [ -f "$f" ] || { echo "  FATAL: $f missing — wrong tree?"; exit 1; }
done

# The retired shape's actual fingerprint: split('/').next() to strip the path,
# THEN — chained in the SAME statement, no ';' between — split(':').next() to
# strip the port, in that exact order (the doc comment above names it: "strip the
# scheme … split('/').next() … split(':').next()"). Matching the two substrings
# ANYWHERE in the file, independent of order or statement boundary, over-fires on
# a legitimate LATER use of the same two calls for something that isn't URL-authority
# extraction at all — e.g. parsing the scp-like `git@host:path` git-remote shorthand
# (s417's git-deploy repo_url SSRF guard), which is not a URI, has no userinfo
# ambiguity to get wrong, and genuinely cannot go through `url::Url::parse`. Per the
# s416 pin lesson (project_dockpanel_lessons_p153 #862): when a pin fires on a
# legitimate new shape, tighten the pin to the ACTUAL invariant instead of loosening
# the code to dodge it.
OLD_CHAIN='split\(\x27/\x27\)\.next\(\)[^;]*split\(\x27:\x27\)\.next\(\)'

echo "§A  the hand-rolled host extraction is absent in both parsers"
for f in "$HELPERS" "$EXT"; do
  F=$(flat "$f")
  if grep -qP "$OLD_CHAIN" <<< "$F"; then
    bad "§A $f: hand-rolled 'split('/')…split(':')' host extraction is back"
  else
    ok "§A $f: no substring host extraction"
  fi
done

echo
echo "§B  the one real parser is present (positive control)"
if grep -qF "url::Url::parse" <<< "$(flat "$HELPERS")"; then
  ok "§B helpers.rs parses with url::Url::parse"
else
  bad "§B helpers.rs no longer parses with url::Url — the guard may have been gutted"
fi
# extensions.rs must DELEGATE, not re-implement: it calls the shared helper and
# does NOT itself call url::Url::parse or lookup_host for the webhook URL.
EXTF=$(flat "$EXT")
if grep -qF "helpers::validate_url_not_internal" <<< "$EXTF"; then
  ok "§B extensions.rs delegates to the shared helper"
else
  bad "§B extensions.rs no longer delegates to helpers::validate_url_not_internal"
fi
if grep -qF "tokio::net::lookup_host" <<< "$EXTF"; then
  bad "§B extensions.rs resolves a host itself again — the weak second copy is regrowing"
else
  ok "§B extensions.rs does not resolve a webhook host itself"
fi

echo
echo "§C  the tcp/ping lane is guarded (bare-host SSRF check exists and is wired)"
if grep -qF "fn validate_host_not_internal" <<< "$(code "$HELPERS")"; then
  ok "§C helpers.rs defines validate_host_not_internal"
else
  bad "§C helpers.rs no longer defines the bare-host guard"
fi
# Write time (monitors.rs, create+update) still calls validate_host_not_internal
# directly — nothing to connect to yet, so there is nothing to pin.
#
# Check time (uptime.rs, check_tcp/check_ping) was rewritten at s414 to call
# `resolve_validated` instead: the SAME internal-address check
# (ip_is_internal over a literal IP or every resolved address), but returning
# the one validated SocketAddr so the connect below dials THAT address rather
# than asking the OS to resolve the hostname a second, independent time (the
# TOCTOU a rebinding DNS server could exploit between the old validate-then-
# reconnect pair). Count either name at check time — both are the real guard;
# only the identifier changed.
UPT_CALLS=$(grep -cE "validate_host_not_internal|resolve_validated" <<< "$(code "$UPT")")
MON_CALLS=$(grep -cF "validate_host_not_internal" <<< "$(code "$MON")")
if [ "${UPT_CALLS:-0}" -ge 2 ]; then
  ok "§C uptime.rs re-validates the host in check_tcp and check_ping ($UPT_CALLS calls)"
else
  bad "§C uptime.rs check-time host validation missing ($UPT_CALLS calls, want ≥2)"
fi
if [ "${MON_CALLS:-0}" -ge 2 ]; then
  ok "§C monitors.rs validates the tcp/ping host at create and update ($MON_CALLS calls)"
else
  bad "§C monitors.rs write-time host validation missing ($MON_CALLS calls, want ≥2)"
fi

echo
echo "----------------------------------------------"
printf '  PASS %d   FAIL %d   SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo
[ "$FAIL" -eq 0 ] || exit 1
