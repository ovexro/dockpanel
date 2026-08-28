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
#   §E  s419 (v2.172.0): the uptime HTTP check's redirect-following policy used to
#       re-validate each hop with a BLOCKING, synchronous check
#       (`host_resolves_internal_blocking`) and then let reqwest resolve the
#       hop's hostname AGAIN, independently, to actually connect — the same
#       check-then-reconnect TOCTOU `resolve_validated`/`pinned_client` closed
#       for the INITIAL request at v2.166.0, still open on every redirect hop.
#       Closed by installing `ValidatingResolver` (a `reqwest::dns::Resolve`
#       impl, `helpers.rs`) as the client's DNS resolver via `.dns_resolver`,
#       sharing `resolve_all_validated` with `resolve_validated` so validation
#       and connection are now the SAME lookup on every hop, not two. The
#       synchronous blocking check stays — it is the only thing that catches a
#       redirect `Location` that is already a literal internal IP, which never
#       reaches a `Resolve` impl at all.

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
echo "§E  the uptime redirect-hop resolver closes the check-then-reconnect gap"

# Isolate http_check_client_builder()'s own body (start of its signature to the
# closing brace at column 0 that ends the function) rather than grepping the
# whole file — a bare '.dns_resolver(' or 'ValidatingResolver' match anywhere
# else (e.g. this very comment block) would be vacuous. `code()` strips
# comments FIRST so the header text above can never satisfy the co-occurrence
# it is itself describing.
BUILDER_FN=$(code "$UPT" | perl -0777 -ne 'print $1 if /(fn http_check_client_builder\(\).*?\n\}\n)/s')
if [ -z "$BUILDER_FN" ]; then
  bad "§E could not isolate http_check_client_builder() body — function renamed or removed?"
elif grep -qF '.dns_resolver(' <<< "$BUILDER_FN" && grep -qF 'ValidatingResolver' <<< "$BUILDER_FN"; then
  ok "§E http_check_client_builder() installs .dns_resolver(...ValidatingResolver...)"
else
  bad "§E http_check_client_builder() no longer wires a custom dns_resolver"
fi
# Positive control: the pre-existing synchronous blocking check is STILL in the
# SAME function — proves the resolver was ADDED, not swapped in to replace the
# only thing that can catch a literal-IP redirect target (a Resolve impl is
# never consulted for one; hyper's connector dials a literal IP directly).
if grep -qF 'host_resolves_internal_blocking' <<< "$BUILDER_FN"; then
  ok "§E http_check_client_builder() still runs the synchronous per-hop block too"
else
  bad "§E the synchronous redirect-hop check is gone from http_check_client_builder()"
fi

# ValidatingResolver's OWN resolve() body must call the real validator, not a
# stub — isolate the impl block specifically, not the whole file.
RESOLVER_IMPL=$(code "$HELPERS" | perl -0777 -ne 'print $1 if /(impl reqwest::dns::Resolve for ValidatingResolver \{.*?\n\}\n)/s')
if [ -z "$RESOLVER_IMPL" ]; then
  bad "§E could not isolate ValidatingResolver's Resolve impl — renamed or removed?"
elif grep -qF 'resolve_all_validated(' <<< "$RESOLVER_IMPL"; then
  ok "§E ValidatingResolver::resolve() calls the real validator, not a stub"
else
  bad "§E ValidatingResolver::resolve() no longer calls resolve_all_validated"
fi

# The structural security property: the redirect-hop guard and the
# INITIAL-request guard (resolve_validated, already pinned by §B/§C's siblings)
# share ONE classifier rather than two that could quietly drift apart — count
# resolve_all_validated( call sites across the whole file; both callers plus
# its own `fn` line is 3.
CALLS=$(grep -coF 'resolve_all_validated(' <<< "$(code "$HELPERS")")
if [ "${CALLS:-0}" -ge 3 ]; then
  ok "§E resolve_all_validated is the SHARED core for both the initial request and every redirect hop ($CALLS occurrences)"
else
  bad "§E resolve_all_validated is not shared by both guards ($CALLS occurrences, want ≥3 — declaration + 2 callers)"
fi

echo
echo "----------------------------------------------"
printf '  PASS %d   FAIL %d   SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo
[ "$FAIL" -eq 0 ] || exit 1
