#!/usr/bin/env bash
# secrets-mask-value-boundary-pin-e2e.sh — s442
#
# ONE PROPERTY: masking a secret's value for display never panics, regardless
# of what characters that value contains.
#
# `panel/backend/src/routes/secrets.rs`'s `mask_value` sliced the first 4
# BYTES (`&value[..4]`) of any value longer than 4 bytes. Rust panics if a
# byte-slice boundary doesn't land on a UTF-8 char boundary — reachable from
# any secret value containing a multi-byte character (a pasted passphrase,
# API key, or credential with non-ASCII content). Hit from every masking
# call site: `create_secret`'s response (after the row is already committed,
# so the secret is saved but the caller never sees it), `update_secret`'s
# response, and every subsequent `list_secrets` call with `reveal=false` —
# so a single boundary-violating secret in a vault permanently breaks that
# vault's default masked listing until the offending secret is deleted via
# `reveal=true` (which skips mask_value entirely).
#
# Fix: walk `.chars()` instead of bytes. A char boundary can't be straddled
# by construction — `.chars().take(4)` always stops on one.
#
# §A mask_value walks chars, not raw byte-index slicing.
# §B mask_value is still reachable from all 3 call sites the finder counted.
# §C the fix threshold (4-or-fewer fully masked, more gets a 4-char prefix)
#    is preserved — this is a boundary-safety fix, not a behavior change.
# §D executed unit tests exist for the exact panic scenario (multi-byte value
#    straddling the old byte offset) and the all-multibyte case.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  secrets mask_value UTF-8 boundary — source pins (s442)"
echo "=================================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }

fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn "(") && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

SECRETS=panel/backend/src/routes/secrets.rs
SECRETS_C=$(code "$SECRETS")
MASK_BODY=$(fnbody "$SECRETS_C" "mask_value")

# ── §A no raw byte-index slicing survives ────────────────────────────────
echo "── §A mask_value walks chars, not byte indices ──"

if [ -n "$MASK_BODY" ]; then
  ok "A1 mask_value exists"
else
  bad "A1 mask_value is missing — every arm below measures nothing"
fi

if has "$MASK_BODY" '&value\[\.\.4\]|value\[0\.\.4\]|\[\.\.4\]'; then
  bad "A2 mask_value still byte-slices with a literal ..4 index — the panic is back"
else
  ok "A2 no literal [..4] byte-index slice in mask_value"
fi

if has "$MASK_BODY" '\.chars\(\)'; then
  ok "A3 mask_value iterates .chars() (char-boundary-safe by construction)"
else
  bad "A3 mask_value doesn't use .chars() — the fix shape is gone"
fi

if has "$MASK_BODY" '\.take\(4\)'; then
  ok "A4 the prefix is still 4 characters (unchanged UI contract)"
else
  bad "A4 the prefix length changed or the take(4) call is gone"
fi

# ── §B every known call site still reaches mask_value ────────────────────
echo "── §B mask_value's call-site count is unchanged (3) ──"

# Scoped to code BEFORE the first #[cfg(test)] block so this suite's own
# test-module calls to mask_value() don't inflate the count.
PROD_ONLY=$(awk '/^#\[cfg\(test\)\]/{exit} {print}' "$SECRETS")
CALL_COUNT=$(grep -c 'mask_value(' <<< "$PROD_ONLY")
# 3 call sites + 1 definition = 4 occurrences of the identifier.
if [ "$CALL_COUNT" -eq 4 ]; then
  ok "B1 mask_value appears 4 times (1 definition + 3 call sites), matching the finder's exhaustiveness claim"
else
  bad "B1 mask_value appears $CALL_COUNT times, expected 4 — a call site was added, removed, or the fix moved the definition"
fi

# ── §C the masking threshold behavior is unchanged ────────────────────────
echo "── §C 4-or-fewer stays fully masked; more gets a char prefix ──"

if has "$MASK_BODY" 'is_none\(\)' && has "$MASK_BODY" '••••••••'; then
  ok "C1 a short (<=4 char) value still resolves to the fully-masked constant"
else
  bad "C1 short-value handling looks different — verify by hand"
fi

if has "$MASK_BODY" 'format!\("\{prefix\}••••••••"\)|format!\("\{\}••••••••"'; then
  ok "C2 a long value still formats as prefix + mask suffix"
else
  bad "C2 the long-value format string changed shape"
fi

# ── §D executed tests cover the exact panic scenario ──────────────────────
echo "── §D unit tests exist for the exact byte-offset-4 panic input ──"

TEST_MOD=$(fnbody "$SECRETS_C" "a_multibyte_value_straddling_the_old_byte_offset_does_not_panic")
if [ -n "$TEST_MOD" ]; then
  ok "D1 a test named for the exact straddling scenario exists"
else
  bad "D1 no test names the exact byte-offset-4 straddling scenario"
fi

if grep -q 'mod mask_value_tests' "$SECRETS"; then
  ok "D2 a dedicated mask_value_tests module exists"
else
  bad "D2 no dedicated test module for mask_value"
fi

if grep -qE '日|本|語' "$SECRETS"; then
  ok "D3 the tests use real multi-byte (non-ASCII) literals, not just an assertion of intent"
else
  bad "D3 no multi-byte literal found — the panic scenario may not actually be exercised"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
