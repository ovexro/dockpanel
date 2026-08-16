#!/usr/bin/env bash
# mail-client-view-pin-e2e.sh — s349 / GitHub #106, the frontend half
#
# Pins the CONSOLE in front of the boundary `mail-authorisation-pin-e2e.sh`
# pins in the backend.
#
# WHY THIS SUITE EXISTS, measured rather than asserted: v2.102.0 shipped the
# backend half of #106 and left the panel unchanged, so the state was "live API,
# hidden UI". At s349 the frontend half was measured and found to be pinned by
# NOTHING: `grep -rn 'pages/Mail\.tsx' tests/` returned 0 across all 59 suites
# (control: `pages/Settings.tsx` returns 6), and no arm anywhere read the /mail
# nav row. Both suites that parse `navItems.ts` — client-account-door and
# client-role-honesty — stay green whether that row is `adminOnly`, unrestricted,
# or deleted outright. There was nothing to edit; this had to be written.
#
# THE SHAPE OF THE REGRESSION THIS GUARDS, and why the arms are written the way
# they are: the danger here is not deletion, it is a WIDENING (lesson #423 — the
# s347 pin asserted a gate "refuses everything else" by testing that the
# administrator comparison was PRESENT, and `role == "admin" || role == "client"`
# still contains it). So every arm below was checked against the widened form of
# its own subject, not against removal:
#
#   §A  the NAV ROW — the entry is unrestricted, and the row still EXISTS. An
#       absence test on a deleted row passes for the wrong reason, so A1 asserts
#       presence before A2 asserts what the row does not carry.
#   §B  the FETCH GATE — the eight host-level requests are behind the role test,
#       and `/mail/domains` is NOT. This is the arm that matters: hiding a panel
#       while still requesting it leaves eight 403s per page load, each swallowed
#       by its own `.catch` into an empty state.
#   §C  the RENDER GATES — every control whose endpoint the backend refuses.
#   §D  ANTI-WIDENING — the admin-only redirect has not come back, and the arms
#       above are not vacuous.
#   §E  the CROSS-TREE arm, and the reason this suite is worth more than the sum
#       of §A-§D: the frontend partition is asserted AGAINST THE BACKEND'S OWN
#       EXTRACTORS. A future session that flips a handler between AuthUser and
#       AdminUser turns this suite red in the other tree, which is the only way
#       two copies of one rule stay in step.
#
# Pure source analysis: no box, no network, no build.

set -u
cd "$(dirname "$0")/.."
REPO=$(pwd)

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

NAV="$REPO/panel/frontend/src/data/navItems.ts"
PAGE="$REPO/panel/frontend/src/pages/Mail.tsx"
BACK="$REPO/panel/backend/src/routes/mail.rs"

for f in "$NAV" "$PAGE" "$BACK"; do
  [ -f "$f" ] || { echo "FATAL: missing subject $f"; exit 1; }
done

