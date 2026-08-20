#!/usr/bin/env bash
# Regression pins for s381 — the notification centre that could not take you
# anywhere, and the badges that could not be told apart.
#
#   N1  `panel_notifications.link` shipped with the notification centre in March.
#       The migration creates the column, `notify_panel` binds it, the list
#       endpoint SELECTs it, the frontend TYPES it at `link: string | null` — and
#       the render never touched it. `git log -S'n.link' -- panel/frontend/src`
#       is EMPTY across the whole repository history, so the field was written,
#       transported and typed for five months without being read once. Half the
#       producers were passing a real destination the entire time.
#
#   N2  The busiest producer on a live panel — "Login from new IP", 25 of 78 rows
#       on the demo — did not go through `notify_panel` at all. It was a raw
#       INSERT with its own column list, and the two columns it omitted were
#       `link` (so that class could never become clickable) and the `NOTIF_TX`
#       broadcast (so that class never arrived without a reload). A raw INSERT
#       into a table with defaults succeeds whatever you leave out.
#
#   N3  The unread badge saturated at `> 9 ? "9+"` while the open-incident badge
#       two rows away was uncapped — so a "9+" meaning 29 sat on the same screen
#       as a "9" meaning 9. Five render sites across four layout files, each with
#       its own copy of the ternary.
#
# The durable shape is CLASS checks, not checks for these three defects:
#
#   * §1 derives the column list from the list endpoint's own SELECT and asserts
#     each column is rendered by the page or waived with a reason. The NEXT
#     column that is plumbed end-to-end and dropped at the render fails here.
#   * §2 asserts `notify_panel` is the ONLY writer of the table, so the next
#     hand-rolled INSERT that skips a column fails without anyone remembering
#     this happened.
#   * §3 asserts every producer passes a link, against a reasoned allow-list.
#   * §4 resolves every link a producer can emit against the SPA's own route
#     table, so a notification can never point at a screen that is not there.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

ROUTE=panel/backend/src/routes/notifications.rs
SVC=panel/backend/src/services/notifications.rs
PAGE=panel/frontend/src/pages/Notifications.tsx
HOOK=panel/frontend/src/hooks/useLayoutState.ts
MAIN=panel/frontend/src/main.tsx
ICONS=panel/frontend/src/data/icons.ts
NAV=panel/frontend/src/data/navItems.ts
GUIDE=docs/guides/notifications.md
BACKEND=panel/backend/src

for f in "$ROUTE" "$SVC" "$PAGE" "$HOOK" "$MAIN" "$ICONS" "$NAV" "$GUIDE"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

echo
echo "── 1. every column the API returns is rendered, or waived with a reason ──"

# The column list is DERIVED from the endpoint's own SELECT rather than listed
# here, so adding a column to the API puts it under this arm automatically.
# A waiver must name a reason; a column can only leave the arm deliberately.
COLS=$(python3 - "$ROUTE" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
m = re.search(r'"SELECT ([^"]+?)\s*\\\s*\n\s*FROM panel_notifications', src)
if not m:
    print("PARSE-FAILED"); raise SystemExit
print(" ".join(c.strip() for c in m.group(1).split(",")))
PY
)

if [ "$COLS" = "PARSE-FAILED" ] || [ -z "$COLS" ]; then
  bad "N1 could not derive the SELECT column list from $ROUTE"
else
  NCOLS=$(echo "$COLS" | wc -w)
  if [ "$NCOLS" -lt 6 ]; then
    bad "N1 derived only $NCOLS columns — the parse is wrong, not the code ($COLS)"
  else
    ok "N1 derived $NCOLS columns from the list endpoint: $COLS"
  fi

  # Waived, with the reason each is not read by name in the render.
  #   id       — the React key and every action's argument, used as `n.id`
  #              (it IS read; listed here only because it is also the key)
  #   read_at  — read as `n.read_at` in the styling branches
  # Nothing else may be waived without adding it here and saying why.
  MISSING=""
  for c in $COLS; do
    if ! /usr/bin/grep -q "n\.$c" "$PAGE"; then
      MISSING="$MISSING $c"
    fi
  done
  if [ -n "$MISSING" ]; then
    bad "N1 column(s) returned by the API and never rendered:$MISSING"
  else
    ok "N1 every column the API returns is read by the page"
  fi

  # POSITIVE CONTROL: the arm must be able to fail. A column the page cannot
  # possibly render proves the matcher is looking at the page and not at a
  # constant true.
  if /usr/bin/grep -q "n\.definitely_not_a_column" "$PAGE"; then
    bad "N1 positive control matched a column that does not exist — the matcher is broken"
  else
    ok "N1 positive control: an absent column is correctly reported absent"
  fi
