#!/usr/bin/env bash
# mail-sender-identity-pin-e2e.sh — s352
#
# Pins the binding between a SASL login and the envelope senders it may use.
#
# WHY THIS SUITE EXISTS. Until v2.106.0 nothing tied the authenticated login to
# MAIL FROM, so any mailbox on the box could send as any address on any other
# tenant's hosted domain — and because OpenDKIM's signing table is keyed on the
# sender domain alone (`*@{domain}`), the forgery left the box carrying a VALID
# DKIM signature for the victim's domain, so DMARC passed on both legs. The
# co-tenant population that exploits this is self-service since v2.102.0, when
# nine mail handlers changed from the administrator extractor to the
# authenticated-user one.
#
#   §0  the suite's own subjects exist and its stripper did not eat them.
#   §A  the MAP — built, not borrowed. Reusing either existing Postfix map would
#       manufacture the defect, so the arms here refuse both re-pointings.
#   §B  the WILDCARD row — the load-bearing one. Without a per-domain row the
#       control only covers addresses that are already mailboxes, which is the
#       smaller half of the surface and none of the DKIM-signed part.
#   §C  the ORDER — map, then hash, then the key that names it. Getting this
#       backwards takes smtpd off the air on 25 and 587, so the arms pin the
#       checked postmap and the early return, not merely the calls.
#   §D  the RESTRICTION FORM — authenticated-only, and mynetworks first.
#   §E  the CARRIERS — both paths that reach a box, including the one that
#       reaches a box already running mail.
#   §F  the UNIT TESTS exist and name the property, so deleting the map builder
#       cannot quietly take its coverage with it.
#
# Pure source analysis: no box, no network, no build.

set -u
cd "$(dirname "$0")/.."
REPO=$(pwd)

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

AGENT="$REPO/panel/agent/src/routes/mail.rs"

[ -f "$AGENT" ] || { echo "FATAL: missing subject $AGENT"; exit 1; }

