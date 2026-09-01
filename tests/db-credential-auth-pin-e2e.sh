#!/usr/bin/env bash
# db-credential-auth-pin-e2e.sh — s323
#
# A credential the code TRANSMITS must be the thing that AUTHORISES the action.
#
# `databases::reset_password` sent `old_password` to the agent, the agent's route
# validated its length and charset, and the mysql/mariadb branch then threw it away
# and reached the account as container-root instead:
#
#     docker exec <container> mariadb -u root --skip-password -e "ALTER USER …"
#
# The doc comment above it stated the premise — "MariaDB root can auth via unix
# socket inside the container" — and that premise was never true of a container
# THIS CODEBASE creates. `create_database` passes MYSQL_RANDOM_ROOT_PASSWORD=yes,
# which gives root@localhost a random mysql_native_password and does NOT enable the
# unix_socket plugin. So the branch could not authenticate at all: every MariaDB
# password reset since the branch was written returned
# `ERROR 1045 … Access denied for user 'root'@'localhost' (using password: NO)`
# and surfaced as a 500. Measured on mariadb:11 with exactly that env, s323.
#
# Two failures wearing one coat, which is why the arms below come in two sections:
# the authority was missing (§A) AND the premise the missing authority rested on
# was contradicted by the container config in the same file (§B). Fixing only §A
# leaves the next author free to reach for container-root again for some other
# verb; §B is what makes that go red.
#
#   §A  THE RESET AUTHENTICATES AS THE TENANT. The password in the request body is
#       resolved into the connection, and the statement is one the account may run
#       on itself. Not "the identifier appears somewhere in the body" — WHERE THE
#       VALUE CAME FROM (lesson #275: a draft arm that accepted any body mentioning
#       the name printed GREEN over the exact defect it was written for).
#   §B  NOBODY REACHES FOR CONTAINER-ROOT. Tree-wide absence with a CONTROL string
#       proving the grep is non-vacuous, plus a positive pin on the container env
#       that makes container-root unavailable in the first place.
#   §C  ESCAPING IS PER-ENGINE. MariaDB treats `\` as an escape inside string
#       literals and PostgreSQL (standard_conforming_strings) does not, so one
#       shared helper silently corrupts one of the two.
#   §D  CONTEXT. Arms that must be green at BOTH tags, so a harness that measures
#       nothing cannot read as a pass.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  database credential authority — source pins (s323)"
echo "=============================================="
echo

PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# ⚠ NEVER call `bad` inside `cmd | while read`. A pipeline puts the loop in a
# SUBSHELL, so `FAIL=$((FAIL+1))` increments a copy that dies with it: the arm
# prints its ✗ marks, the summary still reads FAIL 0, and the suite exits 0.
# Found s322 on wrong-host-dispatch §C4 — the one arm whose entire job was
# reporting violations had been unable to fail the build since it was written.
# Use `while read; do …; done < <(printf …)` so the loop runs in THIS shell.

# Strip comments before matching. Without this every arm below could be satisfied
# by the prose that NARRATES it — and the header of this very file spells the
# defect it pins, which is precisely the trap.
code() {
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }

# Collapse a body to a single line. `grep` is line-oriented, so an arm that pins an
# ORDERED PAIR of builder calls — `.arg("-u")` immediately followed by `.arg(user)`,
# which is the whole point of asking WHO we connect as — cannot express it against
# the raw body. Flattening is what lets the arm assert adjacency rather than mere
# co-presence, and co-presence is exactly the weak form lesson #275 is about.
flat() { tr '\n' ' ' <<< "$1" | tr -s ' '; }

# One function body by brace balance. A fixed -A window is not a function: a
# regression that moves a line past the window makes the arm measure nothing.
fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn) && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

AGENT_DB_SVC=panel/agent/src/services/database.rs
AGENT_DB_RT=panel/agent/src/routes/database.rs
PANEL_DB=panel/backend/src/routes/databases.rs

for f in "$AGENT_DB_SVC" "$AGENT_DB_RT" "$PANEL_DB"; do
  [ -f "$f" ] || { echo "  FATAL: $f missing — wrong tree?"; exit 1; }
done

SVC=$(code "$AGENT_DB_SVC")
RT=$(code "$AGENT_DB_RT")
PNL=$(code "$PANEL_DB")

RESET=$(fnbody "$SVC" "reset_password")
RESET_FLAT=$(flat "$RESET")

