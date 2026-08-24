#!/usr/bin/env bash
# Regression pins for the s399 ACME-account-identity ship.
#
# THE DEFECT: `load_or_create_account` was a check-then-act with a network round
# trip in the middle. Two sites created together on a fresh box each spawn an
# auto-SSL task; both saw no account file, both created a DIFFERENT account at
# Let's Encrypt, and the second write clobbered the first. Measured on a
# throwaway box: two "Created new ACME account" lines 443 ms apart, after which
# every certificate issued inside that window answered 422 to renewal for ever
# (the CA rejects the ARI `replaces` hint as unauthorized, and `provision_cert`
# has no path that retries without it), while one issued after the window
# renewed 200.
#
# THE INVARIANT THESE ARMS EXIST TO HOLD: the account handed back is always the
# one whose credentials are on disk.
#
# Why these are pins and not only unit tests: the unit tests in
# `panel/agent/src/services/ssl.rs` cover `persist_account_credentials` — the
# CROSS-PROCESS half — directly, and 3/3 mutations kill them. They cannot see
# the IN-PROCESS half, because the serialising lock lives in a function whose
# other half is a round trip to Let's Encrypt. §A is that half.
#
# ⚠ Arms are scoped to a FUNCTION BODY, never the file, and each subject is
# FLOORED: an `fnbody` whose anchor stops matching yields an EMPTY subject and
# every `hasnt` beneath it then passes green for a file that no longer contains
# the code at all. §Z plants each defect back into a copy and asserts the
# matching arm goes RED — the only execution an arm's failure branch ever gets
# before it matters.
#
# Static analysis over source text: offline, deterministic, same verdict on an
# air-gapped runner.
#   run: bash tests/acme-account-identity-pin-e2e.sh
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()  { printf '  \033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '  \033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }
has() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1" "missing: $3" ;; esac; }
hasnt(){ case "$2" in *"$3"*) bad "$1" "present but must not be: $3" ;; *) ok "$1" ;; esac; }

# ugrep's --ignore-files shim honours .gitignore, so use the real binary.
G=/usr/bin/grep

ASSL=panel/agent/src/services/ssl.rs

# Comments blanked (a pin greps RAW source, so an explanation satisfies it),
# then whitespace and the `\` that continues a Rust string literal removed, so a
# multi-line statement flattens to the token it compiles to.
strip_comments() { awk '/^[[:space:]]*\/\//{print "";next}{print}' "$1"; }
squash() { tr -d ' \n\\'; }
# ⛔ Production only: the pin suites blank from the first `#[cfg(test)]` to EOF,
# and this file's own unit tests spell every token these arms look for.
prod() { awk '/^#\[cfg\(test\)\]/{exit} {print}' "$1"; }
fnbody() { awk -v p="$2" 'index($0,p){f=1} f{print} f && /^}$/{exit}' "$1"; }

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
PRODSRC="$TMP/ssl.prod.rs"
prod "$ASSL" | awk '/^[[:space:]]*\/\//{print "";next}{print}' > "$PRODSRC"