# Strip comments before measuring. This matters more here than in most suites:
# the s349 change is heavily commented and those comments NAME the very things
# the arms below grep for — `AdminUser`, `create_domain`, `dns_check`,
# `mailbox_backup`. Grepping raw source would match the sentence that explains
# the check instead of the check ([[feedback_source_pin_prose_trap]]).
#
# The house `code()` in the sibling suites handles `///`, `//` and bare `/* */`.
# It does NOT handle the JSX comment form `{/* ... */}`, which is what a .tsx
# file uses inside markup and what every new comment in Mail.tsx is written as.
# Handled first, non-greedily, before the C-style pass.
code() {
  perl -0777 -pe '
    s{\{\s*/\*.*?\*/\s*\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*///.*$}{}gm;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

has()         { grep -qE -- "$2" <<< "$1"; }
occurrences() { grep -oE -- "$2" <<< "$1" | wc -l; }

# The nav row for a path, read as a WHOLE LINE. Same idiom as
# client-account-door-pin-e2e.sh: read the ROW's own text, never the file's —
# navItems.ts is full of `adminOnly` and a file-level grep proves nothing.
navrow() { grep -F -- "{ to: \"$1\"," <<< "$2" | head -1; }

NAV_C=$(code "$NAV")
PAGE_C=$(code "$PAGE")
BACK_C=$(code "$BACK")

# ── STRIPPER SELF-CHECK ────────────────────────────────────────────────────────
# Lesson #136: the shared stripper was DELETING CODE for months because a `/*`
# inside a string literal opened a block comment that ran to the next `*/`, and a
# truncated subject makes every ABSENCE arm pass on code the stripper merely
# removed — it fails in the reassuring direction. So the stripper is measured
# before anything is measured with it.
echo "§0 stripper"
NAV_ROWS_RAW=$(grep -c 'to: "/' "$NAV")
NAV_ROWS_STRIPPED=$(occurrences "$NAV_C" 'to: "/')
if [ "$NAV_ROWS_RAW" -eq "$NAV_ROWS_STRIPPED" ] && [ "$NAV_ROWS_RAW" -ge 20 ]; then
  ok "0.1 stripper preserved all $NAV_ROWS_RAW nav rows"
else
  bad "0.1 stripper changed the nav row count ($NAV_ROWS_RAW raw vs $NAV_ROWS_STRIPPED stripped)"
fi

PAGE_CALLS_RAW=$(grep -c 'api\.\(get\|post\|put\|delete\)' "$PAGE")
PAGE_CALLS_STRIPPED=$(occurrences "$PAGE_C" 'api\.(get|post|put|delete)')
if [ "$PAGE_CALLS_STRIPPED" -eq "$PAGE_CALLS_RAW" ] && [ "$PAGE_CALLS_RAW" -ge 25 ]; then
  ok "0.2 stripper preserved all $PAGE_CALLS_RAW api calls in Mail.tsx"
else
  bad "0.2 stripper changed the api-call count ($PAGE_CALLS_RAW raw vs $PAGE_CALLS_STRIPPED stripped)"
fi

# And prove it actually REMOVED the JSX comments, or §B/§C are measuring prose.
JSX_COMMENT_TOKENS_RAW=$(grep -c 'AdminUser' "$PAGE" || true)
if [ "$JSX_COMMENT_TOKENS_RAW" -gt 0 ] && ! has "$PAGE_C" 'AdminUser'; then
  ok "0.3 stripper removed the JSX comments naming AdminUser (raw has them, stripped does not)"
else
  bad "0.3 stripper did not remove the JSX comments — §B and §C would grep prose"
fi

# ── §A THE NAV ROW ─────────────────────────────────────────────────────────────
echo "§A nav row"

MAILROW=$(navrow /mail "$NAV_C")
if [ -n "$MAILROW" ]; then
  ok "A1 navItems declares a /mail row"
else
  bad "A1 no /mail row in navItems — every arm below is vacuous"
fi

# A2's widened form is the row REGAINING a restriction, which this catches. Its
# vacuous form is the row being DELETED, which A1 catches first. Both directions
# are covered only because the two arms are read together.
# ⚠ Every restriction flag the registry grows must be listed here — an absence arm
# only sees the vocabulary it was taught. `resellerOnly` joined for #112.
if [ -n "$MAILROW" ] && ! has "$MAILROW" 'adminOnly' && ! has "$MAILROW" 'resellerVisible' && ! has "$MAILROW" 'resellerOnly'; then
  ok "A2 the /mail row carries no restriction flag"
else
  bad "A2 the /mail row is restricted — the backend admits any site owner, so the sidebar now contradicts the API"
fi

# The control. Without it A2 would pass on a file where NO row carries a flag,
# i.e. on a registry whose vocabulary had been removed wholesale.
if has "$(navrow /settings "$NAV_C")" 'adminOnly'; then
  ok "A3 /settings is still adminOnly (so A2 is a real distinction, not an empty vocabulary)"
else
  bad "A3 /settings is no longer adminOnly — A2 no longer distinguishes anything"
fi

# ── §B THE FETCH GATE ──────────────────────────────────────────────────────────
echo "§B fetch gate"

# The host-level effect body: from the role test to the end of that useEffect.
# Bounded on the effect's own closing `}, [isAdmin]);` rather than on a fixed
# -A window, because a fixed window is not a declaration (lesson #172) and this
# body is long enough that any budget would eventually run out of slack.
ADMIN_EFFECT=$(perl -0777 -ne 'print $1 if /if \(!isAdmin\) return;(.*?)\}, \[isAdmin\]\);/s' <<< "$PAGE_C")

if [ -n "$ADMIN_EFFECT" ]; then
  ok "B1 the host-level mount effect is guarded by an isAdmin early return"
else
  bad "B1 no isAdmin-guarded mount effect — the host-level requests fire for every role"
fi

# The eight requests that must be INSIDE that guard. Each is AdminUser in the
# backend (§E proves that rather than trusting this list), so for any other role
# each is a 403 that its own `.catch` turns into an empty state.
HOST_GETS='/mail/status|/mail/storage|/mail/backups|/mail/rspamd/status|/mail/webmail/status|/mail/relay/status|/mail/rate-limit/status|/mail/tls/status'
MISSING=""
for e in loadMailStatus loadStorage loadBackups /mail/rspamd/status /mail/webmail/status /mail/relay/status /mail/rate-limit/status /mail/tls/status; do
  has "$ADMIN_EFFECT" "$(sed 's/[]\/$*.^[]/\\&/g' <<< "$e")" || MISSING="$MISSING $e"
done
if [ -z "$MISSING" ]; then
  ok "B2 all eight host-level requests sit inside the isAdmin guard"
else
  bad "B2 host-level requests OUTSIDE the isAdmin guard:$MISSING"
fi

# The counterpart, and the arm that stops the fix over-reaching: the one call
# every role may make must NOT be inside the guard, or the page shows a site
# owner nothing at all.
if ! has "$ADMIN_EFFECT" 'loadDomains'; then
  ok "B3 loadDomains is NOT inside the isAdmin guard (a site owner still gets their domains)"
else
  bad "B3 loadDomains moved inside the isAdmin guard — a site owner now sees an empty page"
fi

if has "$PAGE_C" 'useEffect\(\(\) => \{ loadDomains\(\); \}, \[\]\);'; then
  ok "B4 loadDomains fires on mount for every role"
else
  bad "B4 the unconditional loadDomains mount effect is gone"
fi

# The two admin-only tabs fetch on tab change, not on mount. They are not offered
# to anyone else, but a tab strip is a render decision and a render decision must
# never be the only thing standing between a caller and a request they cannot make.
if has "$PAGE_C" 'isAdmin && tab === "queue"' && has "$PAGE_C" 'isAdmin && tab === "logs"'; then
  ok "B5 the queue and logs tab effects carry their own role test"
else
  bad "B5 a tab effect can fire an AdminUser request without a role test"
fi

# ── §C THE RENDER GATES ────────────────────────────────────────────────────────
echo "§C render gates"

# The one that made this a defect rather than a cosmetic gap: this block links to
# /settings, which is adminOnly in the very registry §A reads, so for anyone else
# it is a call to action pointing at a door that is not on their map.
if has "$PAGE_C" 'isAdmin && mailStatus && !mailStatus\.installed'; then
  ok "C1 the Not-Installed banner (which links to the adminOnly /settings) is admin-gated"
else
  bad "C1 the Not-Installed banner can reach a non-admin — it sends them to a page they cannot see"
fi

if has "$PAGE_C" 'isAdmin && mailStatus\?\.installed'; then
  ok "C2 the host services grid is admin-gated by ROLE, not merely by mailStatus being null"
else
  bad "C2 the services grid is gated only by a fetch that did not happen"
fi

if has "$PAGE_C" 'isAdmin && backups\.length > 0'; then
  ok "C3 the mailbox-backups list is admin-gated by role"
else
  bad "C3 the backups list is gated only by an array that is empty for the wrong reason"
fi

# The tab strip: five tabs for an administrator, three for a site owner. Written
# as two literal tuples so the arm can see a widening — adding "queue" to the
# owner tuple changes the text this greps.
if has "$PAGE_C" '\["accounts", "aliases", "dns"\] as const' \
   && has "$PAGE_C" '\["accounts", "aliases", "dns", "queue", "logs"\] as const'; then
  ok "C4 the tab strip offers three tabs to a site owner and five to an administrator"
else
  bad "C4 the tab strip no longer distinguishes the two audiences"
fi

if has "$PAGE_C" 'isAdmin && \('; then
  ok "C5 at least one inline control is wrapped in an isAdmin conditional"
else
  bad "C5 no inline isAdmin conditionals — the per-control gates are gone"
fi

# Count them, because C5 answers "can this still do the right thing" and the
# question that matters is "can it now also do something else" (lesson #423).
# create_domain button, delete_domain button, dns_check button, mailbox_backup
# button — four controls whose endpoints the backend refuses a site owner.
INLINE=$(occurrences "$PAGE_C" 'isAdmin && \(|!isAdmin \? null')
if [ "$INLINE" -ge 4 ]; then
  ok "C6 four or more admin-only controls are individually gated (found $INLINE)"
else
  bad "C6 only $INLINE admin-only controls are gated — expected at least 4"
fi

# ── §D ANTI-WIDENING ───────────────────────────────────────────────────────────
echo "§D anti-widening"

# The redirect this change removed. If it comes back, every arm in §B and §C
# above stays green while the page is once again invisible to the people it was
# built for — the arms would be measuring code nobody reaches.
if ! has "$PAGE_C" '<Navigate'; then
  ok "D1 Mail.tsx no longer redirects by role — the page is reachable by every role"
else
  bad "D1 a <Navigate> is back in Mail.tsx — the frontend half has been reverted"
fi

# D1 is an absence arm, so it passes on an empty or truncated file. This is its
# non-vacuity proof.
if has "$PAGE_C" 'const isAdmin = user\?\.role === "admin"'; then
  ok "D2 Mail.tsx still reads the role (so D1 is about a removed redirect, not a missing file)"
else
  bad "D2 Mail.tsx no longer derives isAdmin — §B and §C cannot be measuring anything"
fi

# The empty state a site owner reaches. Sites.tsx states the rule in its own
# empty state: advice a role cannot act on is a broken promise. "Add a mail
# domain to get started" is exactly that for someone create_domain refuses.
if has "$PAGE_C" 'isAdmin \? \(' && has "$PAGE_C" 'Mail follows your sites'; then
  ok "D3 the empty state says something different to the role that cannot act on it"
else
  bad "D3 the empty state tells a site owner to do something the backend will refuse"
fi

# ── §E THE CROSS-TREE ARM ──────────────────────────────────────────────────────
# The frontend partition is asserted against the BACKEND's own extractors. Two
# copies of one rule drift unless something reads both.
echo "§E cross-tree"

extractor_of() {
  perl -0777 -ne 'print $1 if /pub async fn '"$1"'\s*\((.*?)\)\s*->/s' <<< "$BACK_C"
}

HOST_HANDLERS="mail_status mail_storage mailbox_backups rspamd_status webmail_status relay_status rate_limit_status tls_status get_queue mail_logs mailbox_backup blacklist_check dns_check create_domain delete_domain"
OWNER_HANDLERS="list_domains domain_dns list_accounts create_account update_account delete_account list_aliases create_alias delete_alias"

WRONG=""
for h in $HOST_HANDLERS; do
  sig=$(extractor_of "$h")
  has "$sig" 'AdminUser' || WRONG="$WRONG $h"
done
if [ -z "$WRONG" ]; then
  ok "E1 all 15 handlers the frontend gates are AdminUser in the backend"
else
  bad "E1 the frontend gates handlers that are NOT AdminUser:$WRONG — the gate is wider than the API"
fi

WRONG=""
for h in $OWNER_HANDLERS; do
  sig=$(extractor_of "$h")
  has "$sig" 'AuthUser' || WRONG="$WRONG $h"
done
if [ -z "$WRONG" ]; then
  ok "E2 all 9 handlers the frontend leaves open are AuthUser in the backend"
else
  bad "E2 the frontend leaves open handlers that are NOT AuthUser:$WRONG — the console promises what the API refuses"
fi

# Non-vacuity for §E: the extractor reader must actually be reading signatures.
# If `extractor_of` returned empty for everything, E1 and E2 would both fail —
# but a future edit could make it return something harmless for everything, and
# then both would pass. This is the arm that notices.
if has "$(extractor_of list_domains)" 'AuthUser' && ! has "$(extractor_of create_domain)" 'AuthUser'; then
  ok "E3 the extractor reader distinguishes AuthUser from AdminUser on two known handlers"
else
  bad "E3 the extractor reader cannot tell the two extractors apart — E1 and E2 prove nothing"
fi

echo
echo "PASS $PASS  FAIL $FAIL"
[ "$FAIL" -eq 0 ]