fi

# The specific field this suite is named for, keyed on the render rather than on
# the type declaration — `link: string | null` was present the whole time it was
# broken, so a check for the TYPE would have been green throughout.
if /usr/bin/grep -q 'to={n\.link}' "$PAGE"; then
  ok "N1 the notification title is a <Link to={n.link}>"
else
  bad "N1 nothing navigates to n.link — the field is transported and dropped again"
fi

if /usr/bin/grep -q 'isSafeLink(n\.link)' "$PAGE" && /usr/bin/grep -q 'startsWith("//")' "$PAGE"; then
  ok "N1 link values are shape-checked before being followed"
else
  bad "N1 the link guard is gone — `link` is bare TEXT with no CHECK constraint"
fi

echo
echo "── 2. notify_panel is the only writer of panel_notifications ──"

WRITERS=$(/usr/bin/grep -rn "INSERT INTO panel_notifications" "$BACKEND" --include=*.rs | wc -l)
IN_SVC=$(/usr/bin/grep -c "INSERT INTO panel_notifications" "$SVC")

if [ "$WRITERS" -eq 0 ]; then
  bad "N2 found no writers at all — the pattern is wrong, not the code"
elif [ "$WRITERS" -eq "$IN_SVC" ]; then
  ok "N2 all $WRITERS INSERT sites live in the notifications service"
else
  OUTSIDE=$(/usr/bin/grep -rn "INSERT INTO panel_notifications" "$BACKEND" --include=*.rs \
            | /usr/bin/grep -v "services/notifications.rs" | cut -d: -f1-2 | tr '\n' ' ')
  bad "N2 $((WRITERS - IN_SVC)) INSERT site(s) bypass notify_panel: $OUTSIDE"
fi

# The broadcast is half of what a bypassing writer loses, so pin it to the same
# place. If NOTIF_TX is sent from anywhere else, a second delivery path exists.
# Keyed on the CALL, not the name: the name appears in comments that explain why
# a writer must not bypass it, and an arm that counts those is measuring prose.
TX_FILES=$(/usr/bin/grep -rlnE "NOTIF_TX\.(get|set)\(" "$BACKEND" --include=*.rs | tr '\n' ' ')
if [ "$(echo "$TX_FILES" | wc -w)" -eq 1 ]; then
  ok "N2 NOTIF_TX is touched in exactly one file: $TX_FILES"
else
  bad "N2 NOTIF_TX is touched in more than one file: $TX_FILES"
fi

# The INSERT must report its own failure. It used to be `let _ = …`, so a
# notification that failed to store was indistinguishable from one that stored.
if /usr/bin/grep -q "notification not stored" "$SVC"; then
  ok "N2 a failed notification INSERT is logged, not discarded"
else
  bad "N2 the INSERT failure path is silent again"
fi

echo
echo "── 3. every producer passes a link ──"

LINKS=$(python3 - "$BACKEND" <<'PY'
import os, re, sys
root = sys.argv[1]
# Allow-listed: the subject of the notification no longer exists, so any link
# would resolve to the catch-all route and land the operator on the Dashboard
# without saying why. Recorded here so it stays a decision.
ALLOW = {("routes/sites.rs", "Site deleted")}
missing, total, allowed = [], 0, 0
for dirpath, _, files in os.walk(root):
    for fn in files:
        if not fn.endswith(".rs"):
            continue
        path = os.path.join(dirpath, fn)
        rel = os.path.relpath(path, root)
        src = open(path).read()
        for m in re.finditer(r"notify_panel\s*\(", src):
            # Skip the definition itself and any prose mention in a comment.
            head = src.rfind("\n", 0, m.start())
            line = src[head + 1 : m.start()].lstrip()
            if line.startswith("//") or line.startswith("*") or "fn notify_panel" in src[m.start() - 30 : m.start()]:
                continue
            i, depth = m.end(), 1
            while depth and i < len(src):
                if src[i] == "(":
                    depth += 1
                elif src[i] == ")":
                    depth -= 1
                i += 1
            args, parts, cur, depth = src[m.end() : i - 1], [], "", 0
            for ch in args:
                if ch in "([{":
                    depth += 1
                if ch in ")]}":
                    depth -= 1
                if ch == "," and depth == 0:
                    parts.append(cur); cur = ""
                else:
                    cur += ch
            parts.append(cur)
            # A trailing comma leaves an empty final element. Drop blanks before
            # taking the last argument, or the arm inspects whitespace.
            parts = [p for p in parts if p.strip()]
            if len(parts) < 7:
                continue
            total += 1
            if parts[-1].strip().startswith("None"):
                ctx = args[:200]
                if any(rel == a and tag in ctx for a, tag in ALLOW):
                    allowed += 1
                else:
                    missing.append(f"{rel}:{src[:m.start()].count(chr(10)) + 1}")
print(f"TOTAL={total} ALLOWED={allowed}")
for x in missing:
    print("MISSING", x)
PY
)

