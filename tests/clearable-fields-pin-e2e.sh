#!/usr/bin/env bash
# Regression pins for issue #117 — an optional field that cannot be emptied.
#
# THE DEFECT, as reported by Sylvain-SW against 2.132.1:
#   Clearing any optional field of a Git Deploy and saving returned 200, and the
#   value was still there on reload. The form sent `null` for an empty box:
#
#       post_deploy_cmd: formPostDeploy.trim() || null,        (GitDeploys.tsx)
#       post_deploy_cmd = COALESCE($12, post_deploy_cmd),      (git_deploys.rs)
#
#   COALESCE(NULL, old) is old, so "the client omitted this key" and "the operator
#   emptied this box" arrived identically and the second silently became the
#   first. A post-deploy hook the operator had removed went on running.
#
# THE CLASS. A derivation over the crate found 125 COALESCE self-guarded columns
# in 16 UPDATE statements across 13 files. Most are correct and must stay: an
# agent check-in that omits a metric must not blank the stored one, and a NOT NULL
# identifier has no "clear" to express. What makes a genuine defect is the PAIR —
# a column self-guarded on the UPDATE side AND a form field that sends null for an
# empty box. §1 derives both halves and pins the intersection, so a new pairing
# fails here whether it arrives from the SQL side or from the form side.
#
# THE FIX SHAPE, pinned by §2. Three states on the wire instead of two:
#
#       key absent      -> NULL -> COALESCE keeps the stored value
#       key sent as ""  -> ''   -> CASE writes NULL
#       key sent with v -> v    -> CASE writes v
#
#   The empty string is a WIRE sentinel and is never stored. v2.120.0 fixed the
#   same defect on the alert destinations by letting '' reach the column, which is
#   safe THERE because every reader of those columns guards on non-empty. It is
#   not safe here: `remove` reads the domain with `unwrap_or(&name)`, so a stored
#   '' would hand the agent an empty site identifier. §3 pins the normaliser that
#   keeps '' out of storage, and §4 pins the two readers that would otherwise ask
#   the agent for a vhost named ".conf" or an ACME order with no address.
#
# §5 pins the refusal. Clearing a DOMAIN is answered rather than folded away,
# because nothing in the git path takes an nginx vhost down — `unexpose_domain`
# exists for Docker apps and git_build.rs names wiring it as separate work.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }

# ugrep's --ignore-files shim honours .gitignore, so use the real binary.
G=/usr/bin/grep

GD=panel/backend/src/routes/git_deploys.rs
GDUI=panel/frontend/src/pages/GitDeploys.tsx
BACKEND=panel/backend/src/routes
SPA=panel/frontend/src/pages

for f in "$GD" "$GDUI"; do
  [ -f "$f" ] || { bad "subject present" "$f is missing"; }
done

echo "── §1 the class: a COALESCE self-guard must not meet a null-for-empty form field"

# Both halves are DERIVED. A hand-written list of subjects is the defect this arm
# exists to catch, so neither side may be enumerated by name.
#
# Backend half: the columns a placeholder is COALESCEd onto. Matched on the
# COALESCE itself rather than on the assignment, so BOTH shapes
# are in the population: the plain `col = COALESCE($n, col)` and the three-state
# `col = CASE WHEN $n = '' THEN NULL ELSE COALESCE($n, col) END`. That matters —
# under the CASE form a null STILL means "leave it alone", so a form field that
# sends null for an empty box is just as broken against a fixed column as
# against an unfixed one, and an arm that only knew the plain shape would go
# quiet on exactly the columns this release repaired.
guarded=$("$G" -rhoE 'COALESCE\(\$[0-9]+, [a-z_]+\)' "$BACKEND" --include=*.rs \
  | sed -E 's/^COALESCE\(\$[0-9]+, ([a-z_]+)\)$/\1/' | sort -u)

# Frontend half: payload keys sent as `key: <expr> || null`, carrying the FILE
# and — load-bearing — HOW MANY TIMES.
#
# The count is not decoration. Most of these pages post the same key from BOTH a
# create payload and an update payload, and only the update one is a defect: a
# create has no stored value to fold onto. `col@file` alone cannot tell them
# apart, so an allow-list entry earned by the legitimate create would silently
# excuse the update regrowing one — which a mutation proved, by reinstating
# `logo_url: logoUrl || null` on the reseller UPDATE and leaving this arm green.
# Pinning the count closes it: the create occurrence is the one that is expected,
# and a second occurrence in the same file is the regression.
nulled=$("$G" -rnoE '[a-z_]+: [^,;]*\|\| null' "$SPA" --include=*.tsx \
  | sed -E 's#^.*/([A-Za-z]+\.tsx):[0-9]+:([a-z_]+):.*#\2@\1#' \
  | sort | uniq -c | awk '{print $2 "=" $1}')