# Strip comments before measuring. A pin greps RAW SOURCE, so without this every
# arm below would match the sentence that DESCRIBES the check rather than the
# check itself — and this subject is heavily commented precisely because the
# ordering reasoning is the load-bearing part. The block-comment rule only
# recognises one opening at line start and closing at line end: a `/*` inside a
# string literal once opened a comment that ran for hundreds of lines and made
# every absence arm pass on code the stripper had removed.
code() {
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*///.*$}{}gm;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

# PRODUCTION code only. A `#[cfg(test)]` module is not a call site, and an arm
# that counted both would pass on the strength of the tests written beside it.
prod() { perl -0777 -pe 's/#\[cfg\(test\)\]\s*mod\s+\w+\s*\{.*//s' <<< "$1"; }

has()         { grep -qF -- "$2" <<< "$1"; }
hasre()       { grep -qE -- "$2" <<< "$1"; }
occurrences() { grep -oF -- "$2" <<< "$1" | wc -l; }

# Extract a function body by scanning to the next TOP-LEVEL item — NOT to the
# next `}` in column 0.
#
# This subject embeds whole config files as raw string literals, and the Dovecot
# one contains a bare `}` in column 0 (the closing brace of its `passdb {`
# block). A non-greedy `.*?\n\}` stops THERE, 200 lines early, and hands every
# later arm a truncated function. That is not hypothetical: the first draft of
# §E reported "mail_install does not arm enforcement" about code that arms it on
# the very next line after the truncation point. Same family as a comment
# stripper that opens a block comment on a `/*` inside a string literal — the
# instrument reads the embedded document as if it were code.
# The name is passed through the ENVIRONMENT, not through perl's `-s` switch
# parsing: `-s` without `-n` gives no implicit read loop, so `$_` stays empty and
# the extractor returns nothing for every input. That is a silent empty, and an
# arm fed an empty body reports the absence of whatever it looks for — which is
# why the §0 arms below assert a token from each function's TAIL rather than
# merely that something came back.
fn_body() {
  FN_NAME="$2" perl -0777 -ne '
    my $n = $ENV{FN_NAME};
    if (/(async fn \Q$n\E\b.*?)(?=\n(?:pub |async fn |fn |const |struct |enum |impl |#\[))/s) {
      print $1;
    }
  ' <<< "$1"
}

AGENT_RAW=$(cat "$AGENT")
AGENT_CODE=$(code "$AGENT")
AGENT_PROD=$(prod "$AGENT_CODE")
AGENT_TESTS=$(perl -0777 -ne 'print $1 if /(#\[cfg\(test\)\]\s*mod\s+tests\s*\{.*)/s' <<< "$AGENT_CODE")

echo "── mail-sender-identity-pin-e2e ─────────────────────────────────────"

# ── §0 the suite measures what it thinks it measures ────────────────────
echo "§0 the instrument"

# An absence arm over a truncated subject passes for the wrong reason, and it
# fails in the reassuring direction. Prove the stripper left the file usable.
raw_lines=$(wc -l <<< "$AGENT_RAW")
code_lines=$(wc -l <<< "$AGENT_CODE")
prod_lines=$(wc -l <<< "$AGENT_PROD")
if [ "$code_lines" -gt $((raw_lines / 2)) ]; then
  ok "the comment stripper kept over half the file ($code_lines of $raw_lines lines)"
else
  bad "the comment stripper ate the subject ($code_lines of $raw_lines lines) — every absence arm below is void"
fi
if [ "$prod_lines" -lt "$code_lines" ] && [ "$prod_lines" -gt $((code_lines / 2)) ]; then
  ok "the test module was separated from production code ($prod_lines prod of $code_lines)"
else
  bad "the cfg(test) split is wrong ($prod_lines prod of $code_lines) — §A-§E may be reading tests"
fi
# Positive control: a token that certainly exists must be found by the same
# machinery the absence arms use. Without this an arm can report a clean absence
# because the haystack is empty.
if has "$AGENT_PROD" 'ensure_sender_login_enforcement'; then
  ok "positive control: the enforcement helper is visible to these arms"
else
  bad "positive control FAILED — the arms below are reading nothing"
fi
# Negative control: a token that certainly does NOT exist must not be found.
if has "$AGENT_PROD" 'zzz_not_a_real_token_zzz'; then
  bad "negative control FAILED — the matcher reports hits that are not there"
else
  ok "negative control: an absent token is reported absent"
fi

# The function extractor must reach the END of a function, not stop at the first
# `}` it meets inside an embedded config string. §E's verdicts are worthless if
# it does, and they fail in BOTH directions: a truncated body reports a call
# absent that is present, and would equally report an absence it never reached.
# So assert a token known to sit near the tail of each subject function.
_inst=$(fn_body "$AGENT_PROD" mail_install)
_sync=$(fn_body "$AGENT_PROD" sync_config)
if [ -n "$_inst" ] && has "$_inst" 'write_opendkim_config'; then
  ok "the extractor reaches the end of mail_install ($(wc -l <<< "$_inst") lines)"
else
  bad "mail_install extraction is truncated ($(wc -l <<< "$_inst") lines) — §E cannot be believed"
fi
if [ -n "$_sync" ] && has "$_sync" 'Mail config synced'; then
  ok "the extractor reaches the end of sync_config ($(wc -l <<< "$_sync") lines)"
else
  bad "sync_config extraction is truncated ($(wc -l <<< "$_sync") lines) — §E cannot be believed"
fi

# ── §A the map is built, not borrowed ───────────────────────────────────
echo "§A the map"

if has "$AGENT_PROD" 'const POSTFIX_SENDER_LOGIN'; then
  ok "a dedicated sender-login map path exists"
else
  bad "no dedicated sender-login map — the control has nothing to read"
fi

# The two re-pointings that would MANUFACTURE the defect. virtual_mailbox_maps
# values are maildir paths, so that one refuses every authenticated send;
# virtual_alias_maps values are forward targets, so a mailbox that sets
# forwarding loses the right to send as itself and the forward target gains it —
# and a tenant could grant another tenant send-as rights by creating an alias.
for wrong in POSTFIX_VIRTUAL_MAILBOX POSTFIX_VIRTUAL_ALIAS; do
  if hasre "$AGENT_PROD" "smtpd_sender_login_maps=hash:\{$wrong\}"; then
    bad "smtpd_sender_login_maps points at $wrong — that map's VALUES are not login names"
  else
    ok "smtpd_sender_login_maps does not borrow $wrong"
  fi
done

if hasre "$AGENT_PROD" 'smtpd_sender_login_maps=hash:\{POSTFIX_SENDER_LOGIN\}'; then
  ok "the restriction reads the purpose-built map"
else
  bad "the sender-login map is not the one the restriction names"
fi

if has "$AGENT_PROD" 'fn build_sender_login_map'; then
  ok "the map builder is a named function, so it can be tested without a box"
else
  bad "no map builder function — the table is unreachable from a unit test"
fi

# ── §B the wildcard row ─────────────────────────────────────────────────
echo "§B the per-domain row"

# ⚠ This comment used to say the map is consulted permissively — that an address
# it does not mention is allowed through, making the per-domain row the thing
# that closed the hole. The parameter's own manual page says the opposite: the
# authenticated form refuses a SASL client whose MAIL FROM "is not listed in
# $smtpd_sender_login_maps". The table is an ALLOW-LIST.
#
# So the row is not what stops a forge — an invented local part at another
# tenant's domain is unlisted and refused with or without it. The row is what
# keeps a domain's OWN logins able to send as their aliases, catch-alls and role
# addresses. Deleting it would not open a hole; it would break every alias on
# the box. Same arm, opposite reason, and the reason is the part that has to be
# right, because it is what the next person edits against.
if hasre "$AGENT_PROD" 'lines\.push\(format!\("@\{\}'; then
  ok "a per-domain row is emitted, so a domain's own logins keep their aliases"
else
  bad "no per-domain row — a mailbox could send only as the exact address it authenticated with, and every alias on the box would stop working"
fi

# The line above pins that the row is WRITTEN; this pins that the loop writing it
# actually runs over the domains. A grep cannot see reachability, and the first
# draft proved it: neutering the loop's filter to `|_d| false` left every §B arm
# green because the push statement was still there to be matched. The unit tests
# catch that case; this arm is what makes the suite catch it too.
if has "$AGENT_PROD" 'for domain in body.domains.iter().filter(|d| d.enabled) {'; then
  ok "the per-domain loop runs over enabled domains, so the row is reached as well as written"
else
  bad "the per-domain loop no longer iterates enabled domains — the row above may be unreachable"
fi

# Pin the USE, not the name. The first draft grepped the bare token and stayed
# green when the use site was replaced with an empty string, because the `const`
# DECLARATION still carries the token — an arm matching a different occurrence
# of the thing it is named after. Require both, so deleting either reddens.
sentinel_uses=$(occurrences "$AGENT_PROD" 'NO_AUTHORISED_SENDER')
if [ "$sentinel_uses" -ge 2 ] && has "$AGENT_PROD" 'NO_AUTHORISED_SENDER.to_string()'; then
  ok "a domain with no mailbox is owned by an unmatchable sentinel ($sentinel_uses occurrences: declaration + use)"
else
  bad "the sentinel is declared but not used ($sentinel_uses occurrences) — a domain holding no mailbox is left unowned, so anyone with a login elsewhere may send as it"
fi

# Exactness, not suffix. `contains` would make a subdomain's tenant an owner of
# the parent domain, which is a tenant crossing.
if has "$AGENT_PROD" 'a.email.ends_with(&suffix)'; then
  ok "domain membership is an exact-domain test, so a subdomain is not its parent"
else
  bad "domain membership is not the exact-domain test — a subdomain tenant may own the parent"
fi

if has "$AGENT_PROD" 'body.accounts.iter().filter(|a| a.enabled)'; then
  ok "disabled mailboxes own nothing (they hold no login either)"
else
  bad "disabled accounts are not filtered out of the owner lists"
fi

# ── §C the order ────────────────────────────────────────────────────────
echo "§C map, then hash, then the key"

# A hash: table named by an smtpd restriction is opened when smtpd initialises.
# A missing or stale .db makes smtpd exit fatal and master throttle it — a total
# outage on 25 and 587, not a per-message deferral. So the postmap result must
# be CHECKED, and the keys written only after it.
if hasre "$AGENT_PROD" 'Ok\(out\) if out\.status\.success\(\)'; then
  ok "the postmap result is checked, not discarded"
else
  bad "the postmap result is not checked — the restriction can be armed against a map Postfix cannot open"
fi

# The four pre-existing postmap calls deliberately discard their status; the new
# one must not be allowed to join them. Count the discarding form so a future
# edit that relaxes this arm's subject is visible.
# FIVE legacy call sites discard their status — install x2 (the two virtual
# maps), sync x2 (the same pair), and the relay's sasl_passwd. The sender-login
# map must never join them, so pin the number rather than the absence: a sixth
# discarding call is either this map losing its check or a new map arriving
# without one, and both deserve a red.
discarded=$(occurrences "$AGENT_PROD" 'let _ = safe_command("postmap")')
if [ "$discarded" -eq 5 ]; then
  ok "exactly the five legacy postmap calls discard their status ($discarded)"
else
  bad "expected 5 status-discarding postmap calls, found $discarded — the sender-login map may have become one of them"
fi

# The early return is the fail-safe. Without it a failed hash still arms the key.
helper=$(fn_body "$AGENT_PROD" ensure_sender_login_enforcement)
if [ -z "$helper" ]; then
  bad "could not extract the enforcement helper — §C's remaining arms cannot fire"
else
  ok "the enforcement helper was extracted ($(wc -l <<< "$helper") lines)"
  returns=$(occurrences "$helper" 'return;')
  if [ "$returns" -ge 3 ]; then
    ok "every failure path returns before arming the restriction ($returns)"
  else
    bad "only $returns early returns — a failure can fall through and arm the key anyway"
  fi
  # Order within the helper: the map write and hash must precede the keys.
  map_at=$(grep -nF 'postmap' <<< "$helper" | head -1 | cut -d: -f1)
  key_at=$(grep -nF 'smtpd_sender_restrictions' <<< "$helper" | head -1 | cut -d: -f1)
  if [ -n "$map_at" ] && [ -n "$key_at" ] && [ "$map_at" -lt "$key_at" ]; then
    ok "the map is hashed (line $map_at) before the restriction is written (line $key_at)"
  else
    bad "the restriction is written before the map is hashed — this is the ordering that takes smtpd down"
  fi
  # The trap the refuted premise hid. An allow-list armed against a table with no
  # rows authorises nobody, so it refuses EVERY authenticated submission on the
  # box — 553, no other symptom. `mail_install` calls this helper with whatever
  # the map file already held, and on a box upgrading into the release that
  # introduced that file it holds nothing. Re-running the installer is the
  # ordinary response to mail looking broken, so without this guard the retry
  # path takes the box's own authenticated mail off the air until the next
  # mailbox change. Pin that the guard exists AND that it precedes the key: a
  # check placed after the arming is decoration.
  empty_at=$(grep -nE 'map_content\.trim\(\)\.is_empty\(\)' <<< "$helper" | head -1 | cut -d: -f1)
  if [ -n "$empty_at" ] && [ -n "$key_at" ] && [ "$empty_at" -lt "$key_at" ]; then
    ok "a map that authorises nobody is refused (line $empty_at) before the restriction is written (line $key_at)"
  else
    bad "nothing stops an empty map from arming the restriction — every authenticated submission on the box would be refused with 553"
  fi
fi

# ── §D the restriction form ─────────────────────────────────────────────
echo "§D the restriction"

# The AUTHENTICATED form. The bare reject_sender_login_mismatch also refuses
# UNAUTHENTICATED inbound mail on 25 whose envelope sender is a hosted address —
# ordinary for a mailing list, a forwarder, or a tenant's external SaaS.
if has "$AGENT_PROD" 'reject_authenticated_sender_login_mismatch'; then
  ok "the authenticated-only form is used, so inbound port-25 mail is untouched"
else
  bad "the authenticated-only form is not used — legitimate inbound mail can be refused"
fi

# The arm this suite got WRONG first, and the one a source pin could never have
# settled on its own. The first cut wrote `permit_mynetworks, reject_…` on the
# theory that it protected local mail. Driven on a real two-tenant box that
# version accepted every forge: `mynetworks_style = host` still covers
# 127.0.0.0/8, and a permit matches before the reject is reached — so on a
# hosting box, where "from this machine" means "from any tenant's own code",
# the permit disarmed the control completely. It also protected nothing:
# `sendmail` submission is non_smtpd and never reaches these restrictions,
# measured by delivering mail with an unowned -f address after removal.
if hasre "$AGENT_PROD" 'smtpd_sender_restrictions=[^"]*permit_mynetworks'; then
  bad "permit_mynetworks precedes the sender check — it matches first for anything connecting from the box, which on a hosting machine is every tenant's own code, and the control is inert"
else
  ok "no permit_mynetworks in front of the sender check, so a local authenticated session is checked like any other"
fi
if hasre "$AGENT_PROD" 'smtpd_sender_login_maps=proxy:'; then
  bad "the map is opened through proxymap — measured unnecessary (plain hash: works; smtpd opens the table before it chroots)"
else
  ok "the map is a plain hash:, which smtpd opens before chrooting"
fi

# It must be smtpd_sender_restrictions, not the recipient list. Appending to
# smtpd_recipient_restrictions is a guaranteed no-op: permit_sasl_authenticated
# short-circuits it, on both the main.cf value and the submission override.
if hasre "$AGENT_PROD" 'smtpd_recipient_restrictions=[^"]*sender_login'; then
  bad "the check was appended to smtpd_recipient_restrictions — permit_sasl_authenticated short-circuits it"
else
  ok "the check is not hidden behind permit_sasl_authenticated in the recipient list"
fi

# ── §E the carriers ─────────────────────────────────────────────────────
echo "§E delivery"

calls=$(occurrences "$AGENT_PROD" 'ensure_sender_login_enforcement(')
if [ "$calls" -ge 3 ]; then
  ok "the helper is defined and called from both carriers ($calls occurrences)"
else
  bad "only $calls occurrences — one of the two carriers does not arm enforcement"
fi

sync_fn="$_sync"
if [ -n "$sync_fn" ] && has "$sync_fn" 'ensure_sender_login_enforcement'; then
  ok "sync_config arms enforcement, so a box already running mail repairs itself on the next mailbox change"
else
  bad "sync_config does not arm enforcement — no existing install would ever receive this"
fi

# What this carrier is actually for, now that the helper refuses an empty map:
# preserving and re-arming an EXISTING map across an installer re-run. A box with
# no mailboxes has nothing to protect and nobody who can authenticate, and its
# first mailbox goes through sync_config, which arms it there.
install_fn="$_inst"
if [ -n "$install_fn" ] && has "$install_fn" 'ensure_sender_login_enforcement'; then
  ok "mail_install re-arms enforcement, so an installer re-run keeps an existing map armed"
else
  bad "mail_install does not arm enforcement — an installer re-run would leave an existing map unenforced"
fi

# The keys must NOT be added to the appended config block: that block is written
# once and skipped for ever after, so a template edit reaches new installs only.
block=$(perl -0777 -ne 'print $1 if /(# DockPanel mail configuration.*?"#\))/s' <<< "$AGENT_RAW")
if [ -z "$block" ]; then
  bad "could not extract the main.cf template block — this arm cannot fire"
elif grep -qF 'smtpd_sender_restrictions' <<< "$block"; then
  bad "the restriction was put in the write-once main.cf block — it would reach no existing install"
else
  ok "the restriction is written by postconf, not frozen into the write-once block"
fi

# ── §F the unit tests ───────────────────────────────────────────────────
echo "§F the property is unit-tested"

if [ -z "$AGENT_TESTS" ]; then
  bad "no test module found — §F cannot fire"
else
  ok "the test module was extracted ($(wc -l <<< "$AGENT_TESTS") lines)"
  for t in a_tenant_may_not_send_as_another_tenants_domain \
           an_invented_local_part_at_a_hosted_domain_is_still_owned \
           a_domain_with_no_mailbox_is_owned_by_nobody \
           a_subdomain_is_not_its_parent_domain \
           a_disabled_mailbox_owns_neither_itself_nor_its_domain; do
    if has "$AGENT_TESTS" "fn $t("; then
      ok "unit test present: $t"
    else
      bad "unit test missing: $t — the map builder lost coverage of that case"
    fi
  done
fi

echo
echo "PASS $PASS FAIL $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
