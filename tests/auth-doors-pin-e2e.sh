#!/usr/bin/env bash
# Regression pins for s277 — every door that mints a session owes the same gates.
#
# A session cookie is a session cookie no matter which handler emits it, but the
# cross-cutting access gates were written per-handler, and by v2.46.0 they had
# drifted into four different opinions about who may sign in:
#
#   D1  THE IP ALLOWLIST GATED ONE DOOR. `allowed_panel_ips` gates login — s276
#       gave it range matching, save-time validation and a control. It lived
#       inline in `login` alone, so an operator who restricted the panel to their
#       office range still had `passkey/auth/*` and `oauth/{provider}/callback`
#       answering from anywhere, both issuing the same cookie. Driven on a real
#       box from an excluded address: the password door returned 403 while
#       passkey/auth/begin returned 200 and a usable WebAuthn challenge.
#   D2  LOCKDOWN SKIPPED THE OAUTH DOOR. Lockdown holds non-admins out until it
#       expires. `login` and `passkey/auth/complete` checked it; `oauth::callback`
#       had zero occurrences of the lockdown probe, so with SSO configured a
#       lockdown did not apply to the SSO path.
#   D3  THE SECOND-STEP DOOR WAS INVISIBLE. `twofa_verify` is what actually emits
#       the cookie when 2FA is on, holding a 5-minute bearer token from a first
#       request. It re-checked suspension and nothing else, so the temp token
#       could be redeemed from an excluded address or after a lockdown began.
#
# The shape is lesson #104d one layer out: a control's blast radius is what it
# GATES, not what it names — and here the same control had four different radii.
#
# These pins DISCOVER their subjects. Listing the four doors would reproduce the
# defect: the list is complete only until someone adds a fifth. So a door is
# found by what makes it a door — a handler that emits a session cookie — and the
# suite FAILS if it discovers fewer doors than are known to exist, because a
# broken discovery pattern must never read as a clean sweep (lesson #100, and
# #104f, where a line-oriented pattern found 7 of 8 subjects and three arms below
# it passed vacuously).
#
# Pure source analysis: no box, no network, no build.
#
# ─────────────────────────────────────────────────────────────────────────────
# s338 — WHAT THIS SUITE MEASURED, AND WHAT IT ONLY APPEARED TO
#
# Every arm here read RAW source. A pin that greps raw source cannot tell the
# mechanism from the story told about the mechanism, so a COMMENT satisfies an
# arm whose code is gone. Measured at c9d2a1f by mutation rather than argued —
# five of the thirty-two assertions were satisfiable by prose, and one arm was
# the mirror-image failure:
#
#   · The OAuth door's allowlist call was replaced by a tombstone comment naming
#     the helper. Green. Blanking the same line instead: red. The comment was the
#     entire difference.
#   · The OAuth door's lockdown test was neutered to a constant, with the old
#     expression left behind as an explanatory tail comment. Green.
#   · The whole revocation block on the metrics-socket decode path was commented
#     out. Green — and this arm's own in-file note claimed that keying on the
#     read operation dodged the prose trap. It dodged only the PRE-EXISTING
#     prose, never a newly written comment.
#   · WORST, because it needed no cooperation at all: the panic handler's entire
#     session-revocation leg — the audit insert AND the watermark write — was
#     BLANKED, no attacker comment involved, and the arm stayed green, fed by an
#     unrelated narrative comment 150 lines above that happens to spell the
#     watermark's name. Panic could silently stop revoking sessions today and
#     this pin would not notice.
#   · The negative arm over the panic response went the other way: a single
#     explanatory comment quoting the forbidden field turned the suite RED on
#     correct code. A false alarm is the same defect wearing the other polarity.
#
# So the comment strippers now sit ABOVE section 1 rather than being absent, they
# choose their comment syntax by EXTENSION rather than assuming one, and they
# BLANK comment lines instead of deleting them — deleting renumbers a file, and
# the body slicers below depend on height. §6-§9 hold all of that in place, and
# §7 is the mutation itself, encoded so it re-runs on every push instead of
# living in one session's notes.
#
# The guide under docs/ is the exception and it is deliberate: it is prose BY
# DESIGN, its arms assert sentences, and routing it through a stripper that
# treats a leading hash as a comment would blank its own headings. §6 pins that
# reason so the exemption cannot later be mistaken for an oversight.
# ─────────────────────────────────────────────────────────────────────────────

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# ── the comment strippers, ABOVE every arm that needs them (s338) ────────────
#
# A check placed below the thing it must run before inherits that position as a
# defect even when the check itself is perfect — the s336 lesson, and the reason
# these functions are the first thing in the file rather than the middle. The
# sibling suite shipped them at line 379 with thirty arms above the definition;
# reproducing that layout here would reproduce that outcome here.
#
# The comment marker is chosen by EXTENSION, never guessed. Getting it wrong is
# not symmetric: over-stripping turns a positive arm loudly red, but turns a
# NEGATIVE arm (match ⇒ bad) silently green, which is the direction that ships.
comment_marker_for() {
  case "$1" in
    *.rs|*.mjs|*.js|*.ts|*.tsx)                echo '//' ;;
    *.json)                                    echo ''   ;;  # JSON has no comments
    *)                                         echo '#'  ;;  # .sh, .service, .gitignore, .toml
  esac
}