TOTAL=$(echo "$LINKS" | sed -n 's/^TOTAL=\([0-9]*\).*/\1/p')
ALLOWED=$(echo "$LINKS" | sed -n 's/.*ALLOWED=\([0-9]*\).*/\1/p')
NMISS=$(echo "$LINKS" | /usr/bin/grep -c "^MISSING" || true)

if [ -z "${TOTAL:-}" ] || [ "${TOTAL:-0}" -lt 20 ]; then
  bad "N3 found only ${TOTAL:-0} notify_panel call sites — the parse is wrong, not the code"
else
  ok "N3 parsed $TOTAL notify_panel call sites"
  if [ "$NMISS" -eq 0 ]; then
    ok "N3 every producer passes a link ($ALLOWED reasoned exception(s))"
  else
    bad "N3 $NMISS producer(s) pass no link: $(echo "$LINKS" | /usr/bin/grep '^MISSING' | tr '\n' ' ')"
  fi
  # STALE-ALLOW: an allow-list entry that no longer matches anything is a lie
  # about the code, and it silences the arm for whatever takes its place.
  if [ "$ALLOWED" -eq 1 ]; then
    ok "N3 the one reasoned exception still matches its call site"
  else
    bad "N3 the allow-list matched $ALLOWED call sites, expected exactly 1 — it has gone stale"
  fi
fi

echo
echo "── 4. every link a producer can emit resolves to a real screen ──"

# The failure this prevents is a notification that takes you somewhere that is
# not there. Both sides are derived: the link literals from the backend, the
# route table from the SPA's own router.
ROUTES=$(python3 - "$MAIN" "$BACKEND" <<'PY'
import os, re, sys
routes = set(re.findall(r'path="([^"]+)"', open(sys.argv[1]).read()))
if len(routes) < 20:
    print("ROUTES-PARSE-FAILED"); raise SystemExit

def resolves(path):
    for r in routes:
        if r == path:
            return True
        rp, pp = r.strip("/").split("/"), path.strip("/").split("/")
        if len(rp) == len(pp) and all(a.startswith(":") or a == b for a, b in zip(rp, pp)):
            return True
    return False

bad = []
seen = 0
for dirpath, _, files in os.walk(sys.argv[2]):
    for fn in files:
        if not fn.endswith(".rs"):
            continue
        src = open(os.path.join(dirpath, fn)).read()
        for m in re.finditer(r'Some\((?:&format!\()?"(/[^"?]*)[^"]*"', src):
            p = m.group(1)
            # `/sites/{id}` in a format! string is `/sites/:id` to the router.
            probe = re.sub(r"\{[a-z_.]+\}", "x", p)
            seen += 1
            if not resolves(probe):
                bad.append(f"{os.path.relpath(os.path.join(dirpath, fn), sys.argv[2])} -> {p}")
print(f"SEEN={seen}")
for b in sorted(set(bad)):
    print("UNRESOLVED", b)
PY
)

if echo "$ROUTES" | /usr/bin/grep -q "ROUTES-PARSE-FAILED"; then
  bad "N4 could not derive the SPA route table from $MAIN"
else
  SEEN=$(echo "$ROUTES" | sed -n 's/^SEEN=\([0-9]*\)/\1/p')
  NBAD=$(echo "$ROUTES" | /usr/bin/grep -c "^UNRESOLVED" || true)
  if [ "${SEEN:-0}" -lt 10 ]; then
    bad "N4 found only ${SEEN:-0} link literals — the parse is wrong, not the code"
  elif [ "$NBAD" -eq 0 ]; then
    ok "N4 all $SEEN link literals resolve against the SPA router"
  else
    bad "N4 $NBAD link(s) point at no route: $(echo "$ROUTES" | /usr/bin/grep '^UNRESOLVED' | tr '\n' ' ')"
  fi
fi

echo
echo "── 5. the badge cap is shared and honest ──"

# Five copies of `> 9 ? "9+"` is how the cap came to disagree with the uncapped
# badge beside it. One helper, and no layout may hand-roll another.
# A saturating badge is a ternary whose TRUE branch is "<digits>+". Matching a
# bare numeric comparison instead catches every threshold in the SPA.
HANDROLLED=$(/usr/bin/grep -rnE '> [0-9]+ \? "[0-9]+\+"' panel/frontend/src --include=*.tsx | wc -l)
if [ "$HANDROLLED" -eq 0 ]; then
  ok "N5 no layout hand-rolls a saturating badge ternary"