gcount=$(printf '%s\n' "$guarded" | "$G" -c . || true)
ncount=$(printf '%s\n' "$nulled"  | "$G" -c . || true)

# A derivation that returns nothing passes every membership test ever written.
if [ "$gcount" -lt 20 ] || [ "$ncount" -lt 5 ]; then
  bad "§1-derivation both populations are non-empty" \
      "derived $gcount guarded columns and $ncount null-for-empty keys — the greps stopped matching"
else
  ok "§1-derivation both populations are non-empty ($gcount guarded columns, $ncount null-for-empty keys)"
fi

pairs=$(printf '%s\n' "$nulled" \
  | while IFS= read -r entry; do
      col=${entry%@*}
      # NOT `... | "$G" -qx ...` — `-q` closes the read end on its first
      # match, and `$guarded` is 100+ lines: a race between that early close
      # and printf still writing turns into a broken-pipe failure here that
      # has nothing to do with membership (CI hit it; this box did not in 8
      # runs). Draining the match fully avoids the race the same way this
      # project's own `has()`/`hasre()` helpers already do elsewhere.
      [ -n "$(printf '%s\n' "$guarded" | "$G" -x -- "$col")" ] && printf '%s\n' "$entry"
    done | sort -u | tr '\n' ' ' | sed 's/ *$//')

# Every surviving pair needs a written reason. Keep this list SHORT — a pair
# belongs here only when clearing the field is not a thing an operator can mean.
#
#   ── on a CREATE payload, where null is the correct spelling of "not set".
#      A create has no stored value to preserve, so there is nothing to fold onto
#      and nothing an operator could be trying to clear.
#        domain@Apps.tsx, ssl_email@Apps.tsx            compose-stack create
#        panel_name@Resellers.tsx, logo_url@Resellers.tsx,
#        accent_color@Resellers.tsx                     reseller create (the
#                                                       UPDATE beside it sends
#                                                       the operator's blank)
#        allowed_images@ContainerPolicies.tsx           policy create, ditto
#        alert_slack_url@Monitors.tsx, alert_discord_url@Monitors.tsx,
#        keyword@Monitors.tsx                           monitors have no edit form
#                                                       at all — `api.put` there
#                                                       sends only { enabled }
#   ── deliberate on an UPDATE, and the only ones:
#        github_token@GitDeploys.tsx   the GET masks a stored token, so the box is
#                                      blank on every ordinary edit; "" would mean
#                                      "cleared" on a form that clears itself, and
#                                      every save would delete the token.
#        domain@Apps.tsx (2nd),        s414's Stack EDIT modal (PUT /stacks/{id}),
#        ssl_email@Apps.tsx (2nd)      alongside the pre-existing CREATE occurrence
#                                      of each. Both verified SAFE against the
#                                      ACTUAL target column — stacks.rs, not the
#                                      git_deploys.rs column this arm's bare
#                                      column-name matching pairs them with:
#                                        domain    — UpdateStackRequest.domain is
#                                                     Option<Option<String>> via
#                                                     explicit_option, genuinely
#                                                     three-state at the struct
#                                                     level (absent=keep, explicit
#                                                     null=vacate, per the field's
#                                                     own doc comment); stacks.rs's
#                                                     UPDATE has no COALESCE on
#                                                     domain at all ("no COALESCE,
#                                                     so the statement says exactly
#                                                     what the row will hold").
#                                        ssl_email — UpdateStackRequest.ssl_email
#                                                     really is "absent (or an
#                                                     explicit null) = keep stored"
#                                                     — but that is INTENTIONAL,
#                                                     stated in TlsPlan's own doc
#                                                     comment: "an edit that says
#                                                     nothing about the address
#                                                     keeps it, and a stack
#                                                     switched to none and back to
#                                                     acme finds it again." Not the
#                                                     #117 shape (an operator who
#                                                     WANTS to clear it silently
#                                                     cannot): the round-trip is
#                                                     the point.
#
# Each entry pins a COUNT, because file granularity alone is not enough: several
# of these pages carry a create AND an update payload, and an allow-list entry
# earned by the create would otherwise excuse the update regrowing one. Most
# expected counts are 1 — the create occurrence — so any OTHER count in the same
# file fails here unless written above. `domain@GitDeploys.tsx` must never appear
# at all: that clear is REFUSED rather than folded away (§5).
EXPECTED_PAIRS="accent_color@Resellers.tsx=1 alert_discord_url@Monitors.tsx=1 alert_slack_url@Monitors.tsx=1 allowed_images@ContainerPolicies.tsx=1 domain@Apps.tsx=2 github_token@GitDeploys.tsx=1 keyword@Monitors.tsx=1 logo_url@Resellers.tsx=1 panel_name@Resellers.tsx=1 ssl_email@Apps.tsx=2"
eq "§1 the only null-for-empty fields left on a COALESCE-guarded column are the ten with written reasons" \
   "$pairs" "$EXPECTED_PAIRS"