# BLANK the comment, do not delete it. Every body slicer below cuts a range out
# of this stream and one arm reports the height of what it cut; a stripper that
# removes lines renumbers the file and silently changes what those arms measure.
code_lines() {
  local f="$1" m
  m=$(comment_marker_for "$f")
  if [ -z "$m" ]; then cat "$f"; return; fi
  if [ "$m" = '//' ]; then
    # A block comment is recognised ONLY where one is actually written: opening
    # at the start of a line and closing at the end of one. s294: a `/*` inside a
    # string literal (a Dockerfile's release-artifact glob) opened a comment that
    # swallowed 485 lines of git_build.rs, and every ABSENCE arm over it passed
    # on code the stripper had merely removed.
    #
    # A trailing `//` is stripped, but only when the text before it has BALANCED
    # double quotes — otherwise an https URL in a string literal would lose its
    # tail. Backslash escapes are skipped so an escaped quote cannot flip the
    # parity.
    awk '
      /^[[:space:]]*\/\*/ &&  /\*\/[[:space:]]*$/ { print ""; next }
      /^[[:space:]]*\/\*/                         { inblk = 1; print ""; next }
      inblk { if ($0 ~ /\*\/[[:space:]]*$/) inblk = 0; print ""; next }
      /^[[:space:]]*\/\//                         { print ""; next }
      {
        line = $0; out = line; n = length(line); q = 0; i = 1
        while (i <= n) {
          c = substr(line, i, 1)
          if (c == "\\") { i += 2; continue }
          if (c == "\"") { q = 1 - q; i++; continue }
          if (q == 0 && c == "/" && substr(line, i + 1, 1) == "/") { out = substr(line, 1, i - 1); break }
          i++
        }
        print out
      }
    ' "$f"
  else
    # Shell keeps to FULL-LINE comments only, and the asymmetry with `//` above is
    # deliberate. A trailing `#` cannot be stripped safely here: parameter-expansion
    # trimming, the argument-count variable, a colour literal and a sed script that
    # deletes hashes are all ordinary code. The convention in this tree is that
    # explanation goes on its own line, and §6 asserts the stripper leaves a `#`
    # inside a string alone.
    awk '/^[[:space:]]*#/ { print ""; next } { print }' "$f"
  fi
}

# Counted, never `grep -q`, and the reason is a trap this suite walked straight
# into twice. Under `set -o pipefail`, `code_lines f | grep -q PATTERN` reports
# FAILURE on a successful match: grep -q exits at the first hit, the producer
# upstream dies of SIGPIPE (141), and pipefail takes the pipeline's status from
# it. The effect is silent and selective — a file whose match is near the top gets
# dropped while one whose match is near the bottom survives, because the producer
# had already finished. `grep -c` consumes all of its input, so there is no early
# exit and no signal. (#103d is the same operator lying in the other direction:
# a failed PRODUCER read as a clean no-match.)
code_count() { local f="$1"; shift; code_lines "$f" | grep -c "$@" 2>/dev/null || true; }
has_code()   { local f="$1"; shift; [ "$(code_count "$f" "$@")" -gt 0 ]; }

# True file line numbers, because code_lines blanks rather than deletes.
code_lineno() { local f="$1"; shift; code_lines "$f" | grep -n "$@" 2>/dev/null | head -1 | cut -d: -f1 || true; }

BE=panel/backend/src
ROUTES=$BE/routes

[ -d "$ROUTES" ] || { echo "missing dir: $ROUTES"; exit 1; }

# The named subjects. §9 scans this file for any arm that reads one of them
# without going through code_lines; the guide is deliberately NOT among them.
AUTH_RS=$ROUTES/auth.rs
OAUTH_RS=$ROUTES/oauth.rs
PASSKEYS_RS=$ROUTES/passkeys.rs
SEC_RS=$ROUTES/security.rs
SET_RS=$ROUTES/settings.rs
WS_RS=$ROUTES/ws_metrics.rs
HELPERS=$BE/helpers.rs
HEALER=$BE/services/auto_healer.rs
SET_TSX=panel/frontend/src/pages/Settings.tsx
AGENT_SEC=panel/agent/src/routes/security.rs
TERM_SRC=panel/agent/src/routes/terminal.rs

# PROSE BY DESIGN — read RAW, deliberately. The three arms over it assert
# SENTENCES a human wrote for a human; there is no code in it to isolate, and the
# shell marker would blank every ATX heading. §6 pins that reasoning.
SESSDOC=docs/guides/sessions.md

for f in "$AUTH_RS" "$OAUTH_RS" "$PASSKEYS_RS" "$SEC_RS" "$SET_RS" "$WS_RS" \
         "$HELPERS" "$HEALER" "$SET_TSX" "$AGENT_SEC" "$TERM_SRC" "$SESSDOC"; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

FIXDIR=$(mktemp -d)
trap 'rm -rf "$FIXDIR"' EXIT