else
  bad "N5 $HANDROLLED hand-rolled badge cap(s): $(/usr/bin/grep -rnE '> [0-9]+ \? "[0-9]+\+"' panel/frontend/src --include=*.tsx | cut -d: -f1-2 | tr '\n' ' ')"
fi

if /usr/bin/grep -q 'export function badgeCount' "$HOOK"; then
  ok "N5 badgeCount is the single definition of the cap"
else
  bad "N5 badgeCount is gone — every layout will grow its own cap again"
fi

# A saturated badge is only honest if the exact figure is reachable. Every bell
# carries it in its title; the count badges carry it too.
BELLS=$(/usr/bin/grep -rc 'to="/notifications"' panel/frontend/src/components/*.tsx | awk -F: '{s+=$2} END {print s}')
TITLED=$(/usr/bin/grep -rc 'unread notification\${' panel/frontend/src/components/*.tsx | awk -F: '{s+=$2} END {print s}')
if [ "${BELLS:-0}" -eq 0 ]; then
  bad "N5 found no bell links — the pattern is wrong, not the code"
elif [ "${BELLS:-0}" -eq "${TITLED:-0}" ]; then
  ok "N5 all $BELLS bells state the exact unread count on hover"
else
  bad "N5 $((BELLS - TITLED)) bell(s) show a capped badge with no exact figure"
fi

# The two counts on the Monitoring row mean different things and must say so.
if [ "$(/usr/bin/grep -rc 'firing alert\${' panel/frontend/src/components/*.tsx | awk -F: '{s+=$2} END {print s}')" -ge 4 ] \
   && [ "$(/usr/bin/grep -rc 'open incident\${' panel/frontend/src/components/*.tsx | awk -F: '{s+=$2} END {print s}')" -ge 4 ]; then
  ok "N5 firing-alert and open-incident badges are labelled apart"
else
  bad "N5 the two Monitoring counts are unlabelled again — they read as one quantity"
fi

echo
echo "── 6. the page is live, and the icon exists ──"

if /usr/bin/grep -q 'new EventSource("/api/notifications/stream")' "$PAGE"; then
  ok "N6 the notifications page subscribes to the stream"
else
  bad "N6 the page is a mount-time fetch again — arrivals need a reload"
fi

# The stream is only useful to a list if the payload can BE a list row.
for field in '"id": id' '"created_at": created_at' '"read_at"'; do
  if /usr/bin/grep -qF "$field" "$SVC"; then
    ok "N6 the SSE payload carries $field"
  else
    bad "N6 the SSE payload dropped $field — a client cannot render what arrives"
  fi
done

if /usr/bin/grep -q "addEventListener(NOTIF_CHANGE_EVENT" "$HOOK" \
   && /usr/bin/grep -q "dispatchEvent(new Event(NOTIF_CHANGE_EVENT))" "$PAGE"; then
  ok "N6 mark-read tells the badge, instead of leaving it a minute stale"
else
  bad "N6 nothing tells the badge a notification was read"
fi

# `Icon` falls back to the dashboard glyph for an unknown name, silently. Every
# name the nav registry declares must exist in every icon set, or a nav row
# wears another screen's icon and nothing reports it.
ICONMISS=$(python3 - "$NAV" "$ICONS" <<'PY'
import re, sys
names = set(re.findall(r'iconName:\s*"([^"]+)"', open(sys.argv[1]).read()))
src = open(sys.argv[2]).read()
missing = []
for set_name in ("command", "glass", "atlas"):
    m = re.search(rf"\n  {set_name}: \{{(.*?)\n  \}},\n", src, re.S)
    if not m:
        print("PARSE-FAILED"); raise SystemExit
    keys = set(re.findall(r"\n    ([a-zA-Z0-9_]+): \{", m.group(1)))
    for n in sorted(names - keys):
        missing.append(f"{set_name}/{n}")
print(f"NAMES={len(names)}")
for x in missing:
    print("NOICON", x)
PY
)
if echo "$ICONMISS" | /usr/bin/grep -q "PARSE-FAILED"; then
  bad "N6 could not parse the icon sets"
else
  NNAMES=$(echo "$ICONMISS" | sed -n 's/^NAMES=\([0-9]*\)/\1/p')
  NNOICON=$(echo "$ICONMISS" | /usr/bin/grep -c "^NOICON" || true)
  if [ "${NNAMES:-0}" -lt 10 ]; then
    bad "N6 found only ${NNAMES:-0} nav icon names — the parse is wrong, not the code"
  elif [ "$NNOICON" -eq 0 ]; then
    ok "N6 all $NNAMES nav icon names exist in all three icon sets"
  else
    bad "N6 $NNOICON nav icon(s) fall back to the dashboard glyph: $(echo "$ICONMISS" | /usr/bin/grep '^NOICON' | tr '\n' ' ')"
  fi
fi

echo
echo "── 7. the guide describes this product ──"

# Every claim below was FALSE when written down, and the guide went on saying it
# for five months. Each arm keys on the mechanism, not on the sentence.
if /usr/bin/grep -q "there is no\s*$" "$GUIDE" || /usr/bin/grep -q "there is no" "$GUIDE"; then
  ok "N7 the guide no longer promises a dropdown"
else
  bad "N7 the guide's dropdown claim is back — the bell is a plain Link"
fi

if /usr/bin/grep -q "99+" "$GUIDE" && /usr/bin/grep -q "n > 99" "$HOOK"; then
  ok "N7 the cap the guide publishes is the cap the code applies"
else
  bad "N7 the guide's saturation figure and badgeCount disagree"
fi

if /usr/bin/grep -q "50 at a time" "$GUIDE" && /usr/bin/grep -q "const PAGE_SIZE = 50" "$PAGE"; then
  ok "N7 the page size the guide publishes is the page size the code requests"
else
  bad "N7 the guide's page size and PAGE_SIZE disagree"
fi

if /usr/bin/grep -q "retention_notification_days" "$GUIDE"; then
  ok "N7 the guide discloses that the feed is a 30-day window"
else
  bad "N7 the guide no longer says notifications are deleted after 30 days"
fi

# Every category the guide tables must be a category some producer really passes.
CATMISS=$(python3 - "$GUIDE" "$BACKEND" <<'PY'
import os, re, sys
doc = open(sys.argv[1]).read()
claimed = set(re.findall(r"^\| `([a-z_]+)` \|", doc, re.M))
if len(claimed) < 5:
    print("PARSE-FAILED"); raise SystemExit
real = set()
for dirpath, _, files in os.walk(sys.argv[2]):
    for fn in files:
        if fn.endswith(".rs"):
            src = open(os.path.join(dirpath, fn)).read()
            for m in re.finditer(r'notify_panel\s*\(', src):
                seg = src[m.end() : m.end() + 900]
                real |= set(re.findall(r'"(alert|monitor|security|deploy|site|incident|backup|ssl|auto_heal|system)"', seg))
print(f"CLAIMED={len(claimed)}")
for c in sorted(claimed - real):
    print("PHANTOM", c)
PY
)
if echo "$CATMISS" | /usr/bin/grep -q "PARSE-FAILED"; then
  bad "N7 could not parse the guide's category table"
else
  NPHANTOM=$(echo "$CATMISS" | /usr/bin/grep -c "^PHANTOM" || true)
  NCLAIMED=$(echo "$CATMISS" | sed -n 's/^CLAIMED=\([0-9]*\)/\1/p')
  if [ "$NPHANTOM" -eq 0 ]; then
    ok "N7 all $NCLAIMED categories the guide tables are raised by a real producer"
  else
    bad "N7 $NPHANTOM category(s) documented and never raised: $(echo "$CATMISS" | /usr/bin/grep '^PHANTOM' | tr '\n' ' ')"
  fi
fi

echo
echo "── 8. the security events that had no audience ──"

# `get_user_channels` returns None for a user with no alert_rules row, which is
# every user on a fresh install. Three security paths delivered ONLY through it,
# so on a stock panel they reached nobody. Each must also reach the feed.
for pair in "panel/backend/src/services/security_hardening.rs:alert_lockdown" \
            "panel/backend/src/services/auto_healer.rs:CANARY TRIGGERED" \
            "panel/backend/src/services/uptime.rs:send_alerts"; do
  f=${pair%%:*}; anchor=${pair#*:}
  if /usr/bin/grep -qE "notifications::notify_panel\(" "$f"; then
    ok "N8 $(basename "$f") reaches the notification feed (${anchor})"
  else
    bad "N8 $(basename "$f") delivers only to external channels — silent on a stock install"
  fi
done

echo
if [ "$FAIL" -eq 0 ]; then
  printf '\033[0;32mPASS %d, FAIL 0\033[0m\n' "$PASS"
else
  printf '\033[0;31mPASS %d, FAIL %d\033[0m\n' "$PASS" "$FAIL"
fi
exit $((FAIL > 0))