# ── §A the reset authenticates as the tenant ────────────────────────────────
echo "§A  the reset authenticates as the tenant"

if [ -z "$RESET" ]; then
  bad "A0 could not extract reset_password — every arm in this section measures nothing"
else
  ok "A0 reset_password body extracted ($(wc -l <<< "$RESET") lines)"
fi

# A1 is the load-bearing arm. It does not ask whether `old_password` is MENTIONED
# — the defective version mentioned it too, in its own signature. It asks whether
# the value is RESOLVED INTO the connection the statement runs over.
# s445: the credential moved from a literal `-e MYSQL_PWD={old_password}`
# docker argv (world-readable via ps/proc for the child's lifetime — see
# `DockerEnvFile`) to a 0600 --env-file. Pin the NEW shape — old_password
# still has to reach the connection, just not through argv anymore.
if [ -z "$RESET" ]; then
  bad "A1 skipped — no body"
elif has "$RESET" '"MYSQL_PWD", *old_password'; then
  ok "A1 the mariadb branch resolves old_password into the connection (MYSQL_PWD via --env-file)"
else
  bad "A1 the mariadb branch does not authenticate with old_password — the caller's credential is decorative"
fi

if [ -z "$RESET" ]; then
  bad "A2 skipped — no body"
elif has "$RESET_FLAT" '\.arg\("-u"\) *\.arg\(user\)'; then
  ok "A2 it connects AS the tenant account, taking the user from the request"
else
  bad "A2 the mariadb branch does not connect as the tenant — it is authenticating as somebody else"
fi

# A3: the statement must be one the account may run ON ITSELF. `ALTER USER
# '<u>'@'%'` needs the CREATE USER privilege, so a branch that authenticates as
# the tenant AND uses it is broken in the other direction — it would fail for the
# very account it is resetting. SET PASSWORD with no FOR clause targets
# CURRENT_USER() and needs no privilege at all.
if [ -z "$RESET" ]; then
  bad "A3 skipped — no body"
elif has "$RESET" 'SET PASSWORD = PASSWORD\('; then
  ok "A3 the statement is the self-service form, which the tenant may run unprivileged"
else
  bad "A3 the mariadb statement is not the self-service form — a tenant-authenticated connection cannot run it"
fi

if [ -z "$RESET" ]; then
  bad "A4 skipped — no body"
elif has "$RESET" '"PGPASSWORD", *old_password'; then
  ok "A4 the postgres branch also resolves old_password into its connection (PGPASSWORD via --env-file)"
else
  bad "A4 the postgres branch stopped passing old_password — the sibling regressed"
fi

# ── §B nobody reaches for container-root ────────────────────────────────────
echo
echo "§B  nobody reaches for container-root"

# B1 is an ABSENCE arm, so it carries a CONTROL. An absence arm keyed on a
# spelling reads a DELETION as compliance (lesson #269); the control proves the
# file is being read at all, and the count-and-identify form proves the grep is
# not vacuously zero (lesson: COUNT occurrences, never assert zero blindly).
#
# ⚠ It greps COMMENT-STRIPPED source, not raw bytes. A pin that reads raw source
# matches the prose that narrates it, and the first draft of this arm survived a
# planted defect only because a `grep -v` line filter happened to catch the doc
# comment above `reset_password` — a filter that a trailing or block comment would
# have walked straight through. Strip first, then there is nothing to filter.
SKIPPW=0
while read -r f; do
  [ -n "$f" ] || continue
  n=$(code "$f" | grep -c -- '--skip-password' || true)
  SKIPPW=$((SKIPPW + n))
done < <(grep -rl -- '--skip-password' panel/agent/src panel/backend/src 2>/dev/null)
CONTROL=$(grep -rc -- 'docker' "$AGENT_DB_SVC" 2>/dev/null || echo 0)
if [ "$CONTROL" -eq 0 ]; then
  bad "B1 CONTROL failed — the grep found no 'docker' in $AGENT_DB_SVC, so it is measuring nothing"
elif [ "$SKIPPW" -eq 0 ]; then
  ok "B1 no code path passes --skip-password (control: $CONTROL 'docker' hits in the service, so the grep reads real bytes)"
else
  bad "B1 $SKIPPW code path(s) still pass --skip-password — container-root auth cannot work on a MYSQL_RANDOM_ROOT_PASSWORD container"