# ── body slicers ────────────────────────────────────────────────────────────
#
# STRIP FIRST, THEN SLICE. code_lines blanks rather than deletes, so height and
# line numbers survive and every range boundary lands exactly where it did on the
# raw file — while the text inside the range is code only. Reversing the order
# would slice raw source and hand a comment-fed body to every arm below.
#
# Each takes its subject as an ARGUMENT rather than reading a global, because §8
# re-runs the same slicer over a deliberately broken copy. A control that goes
# down a different path than the arm proves nothing about the arm.
door_body() {   # <file> <fn>
  code_lines "$1" | FN="$2" perl -0777 -ne '
    if (/pub async fn\s+\Q$ENV{FN}\E\s*\(/g) {
      my $rest = substr($_, pos($_));
      my ($b) = $rest =~ /^(.*?)(?=\npub (?:async )?fn |\z)/s;
      print $b;
    }'
}
panic_body() { code_lines "$1" | sed -n '/pub async fn panic_button/,/^}/p'; }
kill_body()  { code_lines "$1" | sed -n '/async fn kill_terminals/,/^}/p'; }

# The §3 predicate, factored out so §8 can drive the identical code path over a
# planted file. Returns the members of the given list that touch the settings key
# in CODE.
inline_allowlist_files() {
  local f
  for f in "$@"; do has_code "$f" -F -e 'allowed_panel_ips' && printf '%s\n' "$f"; done
  return 0
}

# NOTE ON LINE ORIENTATION: door bodies are sliced with perl -0777, not grep. A
# gate call wrapped across lines is invisible to a line-at-a-time pattern, which
# is exactly how #104f's arm found 7 of 8 knobs.

# ── Discovery ───────────────────────────────────────────────────────────────
#
# A door is a `pub async fn` whose body emits an auth session cookie. We look for
# the two ways the tree does that: calling the shared issuer, or formatting the
# session cookie inline. Logout is excluded by construction — it emits an empty
# value to CLEAR the cookie, and clearing a session is not minting one.
#
# Read per file through code_lines, so a handler that has been commented out is
# not discovered as a live door — the discovery count is the denominator for
# eight arms below, and inflating it with dead code inflates them all.

DOORS=$(for f in "$ROUTES"/*.rs; do
  code_lines "$f" | SUBJ="$f" perl -0777 -ne '
    while (/pub async fn\s+(\w+)\s*\(/g) {
      my $name = $1;
      my $start = pos($_);
      my $rest = substr($_, $start);
      # body = up to the next top-level `pub async fn` / `pub fn` / end of file
      my ($body) = $rest =~ /^(.*?)(?=\npub (?:async )?fn |\z)/s;
      next unless defined $body;
      next unless $body =~ /issue_session(?:_pub)?\s*\(/
               || $body =~ /"token=\{token\}/;
      print "$ENV{SUBJ}:$name\n";
    }'
done 2>/dev/null | sort -u)

DOORS_N=$(printf '%s\n' "$DOORS" | grep -c . || true)

echo "── discovery ──"
printf '%s\n' "$DOORS" | sed 's/^/  door: /'

# §0 — the discovery itself. Four doors are known to exist (login, twofa_verify,
# passkeys::auth_complete, oauth::callback). Fewer means the pattern broke and
# every arm below would pass by inspecting nothing.
if [ "$DOORS_N" -ge 4 ]; then
  ok "discovered $DOORS_N session-minting doors (>= 4 known to exist)"
else
  bad "only $DOORS_N doors discovered — the mint pattern moved; every arm below is now vacuous"
fi

# ── §1 every door enforces the panel IP allowlist ───────────────────────────
echo "── §1 allowed_panel_ips covers every door ──"
for d in $DOORS; do
  file=${d%%:*}; fn=${d##*:}
  body=$(door_body "$file" "$fn")
  n=$(printf '%s' "$body" | grep -c "enforce_panel_ip_allowlist" || true)
  if [ "$n" -ge 1 ]; then
    ok "$(basename "$file")::$fn enforces the IP allowlist"
  else
    bad "$(basename "$file")::$fn mints a session WITHOUT the IP allowlist — an excluded address can sign in here"
  fi
done

# ── §2 every door enforces lockdown (with the admin escape hatch) ────────────
echo "── §2 lockdown covers every door ──"
for d in $DOORS; do
  file=${d%%:*}; fn=${d##*:}
  body=$(door_body "$file" "$fn")
  n=$(printf '%s' "$body" | grep -c "is_locked_down" || true)
  if [ "$n" -ge 1 ]; then
    ok "$(basename "$file")::$fn enforces lockdown"
  else
    bad "$(basename "$file")::$fn mints a session WITHOUT the lockdown check — lockdown does not hold this door"
  fi
done

# ── §3 the allowlist lives in ONE place ─────────────────────────────────────
# The defect was a gate written inline in one handler. If a second inline copy of
# the settings lookup reappears, the two will drift exactly as the settings
# whitelist did (#104g), so the query belongs to the helper alone.
#
# NEGATIVE ARM: a match here means BAD. Narrowing it with the stripper therefore
# fails in the silent direction — a stripper that ate too much would report a
# clean tree over a genuinely duplicated gate. §8 plants the real duplicate and
# requires this predicate to fire.
echo "── §3 one implementation, not a copy per door ──"
INLINE=$(inline_allowlist_files "$ROUTES"/*.rs)
# auth.rs holds the helper; settings.rs validates the value on save. Any third
# route file touching the key is an inline re-implementation.
UNEXPECTED=$(printf '%s\n' "$INLINE" | grep -v -e 'routes/auth\.rs$' -e 'routes/settings\.rs$' || true)
if [ -z "$UNEXPECTED" ]; then
  ok "allowed_panel_ips is referenced only by the helper (auth.rs) and its validator (settings.rs)"
else
  bad "inline allowlist re-implementation in: $(printf '%s ' $UNEXPECTED) — it will drift from the helper"
fi

# The helper must fail closed. `panel_ip_allowed` returns false for an
# unparseable client address; a well-meaning permissive fallback here would admit
# every request that arrives without a forwarded-address header.
HELPER_OK=$(code_lines "$HELPERS" | perl -0777 -ne 'print "1" if /fn panel_ip_allowed.*?let Ok\(client\).*?else \{.*?return false;/s')
if [ -n "$HELPER_OK" ]; then
  ok "panel_ip_allowed fails CLOSED when the client address is unusable"
else
  bad "panel_ip_allowed no longer visibly fails closed on an unparseable client address"
fi

# ── §4 ingestion preserves when an event HAPPENED ───────────────────────────
# Auto-lockdown counts "N events within M minutes" over created_at. Stamping an
# ingested backlog with NOW() collapses however long it took to accumulate into
# one instant. Before v2.46.0 ingestion sat under `auto_heal_enabled` (seeded
# false), so a stock box queued events on disk that nothing drained — and the
# first tick after upgrading replayed the whole file as simultaneous and locked
# every non-admin out. Driven on a real box: 16 events spread over ~3 minutes of
# real time landed within 105ms of each other, and the tenant got a 503.
echo "── §4 suspicious-event ingestion honours the event's own timestamp ──"
n=$(code_lines "$HEALER" | perl -0777 -ne 'print "1" if /record_suspicious_event_at\s*\(/s' | grep -c 1 || true)
if [ "$n" -ge 1 ]; then
  ok "the jsonl ingest records events at their occurrence time, not at ingest time"
else
  bad "the ingest no longer passes an occurrence time — a queued backlog will trip auto-lockdown on the first tick"
fi
n=$(code_lines "$HEALER" | perl -0777 -ne 'print "1" if /event\["timestamp"\]/s' | grep -c 1 || true)
if [ "$n" -ge 1 ]; then
  ok "the agent's per-event timestamp is read (the agent has always written it)"
else
  bad "the ingest ignores the agent's timestamp field — the writer/reader pair is severed again"
fi

# ── §5 the recording toggle cannot claim more than the fleet delivers ───────
# The toggle is one settings row enforced by each server's agent: an agent older
# than the gate release ignores the signed claim and keeps recording. The panel
# already knows every member's version (servers.agent_version, written on each
# check-in), so reporting a flat "disabled" is a claim it can itself falsify —
# lesson #104a, one layer out.
echo "── §5 the recording toggle discloses fleet members it cannot control ──"
n=$(code_count "$SET_RS" -F -e 'recording_coverage')
if [ "$n" -ge 1 ]; then
  ok "backend exposes recording coverage across the fleet"
else
  bad "no recording-coverage endpoint — the toggle makes a fleet-wide claim nothing checks"
fi
n=$(code_count "$SET_TSX" -F -e 'recording-coverage')
if [ "$n" -ge 1 ]; then
  ok "the Settings toggle consumes it, so lagging members are named in the UI"
else
  bad "the UI never reads recording coverage — operators are told 'disabled' while members still record"
fi

# ─────────────────────────────────────────────────────────────────────────────
# s308 — THE SAME LESSON ONE LAYER FURTHER ON: a control's blast radius is what
# it GATES. D1/D2/D3 above are about doors that forgot a gate. These are about
# the EMERGENCY control, which gated the doors and nothing else.
#
# Lockdown is tested at login, OAuth, passkey and site creation — every one of
# them a door. Nothing tests it against a token that has ALREADY been minted. So
# the panic button locked the doors while the intruder was already inside, and
# the admin session that got them there kept working for the rest of its two
# hours. Panic must therefore revoke sessions, not merely lock down.
echo
echo "── s308: the panic button ──"

PANIC=$(panic_body "$SEC_RS")
if [ -z "$PANIC" ]; then
  bad "could not extract panic_button — every arm below is measuring nothing"
else
  ok "extracted panic_button ($(wc -l <<< "$PANIC") lines) — the arms below read real code"
fi

if grep -q "sessions_revoked_at" <<< "$PANIC"; then
  ok "panic WRITES the session-revocation watermark"
else
  bad "panic only locks the doors — the stolen session that reached the root shell keeps working"
fi

# ⚠ The arm above says WRITES, and that wording is load-bearing. It shipped in
# v2.65.0 reading "panic revokes every issued session, not just the doors" — a
# claim about every READER, tested by grepping one WRITER. It was green while
# the metrics socket hand-decoded JWTs and never consulted the watermark, so a
# stolen admin token could open a NEW process-list socket after the button was
# pressed. A watermark nothing reads revokes nothing; pin the readers.
#
# ⚠⚠ And until s338 it was green with the ENTIRE revocation leg blanked out,
# because an unrelated narrative comment 150 lines higher in the same handler
# spells the watermark's name. Slicing the STRIPPED file is what removed that
# false floor. The claim "keying on the read operation dodges the prose trap",
# which this block used to make below, was false — it dodged only the prose that
# already existed, never a comment someone writes tomorrow.
#
# Class arm: every hand-rolled Claims decode outside the AuthUser extractor must
# consult the watermark, and both halves — the enumeration and the per-file
# check — now read code only.
DECODERS=$(for f in $(find "$BE" -name '*.rs' | sort); do
  case "$f" in */auth.rs) continue ;; esac
  has_code "$f" -F -e 'decode::<Claims>' && printf '%s\n' "$f"
done)
if [ -z "$DECODERS" ]; then
  bad "found no hand-rolled Claims decoders at all — this arm is measuring nothing"