# ── the arms, as ONE function so §Z can point them at a mutated copy ─────────
# $1 = a production-only, comment-blanked copy of agent services/ssl.rs
run_arms() {
  local src="$1"
  local LOAD PERSIST
  LOAD=$(fnbody "$src" 'pub async fn load_or_create_account(')
  PERSIST=$(fnbody "$src" 'pub(crate) async fn persist_account_credentials(')

  # ⛔ FLOOR BOTH SUBJECTS FIRST. Everything below is meaningless if the anchor
  # stopped matching, and `hasnt` would report green on an empty string.
  local NL_LOAD NL_PERSIST
  NL_LOAD=$(printf '%s' "$LOAD" | wc -l | tr -d ' ')
  NL_PERSIST=$(printf '%s' "$PERSIST" | wc -l | tr -d ' ')
  if [ "$NL_LOAD" -lt 15 ]; then
    bad "A0 subject floor" "load_or_create_account body is $NL_LOAD lines — the anchor stopped matching, every arm below is vacuous"
    return
  fi
  ok "A0 subject floor — load_or_create_account body is $NL_LOAD lines"
  if [ "$NL_PERSIST" -lt 10 ]; then
    bad "B0 subject floor" "persist_account_credentials body is $NL_PERSIST lines — anchor stopped matching"
    return
  fi
  ok "B0 subject floor — persist_account_credentials body is $NL_PERSIST lines"

  local LOADQ PERSISTQ
  LOADQ=$(printf '%s' "$LOAD" | squash)
  PERSISTQ=$(printf '%s' "$PERSIST" | squash)

  # ── A: the IN-PROCESS half ────────────────────────────────────────────────
  has "A1 the load-or-create is serialised process-wide" "$LOADQ" "ACCOUNT_INIT.lock().await"
  has "A2 the lock is a real mutex, declared in the function that holds it" "$LOADQ" "staticACCOUNT_INIT:tokio::sync::Mutex<()>"

  # A3 is the one that matters most and is the easiest to get wrong: a lock
  # taken AFTER the existence check serialises nothing. Compare POSITIONS, not
  # presence — presence is A1's claim and passes for the broken ordering too.
  local LOCK_AT READ_AT
  LOCK_AT=$(printf '%s\n' "$LOAD" | $G -n 'ACCOUNT_INIT.lock()' | head -1 | cut -d: -f1)
  READ_AT=$(printf '%s\n' "$LOAD" | $G -n 'stored_account()' | head -1 | cut -d: -f1)
  if [ -n "$LOCK_AT" ] && [ -n "$READ_AT" ] && [ "$LOCK_AT" -lt "$READ_AT" ]; then
    ok "A3 the lock is taken BEFORE the account file is read (lock=$LOCK_AT read=$READ_AT)"
  else
    bad "A3 lock ordering" "the lock must precede the read or it serialises nothing (lock=${LOCK_AT:-none} read=${READ_AT:-none})"
  fi

  hasnt "A4 the clobbering write is gone from the load path" "$LOADQ" "tokio::fs::write(ACME_ACCOUNT_PATH"
  has   "A5 persistence goes through the atomic writer" "$LOADQ" "persist_account_credentials(ACME_ACCOUNT_PATH"
  # A6: losing the cross-process race must RE-READ. Returning the minted account
  # is the defect wearing the fix's clothes — it compiles, it logs, and it
  # issues certificates under a key that is nowhere on disk.
  has "A6 losing the race adopts the STORED account, not the minted one" "$LOADQ" "Persisted::Adopted=>{"
  local ADOPT_ARM
  ADOPT_ARM=$(printf '%s\n' "$LOAD" | awk '/Persisted::Adopted/{f=1} f{print}')
  case "$(printf '%s' "$ADOPT_ARM" | squash)" in
    *"stored_account().await?"*) ok "A7 the Adopted arm re-reads from disk" ;;
    *) bad "A7 the Adopted arm re-reads from disk" "it must call stored_account() again, not return the account it just minted" ;;
  esac

  # ── B: the CROSS-PROCESS half ─────────────────────────────────────────────
  has   "B1 the account file is created atomically" "$PERSISTQ" "create_new(true)"
  hasnt "B2 …and never truncates an existing one"   "$PERSISTQ" "truncate(true)"
  has   "B3 an existing file means ADOPT, not overwrite" "$PERSISTQ" "ErrorKind::AlreadyExists=>Ok(Persisted::Adopted)"
  has   "B4 the account key is owner-only from creation" "$PERSISTQ" "opts.mode(0o600)"

  # ── C: DERIVED, not counted. #748: a threshold over a population cannot name
  # the member that left, so enumerate the writers and check the set itself.
  local WRITERS
  WRITERS=$(printf '%s\n' "$(cat "$src")" | $G -n 'ACME_ACCOUNT_PATH' | $G -vF 'const ACME_ACCOUNT_PATH' \
            | $G -E 'fs::write|OpenOptions|set_permissions|persist_account_credentials' | wc -l | tr -d ' ')
  eq "C1 exactly one call site writes the account path" "$WRITERS" "1"
}

echo "── A + B: the account handed back is the one whose key is on disk ──"
run_arms "$PRODSRC"