fi

if [ -z "$RESET" ]; then
  bad "B2 skipped — no body"
elif has "$RESET" '\.arg\("root"\)'; then
  bad "B2 reset_password still names root as a connection user"
else
  ok "B2 reset_password names no root account"
fi

# B3 pins the PREMISE, not the fix. This is the arm that makes a FUTURE reach for
# container-root go red: as long as the container is created with a random root
# password, no in-container path can authenticate as root, and any code that tries
# is wrong by construction. If somebody later gives the panel a known root
# password this arm fails and forces the trade-off to be re-argued rather than
# silently assumed — which is exactly what went wrong the first time.
CREATE=$(fnbody "$SVC" "create_database")
if [ -z "$CREATE" ]; then
  bad "B3 could not extract create_database — the premise arm measures nothing"
elif has "$CREATE" 'MYSQL_RANDOM_ROOT_PASSWORD=yes'; then
  ok "B3 the mariadb container is still created with a random root password (so container-root remains unreachable by design)"
else
  bad "B3 create_database no longer sets MYSQL_RANDOM_ROOT_PASSWORD=yes — re-argue every in-container auth decision before changing this"
fi

# ── §C escaping is per-engine ───────────────────────────────────────────────
echo
echo "§C  escaping is per-engine"

MESC=$(fnbody "$SVC" "mysql_string_escape")
PESC=$(fnbody "$SVC" "pg_string_escape")

if [ -z "$MESC" ] || [ -z "$PESC" ]; then
  bad "C1 one or both escape helpers are missing — a single shared helper corrupts one engine"
else
  ok "C1 the two engines have separate escape helpers"
fi

if [ -z "$MESC" ]; then
  bad "C2 skipped — no mysql helper"
elif has "$MESC" "replace\\('\\\\\\\\'" ; then
  ok "C2 the mariadb helper doubles backslashes (MariaDB treats \\ as an escape in string literals)"
else
  bad "C2 the mariadb helper no longer doubles backslashes — a password containing \\ is silently altered"
fi

if [ -z "$PESC" ]; then
  bad "C3 skipped — no postgres helper"
elif has "$PESC" "replace\\('\\\\\\\\'" ; then
  bad "C3 the postgres helper doubles backslashes — standard_conforming_strings means that STORES two"
else
  ok "C3 the postgres helper leaves backslashes alone, as standard_conforming_strings requires"
fi

# ── §D context — must be green at both tags ─────────────────────────────────
echo
echo "§D  context (green at both tags — a harness measuring nothing cannot pass)"

if has "$RT" 'fn reset_password'; then
  ok "D1 the agent still exposes a reset-password route"
else
  bad "D1 the agent reset-password route is gone — the arms above are measuring a deleted feature"
fi

# Pin the WIRE KEY, not the identifier. `old_password` also names a local binding
# from `get_db_info` two dozen lines up, so a bare match stays green after the JSON
# key is deleted — mutation-testing caught exactly that, and it is #275's shape
# ("mentions the identifier" is not "sends it") reappearing inside the pin written
# to police #275.
if has "$PNL" '"old_password": *old_password'; then
  ok "D2 the panel still sends old_password as a key in the agent body"
else
  bad "D2 the panel no longer maps old_password into the agent body — §A can no longer be satisfied"
fi

# D3 catches the NEXT instance rather than this one: a field added to the request
# struct and never read by the service. Its subjects are derived FROM SOURCE — the
# struct's own field list — because a hardcoded list stops covering the thing it was
# written for the moment somebody adds a field, and cannot report that it stopped.
#
# ⚠ Its honest scope limit, stated here rather than discovered later: it looks for
# the field ANYWHERE below the signature, so it would NOT have caught the defect
# this suite exists for. `old_password` was read by the postgres branch while the
# mariadb branch ignored it, and one read is all this arm requires. Per-branch
# authority is §A's job; D3 only guarantees nobody adds a wholly unread field.
STRUCT=$(sed -n '/struct ResetPasswordRequest/,/^}/p' <<< "$RT" | grep -oE '^\s+[a-z_]+:' | tr -d ' :')
# The enumeration must be asserted non-empty first: a parse that returns nothing
# would otherwise loop zero times and print a confident ✓ (lesson: an arm that
# enumerates its own subjects must prove the enumeration is real).
STRUCT_N=$(grep -c . <<< "$STRUCT")
if [ "$STRUCT_N" -lt 1 ]; then
  bad "D3 parsed 0 fields from ResetPasswordRequest — the enumeration is broken, this arm is measuring nothing"