else
  ok "enumerated $(wc -l <<< "$DECODERS") hand-rolled Claims decoder file(s) outside auth.rs"
  for f in $DECODERS; do
    if has_code "$f" -F -e 'sessions_revoked_at.read()'; then
      ok "  $(basename "$f") enforces session revocation on its own decode path"
    else
      bad "  $(basename "$f") decodes a JWT by hand and never checks sessions_revoked_at — panic cannot close this channel"
    fi
  done
fi

# The response used to hardcode a flat success for the terminal kill while
# DISCARDING the agent's Result, so an unreachable agent and a successful kill
# produced the same reassurance. What the agent reports is what gets reported.
#
# NEGATIVE ARM: a match means BAD. Before the strip it fired on an explanatory
# comment that merely QUOTED the forbidden field — a false alarm on correct code,
# the same defect as comment-blindness with the sign flipped. §8 plants the field
# as real code and requires this to fire.
if grep -qE '"terminals_killed": true' <<< "$PANIC"; then
  bad "panic still asserts terminals_killed:true regardless of what the agent did"
else
  ok "panic does not assert a kill it did not observe"
fi
if grep -q "agent_reached" <<< "$PANIC"; then
  ok "and it says whether the agent was reached at all"
else
  bad "an unreachable agent is indistinguishable from a successful kill"