echo
echo "── §2 the three-state contract, pinned per column"

# Keyed on the whole operation in its closed form. A bare column name would be
# satisfied by the comment block above the statement (s381 #627).
for col in ssl_email pre_build_cmd post_deploy_cmd deploy_cron; do
  n=$("$G" -c "$col = CASE WHEN \$[0-9]* = '' THEN NULL ELSE COALESCE(\$[0-9]*, $col) END" "$GD" || true)
  eq "§2 $col distinguishes an absent key from an emptied box" "$n" "1"
done

# The reverted form must be gone, not merely outnumbered.
for col in ssl_email pre_build_cmd post_deploy_cmd deploy_cron; do
  n=$("$G" -cE "^ +$col = COALESCE\(\\\$[0-9]+, $col\), \\\\$" "$GD" || true)
  eq "§2 $col no longer carries the plain self-guard" "$n" "0"
done

echo
echo "── §3 the empty string is a wire sentinel and is never stored"

n=$("$G" -c 'fn blank_to_none' "$GD" || true)
eq "§3 the normaliser exists" "$n" "1"

# create and update post the SAME payload object, so create receives the blanks
# too. Each of the four columns must go through the normaliser on the way in.
for col in ssl_email pre_build_cmd post_deploy_cmd deploy_cron; do
  n=$("$G" -c "blank_to_none(body\.$col\.as_deref())" "$GD" || true)
  eq "§3 create normalises a blank $col to NULL" "$n" "1"
done

# The domain arm had the same defect and is easy to reintroduce, because the
# obvious `None => body.domain.clone()` reads as a no-op.
n=$("$G" -c 'None => body\.domain\.clone()' "$GD" || true)
eq "§3 create does not round-trip a blank domain back into the column" "$n" "0"

echo
echo "── §4 blank is absent at the two readers that hand a value to the agent"

n=$("$G" -c 'b\.domain\.filter(|d| !d\.trim()\.is_empty())' "$GD" || true)
eq "§4 a blank domain does not become a vhost named '.conf'" "$n" "1"
n=$("$G" -c 'b\.ssl_email\.filter(|e| !e\.trim()\.is_empty())' "$GD" || true)
eq "§4 a blank ssl_email does not open an ACME order with no account address" "$n" "1"

echo
echo "── §5 removing a domain is refused, and the refusal says why"

n=$("$G" -c 'Removing the domain is not supported yet' "$GD" || true)
eq "§5 the refusal exists" "$n" "1"
# It must name the domain it is refusing to drop; a generic sentence sends the
# operator looking for a setting instead of telling them what is in the way.
n=$("$G" -c 'cur_domain\.as_deref()\.unwrap_or("")' "$GD" || true)
eq "§5 the refusal names the vhost that would be left behind" "$n" "1"

# And it must only fire when a domain is actually stored — a deploy that never
# had one submits "" on every ordinary save, and refusing those would make the
# form unusable for every domainless deploy.
# Whitespace-insensitive on purpose. The first cut of this arm spelled the chain
# on one line; rustfmt then wrapped it across four and turned my own arm red for
# a change that altered nothing (s374 #585). Match the OPERATION with the
# newlines and indentation squeezed out, so the formatter cannot move it.
flat=$(tr -d ' \n' < "$GD")
n=$(printf '%s' "$flat" | "$G" -c 'cur_domain.as_deref().map(|c|!c.trim().is_empty()).unwrap_or(false)' || true)
eq "§5 the refusal is conditional on a domain being stored" "$n" "1"

echo
echo "── §6 the form sends the operator's blank rather than a null"

for f in domain ssl_email pre_build_cmd post_deploy_cmd deploy_cron; do
  n=$("$G" -cE "^ +$f: form[A-Za-z]+\.trim\(\),$" "$GDUI" || true)
  eq "§6 $f is sent as typed" "$n" "1"
done
# The one that must NOT be converted, and the reason is in the file beside it.
n=$("$G" -c 'github_token: formGithubToken\.trim() || null,' "$GDUI" || true)
eq "§6 github_token still preserves on blank, because the GET masks it" "$n" "1"

echo

if [ "$FAIL" -eq 0 ]; then
  printf '\033[32mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL"
else
  printf '\033[31mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL"
fi
[ "$FAIL" -eq 0 ]