# ── Z: every arm above has now printed its own failure at least once ─────────
#
# A green arm is a claim, not a measurement. Each mutation below is the real
# defect — the shape the code actually had, or the shape a plausible "cleanup"
# would produce — and the arm named beside it MUST go red. An arm that stays
# green here is decoration and must be deleted or rewritten.
echo
echo "── Z: plant each defect back and watch the matching arm go red ──"
# $1 label · $2 the arm that MUST go red · $3+ a command reading PRODSRC on stdin
mutate_and_expect() {
  local label="$1" expect_arm="$2"; shift 2
  local copy="$TMP/mut.rs"
  "$@" < "$PRODSRC" > "$copy"
  if cmp -s "$copy" "$PRODSRC"; then
    bad "Z:$label" "the mutation changed NOTHING — the pattern no longer matches, so this check is vacuous"
    return
  fi
  local out sp sf
  sp=$PASS; sf=$FAIL
  # ⚠ Strip ANSI before matching. The arms colour the ✗, so a literal "✗ A1"
  # never matches — the reset sequence sits between the mark and the name. The
  # first draft of this harness reported all six arms as unfalsifiable when
  # every one of them had fired correctly.
  out=$(PASS=0; FAIL=0; run_arms "$copy" 2>&1 | sed -E 's/\x1b\[[0-9;]*m//g')
  PASS=$sp; FAIL=$sf
  if ! printf '%s' "$out" | $G -qE "✗ ${expect_arm}( |\$)"; then
    bad "Z:$label" "expected arm '$expect_arm' to fail and it did not. Arms said: $(printf '%s' "$out" | tr '\n' '|' | cut -c1-300)"
    return
  fi
  # ⛔ An arm that goes red whenever ANY sibling does has not been shown to test
  # its own claim. Where a mutation names an arm that must STAY GREEN, assert it
  # — that is what separates "A3 fired" from "A3 is independent of A1".
  if [ -n "${MUST_STAY_GREEN:-}" ] && ! printf '%s' "$out" | $G -qE "✓ ${MUST_STAY_GREEN}( |\$)"; then
    bad "Z:$label" "$expect_arm went red, but so did ${MUST_STAY_GREEN}, which had to stay green — the mutation is too broad to attribute"
    return
  fi
  ok "Z:$label → $expect_arm went red${MUST_STAY_GREEN:+, $MUST_STAY_GREEN stayed green}"
}

m_drop_lock()    { sed -E 's|^.*ACCOUNT_INIT\.lock\(\)\.await;.*$||'; }
m_plain_create() { sed -E 's|create_new\(true\)|create(true).truncate(true)|'; }
m_drop_mode()    { sed -E 's|^.*opts\.mode\(0o600\);.*$||'; }
m_keep_minted()  { sed -E 's|AlreadyExists => Ok\(Persisted::Adopted\)|AlreadyExists => Ok(Persisted::Written)|'; }
m_rename_anchor(){ sed -E 's|pub async fn load_or_create_account\(|pub async fn load_or_create_acct(|'; }
# ⛔ The ORDERING mutation must MOVE the lock, not delete it. Deleting it also
# kills A1, so a deleting mutation cannot tell whether A3 checks ordering or is
# merely a second copy of A1. This one re-emits the lock AFTER the read, so A1
# stays green and only A3 may fail.
m_lock_after()   { awk '/ACCOUNT_INIT\.lock\(\)\.await;/ { lock = $0; next }
                        /let \(account, creds\) = Account::builder\(\)/ && lock { print lock; lock = "" }
                        { print }'; }

mutate_and_expect "the lock is removed entirely"             "A1" m_drop_lock
MUST_STAY_GREEN=A1 mutate_and_expect "the lock is MOVED below the check" "A3" m_lock_after
mutate_and_expect "create_new becomes a plain create"        "B1" m_plain_create
MUST_STAY_GREEN=B1 mutate_and_expect "the 0600 is dropped"           "B4" m_drop_mode
mutate_and_expect "losing the race keeps the minted account" "B3" m_keep_minted
mutate_and_expect "the anchor is renamed (subject vanishes)" "A0" m_rename_anchor

echo
echo "──────────────────────────────────────────"
printf 'PASS: %d   FAIL: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