fi

# A terminal share is an UNAUTHENTICATED public URL — the shared-terminal route
# is registered in the no-auth block and its handler takes no auth extractor —
# serving up to 500KB of terminal output for an hour. Lockdown does not reach it
# (lockdown is tested at the doors; this is not a door), so before v2.66.0 panic
# left the intruder's own share serving to the whole internet while the UI said
# "all sessions revoked". Keyed on the DELETE, not on the key name, which also
# appears in the comment above the statement.
if grep -qE "DELETE FROM settings WHERE key LIKE 'terminal_share_%'" <<< "$PANIC"; then
  ok "panic revokes every terminal share"
else
  bad "panic leaves public terminal shares serving — the one channel lockdown cannot close"
fi
if grep -q "shares_revoked" <<< "$PANIC"; then
  ok "and it reports how many it revoked"
else
  bad "shares are revoked but the count is dropped — the operator cannot tell"
fi

# The agent half: killing by unix user is a GUESS about which processes are ours,
# and it guessed wrong in the one direction that matters — a SERVER terminal is
# deliberately kept as root, so the shell most worth killing was the one process
# the control could not reach.
AGENT_KILL=$(kill_body "$AGENT_SEC")
if [ -z "$AGENT_KILL" ]; then
  bad "could not extract kill_terminals — the arms below are measuring nothing"
else
  ok "extracted kill_terminals — the arms below read real code"
fi
if grep -q "TERMINAL_PIDS" <<< "$AGENT_KILL"; then
  ok "the kill walks a registry of shells this agent started, rather than guessing by user"
else
  bad "the kill still guesses by user and command name, so a root server terminal survives it"
fi
# NEGATIVE ARM: a match means BAD. §8 plants the real counter-reset and requires
# this to fire.
if grep -qE 'ACTIVE_TERMINALS\.swap\(0' <<< "$AGENT_KILL"; then
  bad "the session counter is zeroed without killing — an attacker gets a fresh full quota"
else
  ok "the counter is not zeroed independently of the kill"
fi

# Both ends of the registry, because either alone leaks: register without forget
# grows for ever and kills reaped pids; forget without register kills nothing.
if has_code "$TERM_SRC" -F -e 'register_terminal(child_pid' && has_code "$TERM_SRC" -F -e 'forget_terminal(child_pid'; then
  ok "every spawned shell is registered on open and forgotten on close"
else
  bad "the pid registry is written at only one end — it either leaks or kills nothing"
fi

echo
echo "── s308: the sessions screen its own guide describes ──"

# Three endpoints, complete and caller-scoped, with zero callers in the shipped
# bundle — while the sessions guide told a user to click controls that were not
# there, as the response to a suspected compromise.
TSX_FILES=$(find panel/frontend/src -name '*.tsx' | sort)
for ep in "auth/sessions" "export-my-data"; do
  # NOT `grep -rc … || echo 0`: grep exits 1 on no match, so the fallback
  # APPENDS a second zero and the numeric test dies with a bash error. The arm
  # then goes red for a reason that has nothing to do with the defect, which is
  # a false negative wearing a false positive's clothes. Count, don't fall back.
  #
  # ⚠ This counted the settings page ALONE until v2.83.0, which pinned the
  # SUBJECT AT AN ADDRESS: extracting these controls into their own component —
  # so a non-admin could finally reach them — made the arm go red over a tree
  # where the screen had just become MORE built. The property is "the shipped
  # bundle calls it", so count the tree.
  #
  # ⚠⚠ And counting the tree RAW counted commented-out calls. All three real call
  # sites were commented out at s338 and both arms stayed green on a screen that
  # no longer talked to the backend at all. Per-file through code_lines now.
  n=0
  for f in $TSX_FILES; do n=$((n + $(code_count "$f" -F -e "$ep"))); done
  if [ "${n:-0}" -ge 1 ]; then
    ok "the panel calls $ep — the guide's screen exists"
  else
    bad "$ep still has no caller; the guide documents a screen that is not built"
  fi
done

# The guide's own corrected claims. Each of these was false at v2.64.2.
#
# READ RAW, AND THAT IS THE POINT. These three arms assert English sentences in a
# markdown document — prose is the mechanism here, not a disguise for one.
# Stripping would be the defect: the shell marker blanks a leading hash, which in
# markdown is a heading, not a comment. §6 pins that so the exemption stays
# visibly deliberate and §9 keeps the guide out of the raw-read scan on purpose.
if grep -qiE "^There is no location column|does not geolocate" "$SESSDOC"; then
  ok "the guide no longer promises a Location column the schema cannot hold"
else
  bad "the guide still claims a Location column; user_sessions has no such field"
fi
if grep -qi "panel-wide, not account-wide" "$SESSDOC"; then
  ok "the guide says Revoke All is panel-wide, not 'everywhere except this session'"
else
  bad "the guide still describes Revoke All as sparing your current session"
fi
if grep -qiE "does \*\*not\*\* bind a session to its originating IP" "$SESSDOC"; then
  ok "the guide no longer claims session IP binding or concurrent-session limits"
else
  bad "the guide still claims IP binding / concurrent limits; neither setting has ever existed"
fi

echo
echo "── §6 the strippers strip comments, and nothing else (s338) ──"

# Everything above now depends on code_lines. A stripper that under-strips
# restores the whole defect silently; one that OVER-strips turns the three
# negative arms green on broken code. So it gets its own fixtures, and they are
# hermetic — no repository file is involved, because a fixture that shares a
# subject with the thing it validates cannot tell the two apart.