else
  # Drop the signature: every parameter name appears there by construction, so
  # matching against the whole body would make this arm unfailable.
  RESET_BODY=$(sed -n '/^) ->/,$p' <<< "$RESET")
  UNREAD=""
  while read -r field; do
    [ -n "$field" ] || continue
    has "$RESET_BODY" "\\b$field\\b" || UNREAD="${UNREAD}${field}"$'\n'
  done < <(printf '%s\n' "$STRUCT")

  if [ -z "$UNREAD" ]; then
    ok "D3 all $STRUCT_N fields of ResetPasswordRequest are read below the signature"
  else
    while read -r f; do
      [ -n "$f" ] || continue
      bad "D3 '$f' is accepted by the route and never read by the service"
    done < <(printf '%s\n' "$UNREAD")
  fi
fi

# ── §E databases.rs takes its host from the row ─────────────────────────────
echo
echo "§E  every databases.rs handler resolves its host from the row"

# COVERAGE GAP CLOSED HERE, s323. `wrong-host-dispatch-pin-e2e.sh` §E3 requires
# the agent call to carry a ROW-DERIVED DOMAIN, and these handlers name a
# CONTAINER instead — so the whole module contributes zero members to that census
# and is guarded by no other. That is the module holding both this session's seed
# defect and s322's fix for the wrong-host family: the best-covered code in the
# tree by attention, and the least by any arm. Subjects are derived from SOURCE,
# so a handler joins by existing.
DBH=$(python3 - "$PANEL_DB" <<'PY'
import re, sys, pathlib
src = pathlib.Path(sys.argv[1]).read_text()
src = re.sub(r'^\s*///.*$', '', src, flags=re.M)
src = re.sub(r'^\s*//.*$', '', src, flags=re.M)
fns = [(m.start(), m.group(1)) for m in re.finditer(r'(?:pub )?(?:async )?fn ([a-z_0-9]+)\s*\(', src)]
for i, (s, n) in enumerate(fns):
    e = fns[i + 1][0] if i + 1 < len(fns) else len(src)
    body = src[s:e]
    if not re.search(r'agent\s*\n?\s*\.(?:post|get|put|delete)', body):
        continue
    resolves = bool(re.search(r'agent_for_site_server\(|site_agent_for_caller\(|\bfor_server\(', body))
    # A helper that ACCEPTS an already-resolved handle takes its provenance from
    # its caller rather than from a lookup of its own. Excluding it is a
    # narrowing, so E1b below pins every such helper's call sites — without that
    # arm this clause is a hole shaped exactly like the defect the census exists
    # to catch: extract the agent call into a helper and the census stops looking.
    takes_handle = bool(re.search(r'agent:\s*&[A-Za-z_:]*AgentHandle', body))
    kind = 'OK' if resolves else ('PARAM' if takes_handle else 'VIOL')
    print(f"{n}:{kind}")
PY
)
DBH_N=$(grep -c . <<< "$DBH" || true)
DBH_V=$(grep -c ':VIOL$' <<< "$DBH" || true)
DBH_P=$(grep -c ':PARAM$' <<< "$DBH" || true)

if [ "${DBH_N:-0}" -lt 5 ]; then
  bad "E1 censused only ${DBH_N:-0} agent-dispatching handlers in databases.rs — the extractor is broken, not the tree"
elif [ "${DBH_V:-0}" -eq 0 ]; then
  ok "E1 all $DBH_N agent-dispatching handlers in databases.rs resolve the host from the row ($DBH_P by parameter, pinned by E1b)"
else
  while read -r row; do
    [ -n "$row" ] || continue
    bad "E1 databases.rs::${row%:VIOL} dispatches to whatever host the CALLER named — its database may live on another machine"
  done < <(grep ':VIOL$' <<< "$DBH")
fi

# E1b — the other half of E1's PARAM clause.
#
# A helper excluded because it ACCEPTS a handle is only safe if everything that
# hands it one resolved that handle from the row. Without this arm, "takes a
# handle parameter" is a way to launder the very dispatch E1 exists to catch:
# extract the agent call into a helper and the census stops looking. The
# exclusion and this arm ship together or neither does.
PARAM_FNS=$(grep -E ':PARAM$' <<< "$DBH" | sed 's/:PARAM$//')
if [ -z "$PARAM_FNS" ]; then
  ok "E1b no handler takes a handle by parameter, so there is nothing to launder"