cat > "$FIXDIR/f.sh" <<'FIX'
#!/usr/bin/env bash
# TOKENWORD lives here as a full-line comment
run_the TOKENWORD --now
echo "a literal hash # inside a string keeps TOKENSTR"
FIX

cat > "$FIXDIR/f.rs" <<'FIX'
// TOKENWORD in a line comment
/*
 * TOKENWORD in a block comment
 */
let live = call(TOKENWORD);
let trail = other(); // TOKENWORD trailing
let url = "https://example.invalid/TOKENURL";
let glob = "COPY /app/target/release/*";
let after_glob = keep(TOKENAFTER);
FIX

cat > "$FIXDIR/f.tsx" <<'FIX'
// TOKENTSX in a line comment
const live = load(TOKENTSX);
const path = "a//b/TOKENPATH";
FIX

printf '{ "name": "x", "note": "TOKENWORD" }\n' > "$FIXDIR/f.json"

printf '# TOKENMD as a markdown heading\nan ordinary TOKENMD sentence\n' > "$FIXDIR/f.md"

if [ "$(code_count "$FIXDIR/f.sh" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "shell: a full-line '#' comment carrying the token is blanked (1 code hit, not 2)"
else
  bad "shell: expected exactly 1 code hit for TOKENWORD, got $(code_count "$FIXDIR/f.sh" -F -e 'TOKENWORD')"
fi

if [ "$(code_count "$FIXDIR/f.sh" -F -e 'TOKENSTR')" -eq 1 ]; then
  ok "shell: a '#' inside a string literal is left alone (no trailing-comment stripping)"
else
  bad "shell: the stripper ate a '#' inside a string — a negative arm would now pass on broken code"
fi

if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "rust: '//' line, '/* */' block and trailing '//' all blanked (1 code hit of 4)"
else
  bad "rust: expected exactly 1 code hit for TOKENWORD, got $(code_count "$FIXDIR/f.rs" -F -e 'TOKENWORD')"
fi

if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENURL')" -eq 1 ]; then
  ok "rust: '//' inside a quoted string survives — a URL is not a comment"
else
  bad "rust: the stripper cut a line at the '//' of a URL inside a string literal"
fi

# s294: a `/*` inside a string literal opened a block comment that ran to the
# next close marker and deleted 485 lines of git_build.rs. The guard is that a
# block is recognised only where one is WRITTEN — opening at line start.
if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENAFTER')" -eq 1 ]; then
  ok "rust: a '/*' inside a string literal does not open a block (the s294 guard holds)"
else
  bad "rust: a string containing '/*' swallowed the code after it — the s294 stripper defect is back"
fi

if [ "$(code_count "$FIXDIR/f.tsx" -F -e 'TOKENTSX')" -eq 1 ] && [ "$(code_count "$FIXDIR/f.tsx" -F -e 'TOKENPATH')" -eq 1 ]; then
  ok "tsx: the '//' rules apply to the frontend subjects too, strings included"
else
  bad "tsx: expected 1 code hit each for the comment token and the string token, got $(code_count "$FIXDIR/f.tsx" -F -e 'TOKENTSX') and $(code_count "$FIXDIR/f.tsx" -F -e 'TOKENPATH')"
fi

if [ "$(code_count "$FIXDIR/f.json" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "json: nothing is stripped, because JSON has no comment syntax"
else
  bad "json: the stripper altered a JSON file, which has no comments to remove"
fi

# Why the guide is exempt, pinned rather than asserted in a comment: a markdown
# heading starts with the shell comment marker, so routing docs through the
# stripper would delete exactly the lines the guide's arms assert.
MD_CODE=$(code_count "$FIXDIR/f.md" -F -e 'TOKENMD')
MD_RAW=$(grep -c 'TOKENMD' "$FIXDIR/f.md" || true)
if [ "$MD_CODE" -eq 1 ] && [ "$MD_RAW" -eq 2 ]; then
  ok "markdown: stripping would blank an ATX heading — which is why the guide's arms read it raw, on purpose"
else
  bad "markdown: expected raw 2 / stripped 1 for the heading fixture, got raw $MD_RAW / stripped $MD_CODE — the guide's raw exemption no longer has a reason"
fi

# The body slicers cut ranges out of this stream and one arm reports the height
# of what it cut. Deleting rather than blanking would renumber every subject.
LINEDRIFT=""
for f in "$AUTH_RS" "$OAUTH_RS" "$PASSKEYS_RS" "$SEC_RS" "$SET_RS" "$WS_RS" \
         "$HELPERS" "$HEALER" "$SET_TSX" "$AGENT_SEC" "$TERM_SRC"; do
  a=$(wc -l < "$f"); b=$(code_lines "$f" | wc -l)
  [ "$a" != "$b" ] && LINEDRIFT="$LINEDRIFT $f($a≠$b)"
done
if [ -z "$LINEDRIFT" ]; then
  ok "blanking preserves the line count of all 11 code subjects, so every slice boundary still lands where it did"
else
  bad "code_lines changed the line count of:$LINEDRIFT — the body slicers are now cutting the wrong ranges"
fi

echo
echo "── §7 every pinned pattern is live CODE, and a comment cannot satisfy it (s338) ──"

# This is the s338 mutation, encoded. A round of mutation testing done by hand
# dies with the session that did it; the arms below re-run it on every push.
#
# For each subject: assert every pattern this suite pins is present in CODE (a
# pattern that has quietly become prose-only would otherwise turn red later and
# look like a test bug rather than the defect it is), then turn each matching
# code line into a comment in a throwaway copy and assert the count falls to
# zero. If it does not, some arm above is still reading the file raw.
PINNED=(
  "$AUTH_RS|-F|enforce_panel_ip_allowlist"
  "$AUTH_RS|-F|is_locked_down"
  "$AUTH_RS|-F|allowed_panel_ips"
  "$OAUTH_RS|-F|enforce_panel_ip_allowlist"
  "$OAUTH_RS|-F|is_locked_down"
  "$PASSKEYS_RS|-F|enforce_panel_ip_allowlist"
  "$PASSKEYS_RS|-F|is_locked_down"
  "$HELPERS|-F|fn panel_ip_allowed"
  "$HEALER|-F|record_suspicious_event_at"
  "$HEALER|-F|event[\"timestamp\"]"
  "$SET_RS|-F|recording_coverage"
  "$SET_TSX|-F|recording-coverage"
  "$SEC_RS|-F|sessions_revoked_at"
  "$SEC_RS|-F|agent_reached"
  "$SEC_RS|-F|shares_revoked"
  "$SEC_RS|-F|DELETE FROM settings WHERE key LIKE 'terminal_share_%'"
  "$WS_RS|-F|decode::<Claims>"
  "$WS_RS|-F|sessions_revoked_at.read()"
  "$AGENT_SEC|-F|TERMINAL_PIDS"
  "$TERM_SRC|-F|register_terminal(child_pid"
  "$TERM_SRC|-F|forget_terminal(child_pid"
)

# Turn every CODE line matching the pattern into a comment, leaving the text.
# That is the defect this section is about: the mechanism removed, the
# explanation left behind.
#
# The copy KEEPS the original's extension, and that is load-bearing rather than
# tidy: comment_marker_for dispatches on the extension, so a scratch file called
# `mutated` is read as shell no matter what it holds. The path is flattened into
# the basename because two of these subjects share one — the backend and the
# agent each own a security.rs — and a shared scratch name would let one
# subject's copy answer for the other's.
scratch_name() { printf '%s' "$1" | tr '/' '_'; }

comment_out_matches() {
  local src="$1" dst="$2" mode="$3" pat="$4" marker
  marker=$(comment_marker_for "$src")
  code_lines "$src" | grep -n "$mode" -e "$pat" 2>/dev/null | cut -d: -f1 > "$FIXDIR/hits" || true
  # Keyed on FILENAME, not `NR == FNR`: with an EMPTY hit list (a pin that has gone
  # prose-only) the first record of the SUBJECT also satisfies NR == FNR, the whole
  # file is consumed as hit numbers, and the empty copy then reads as "the stripper
  # is not reaching this subject" when the truth is "nothing left to comment out".
  awk -v marker="$marker" -v hitsfile="$FIXDIR/hits" \
      'FILENAME == hitsfile { hit[$1]=1; next } { if (FNR in hit) print marker " " $0; else print }' \
      "$FIXDIR/hits" "$src" > "$dst"
}

PIN_SUBJECTS=$(printf '%s\n' "${PINNED[@]}" | cut -d'|' -f1 | sort -u)
for subj in $PIN_SUBJECTS; do
  dead="" survived=""
  for entry in "${PINNED[@]}"; do
    f=${entry%%|*}; rest=${entry#*|}; mode=${rest%%|*}; pat=${rest#*|}
    [ "$f" = "$subj" ] || continue
    [ "$(code_count "$f" "$mode" -e "$pat")" -gt 0 ] || dead="$dead [$pat]"
    mutated="$FIXDIR/mutated_$(scratch_name "$f")"
    comment_out_matches "$f" "$mutated" "$mode" "$pat"
    # Assert the plant LANDED before reading the result: an empty copy would
    # score zero and read exactly like a stripper that works.
    if [ ! -s "$mutated" ]; then
      survived="$survived [$pat→copy-empty]"
      continue
    fi
    left=$(code_count "$mutated" "$mode" -e "$pat")
    [ "$left" -eq 0 ] || survived="$survived [$pat→$left]"
  done

  if [ -z "$dead" ]; then
    ok "$subj: every pattern this suite pins is present in CODE, not only in prose"
  else
    bad "$subj: pinned pattern(s) present only as prose —$dead — the arm reading it is passing on a comment"
  fi

  if [ -z "$survived" ]; then
    ok "$subj: commenting out the matching code lines drives every pinned pattern to zero"
  else
    bad "$subj: pattern(s) still counted after their code lines were commented out —$survived — the stripper is not reaching this subject"
  fi
done

# The two endpoint arms count a TREE, not a named file, so they get the same
# treatment at tree scope: find the files that carry each call in code, then
# comment those code lines out in copies and require the tree-wide count to fall
# to zero. Naming the component here would re-introduce the v2.83.0 defect of
# pinning the subject at an address.
EP_DEAD="" EP_SURVIVED=""
for ep in "auth/sessions" "export-my-data"; do
  carriers=""
  for f in $TSX_FILES; do
    [ "$(code_count "$f" -F -e "$ep")" -gt 0 ] && carriers="$carriers $f"
  done
  if [ -z "$carriers" ]; then
    EP_DEAD="$EP_DEAD [$ep]"
    continue
  fi
  left=0
  for f in $carriers; do
    mutated="$FIXDIR/ep_$(scratch_name "$f")"
    comment_out_matches "$f" "$mutated" -F "$ep"
    if [ ! -s "$mutated" ]; then
      EP_SURVIVED="$EP_SURVIVED [$ep→copy-empty]"
      continue
    fi
    left=$((left + $(code_count "$mutated" -F -e "$ep")))
  done
  [ "$left" -eq 0 ] || EP_SURVIVED="$EP_SURVIVED [$ep→$left]"
done
if [ -z "$EP_DEAD" ]; then
  ok "frontend tree: both session-screen endpoints are called from CODE, not from a commented-out call site"
else
  bad "frontend tree: endpoint(s) present only as prose —$EP_DEAD — the screen's arms are passing on a comment"
fi
if [ -z "$EP_SURVIVED" ]; then
  ok "frontend tree: commenting out the call sites drives both endpoint counts to zero"
else
  bad "frontend tree: endpoint(s) still counted after their call sites were commented out —$EP_SURVIVED"
fi

echo
echo "── §8 the negative arms still fire on a REAL defect (s338) ──"

# Three arms in this suite are NEGATIVE: a match means BAD. Stripping makes every
# check see LESS, and for a negative arm seeing less is SILENTLY GREEN on broken
# code — the direction that ships. A positive arm that over-strips goes loudly
# red and gets fixed the same hour; a negative one just stops objecting.
#
# So each of the three gets a control that plants the REAL defect, as code, in a
# copy, and drives it through the SAME predicate the arm uses. If a control stops
# firing, the stripper has started eating code and the arm above it is worthless.
# Controls: 3. Negative arms: 3. Those two numbers must stay equal.

insert_after() {  # <src> <anchor-regex> <line> <dst>
  local src="$1" anchor="$2" line="$3" dst="$4"
  awk -v anchor="$anchor" -v ins="$line" \
      '$0 ~ anchor && !done { print; print ins; done = 1; next } { print }' "$src" > "$dst"
}

# NC1 — §3's inline re-implementation detector. A route file that reads the
# settings key itself, in code, must be reported.
cat > "$FIXDIR/ctl_route.rs" <<'CTL'
pub async fn fake_second_door() {
    let list = settings_get(&state, "allowed_panel_ips").await;
}
CTL
if [ -n "$(inline_allowlist_files "$FIXDIR/ctl_route.rs")" ]; then
  ok "§3's duplicate-gate detector still fires on a real inline lookup in a route file"
else
  bad "§3's duplicate-gate detector no longer sees a real inline lookup — the stripper is eating code and a second gate would pass unnoticed"
fi

# NC2 — the panic response arm. Anchored at column 0 so the narrative comment
# 400 lines above the handler cannot become the insertion point; if it did, the
# planted line would land outside the slice and the control would look broken
# while the arm was fine.
insert_after "$SEC_RS" '^pub async fn panic_button' \
  '    let _ctl = json!({ "terminals_killed": true });' "$FIXDIR/ctl_panic.rs"
CTL_PANIC=$(panic_body "$FIXDIR/ctl_panic.rs")
if [ -n "$CTL_PANIC" ] && grep -qE '"terminals_killed": true' <<< "$CTL_PANIC"; then
  ok "the panic-response arm still fires when the hardcoded claim is really in code"
else
  bad "the panic-response arm no longer fires on a real hardcoded claim — it is now green by blindness, not by correctness"
fi

# NC3 — the agent's counter-reset arm.
insert_after "$AGENT_SEC" '^async fn kill_terminals' \
  '    let _ctl = ACTIVE_TERMINALS.swap(0, Ordering::SeqCst);' "$FIXDIR/ctl_kill.rs"
CTL_KILL=$(kill_body "$FIXDIR/ctl_kill.rs")
if [ -n "$CTL_KILL" ] && grep -qE 'ACTIVE_TERMINALS\.swap\(0' <<< "$CTL_KILL"; then
  ok "the counter-reset arm still fires when the reset is really in code"
else
  bad "the counter-reset arm no longer fires on a real reset — an attacker's fresh quota would pass unnoticed"
fi

echo
echo "── §9 no arm in this suite may read a code subject raw again (s338) ──"

# The structural guard. §7 proves today's arms are comment-blind; this one stops
# a NEW arm from being written the old way, which is exactly how this suite came
# to read every subject raw for sixty-one sessions.
SELF=tests/auth-doors-pin-e2e.sh

# Assembled at runtime from a variable, so this line does not contain the shape
# it searches for and the scan cannot match itself. The guide is absent from the
# list on purpose: it is prose by design and §6 pins the reason.
SUBJ_VARS='AUTH_RS|OAUTH_RS|PASSKEYS_RS|SEC_RS|SET_RS|WS_RS|HELPERS|HEALER|SET_TSX|AGENT_SEC|TERM_SRC|BE|ROUTES'
RAW_ARMS=$(code_lines "$SELF" | grep -nE "(grep|sed|perl|awk)[^|;]*\"\\\$($SUBJ_VARS)[\"/]" || true)
if [ -z "$RAW_ARMS" ]; then
  ok "no arm reads a code subject directly; every read goes through code_lines"
else
  bad "these lines read a code subject raw instead of through code_lines: $(printf '%s' "$RAW_ARMS" | cut -d: -f1 | tr '\n' ' ')"
fi

# s336 #364: a check placed below an early exit inherits that exit's blind spot.
# The sibling suite's strippers were the same shape — defined at line 379, used
# by nothing above them. Assert the definition precedes the first section rather
# than trusting it.
DEF_LINE=$(code_lineno "$SELF" -E -e '^code_lines\(\) \{')
SEC1_LINE=$(code_lineno "$SELF" -F -e '── discovery ──')
if [ -n "$DEF_LINE" ] && [ -n "$SEC1_LINE" ] && [ "$DEF_LINE" -lt "$SEC1_LINE" ]; then
  ok "code_lines is defined (line $DEF_LINE) above the first section (line $SEC1_LINE)"
else
  bad "code_lines is no longer defined above the first section — the arms above its definition are reading raw source again (def=${DEF_LINE:-none} first=${SEC1_LINE:-none})"
fi

echo
if [ "$FAIL" -eq 0 ]; then
  printf '\033[0;32mauth-doors pins: %d passed\033[0m\n' "$PASS"
  exit 0
else
  printf '\033[0;31mauth-doors pins: %d passed, %d FAILED\033[0m\n' "$PASS" "$FAIL"
  exit 1
fi