else
  E1B_BAD=""
  E1B_SEEN=0
  for fn in $PARAM_FNS; do
    while read -r site; do
      [ -n "$site" ] || continue
      E1B_SEEN=$((E1B_SEEN+1))
      case "$site" in *:UNRESOLVED) E1B_BAD="$E1B_BAD ${site%:UNRESOLVED}" ;; esac
    done < <(python3 tests/helpers/callers-resolve-host.py "$fn" \
               panel/backend/src/routes/databases.rs panel/backend/src/routes/sites.rs)
  done
  if [ "$E1B_SEEN" -eq 0 ]; then
    bad "E1b found no call site for the handle-taking helper(s) —$PARAM_FNS — the arm measured nothing"
  elif [ -n "$E1B_BAD" ]; then
    bad "E1b these call sites hand over a handle they did not resolve from the row:$E1B_BAD"
  else
    ok "E1b all $E1B_SEEN call site(s) of the handle-taking helper(s) resolve the host from the row"
  fi
fi

# ── §F credentials are not argv-exposed ─────────────────────────────────────
echo
echo "§F  credentials reach docker via --env-file, never via -e KEY=value"

# s445 (dockpanel-fanout completeness critic, agent-crate rotation): a
# `-e MYSQL_PWD=...`/`-e PGPASSWORD=...` docker argument is literal argv,
# world-readable via ps/`/proc/<pid>/cmdline` for the child's lifetime —
# verified live against this box's own /proc mount (no hidepid=). Absence
# arm with a CONTROL: the same files must still mention the credential
# env-var NAMES (now inside `DockerEnvFile::new` calls, not `-e` argv), so a
# grep finding zero of either measures a deleted feature, not a fix.
#
# ONE known exception, excluded by name rather than weakening the pattern:
# `create_database`'s `POSTGRES_PASSWORD={admin_password}` builds the `env`
# Vec bollard's `Docker::create_container` API sends over the Docker
# SOCKET — no `docker` subprocess, no argv, nothing `ps`/`/proc` can see.
# Mutation-tested: this arm DID catch the leak before this exclusion was
# added (F1 failed on `database.rs:60` until scoped out — same class of
# false-positive risk the exclusion documents, not a hole opened blind).
DB_FILES="panel/agent/src/services/database.rs panel/agent/src/services/database_backup.rs panel/agent/src/services/backup_verify.rs panel/agent/src/services/backup_drill.rs"
LEAK_PATTERN='(MYSQL_PWD|PGPASSWORD|MYSQL_ROOT_PASSWORD|POSTGRES_PASSWORD)=\{'
NAME_PATTERN='"(MYSQL_PWD|PGPASSWORD|MYSQL_ROOT_PASSWORD|POSTGRES_PASSWORD)"'
ARGV_LEAK=0
NAME_HITS=0
for f in $DB_FILES; do
  [ -f "$f" ] || continue
  c=$(code "$f")
  leak=$(grep -cE -- "$LEAK_PATTERN" <<< "$c" || true)
  if [ "$f" = "$AGENT_DB_SVC" ]; then
    create_leak=$(grep -cE -- "$LEAK_PATTERN" <<< "$(fnbody "$c" "create_database")" || true)
    leak=$((leak - create_leak))
  fi
  ARGV_LEAK=$((ARGV_LEAK + leak))
  names=$(grep -cE -- "$NAME_PATTERN" <<< "$c" || true)
  NAME_HITS=$((NAME_HITS + names))
done
if [ "$NAME_HITS" -eq 0 ]; then
  bad "F1 CONTROL failed — found zero credential env-var names across the DB files, so the grep is measuring nothing"
elif [ "$ARGV_LEAK" -eq 0 ]; then
  ok "F1 no credential is interpolated into a docker argv slot ($NAME_HITS env-var reference(s) across the DB files all route through --env-file)"
else
  bad "F1 $ARGV_LEAK credential(s) still interpolated directly into docker argv — visible via ps/proc for the child's lifetime"
fi

echo
echo "----------------------------------------------"
printf '  PASS %d  FAIL %d  SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo "----------------------------------------------"
echo

[ "$FAIL" -eq 0 ] || exit 1
exit 0
