#!/usr/bin/env bash
#
# Regression pin for the PANEL THEME token system.
#
# WHAT HAPPENED. The panel ships six chrome themes. Four are dark and two are
# light, and the light pair was a retrofit: the neutral ramp was inverted so a
# single card class works everywhere, but the brand and interactive ramps were
# left running light-to-dark. On a white ground that made the dominant ink
# unreadable — the worst measured 1.86:1 across 319 sites — while the theme that
# had been given a second pass looked fine, because roughly a hundred substring
# overrides repainted its broken markup. Reviewing new UI on that theme was
# therefore a FALSE PASS, and every defect below survived for months behind it.
#
# The other half was quieter. Five token steps were CONSUMED by the app and
# DEFINED IN NO THEME, so those classes emitted no CSS at all: the severity map
# printed CRITICAL in plain body text while MAJOR printed red. Two more steps
# were byte-identical to their neighbour in half the themes, so the hover state
# on 168 buttons changed nothing at all.
#
# WHY THIS SUITE MEASURES RATHER THAN MATCHES. A pin that asserts a hex is a pin
# that has to be edited every time a designer moves a colour, and it says nothing
# about whether the result is legible. So this suite parses the real token blocks
# out of the stylesheet and computes WCAG relative luminance itself, then asserts
# the ROLE constraints the design has to satisfy: ink on its own surface clears
# 4.5, a control boundary clears 3.0, and a hover step is a different colour from
# the step it hovers. Any palette that satisfies those passes; any that does not
# fails, whichever hexes it happens to use.
#
# THE ARITHMETIC IS IN awk ON PURPOSE — no python3, no node, no jq, so the suite
# runs anywhere the rest of the estate does.
#
# PROSE SAFETY. Every arm runs against COMMENT-STRIPPED source. The stylesheet
# now carries comments explaining these very constraints, and this header names
# the tokens it searches for (lesson #149, #395, [[feedback_source_pin_prose_trap]]).
#
# ENUMERATION SAFETY. §0 asserts each subject parsed to something plausible
# BEFORE any arm runs. An empty subject list makes every absence arm pass having
# examined nothing, which fails in the reassuring direction (lesson #143).
#
# Pure source analysis: no box, no network, no build, no browser.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

CSS=panel/frontend/src/index.css
FE=panel/frontend/src
for f in "$CSS" "$FE/pages" "$FE/components"; do
  [ -e "$f" ] || { echo "missing subject: $f"; exit 1; }
done

# Strip line comments only. A block-comment stripper is what silently deleted 485
# lines of real code until s294, and a truncated subject makes an absence arm
# pass on code the stripper merely removed (lesson #136).
CSSCODE=$(sed 's://.*$::' "$CSS")

# ── the token table, parsed out of the stylesheet itself ────────────────────
# Emits "theme token hex" for every theme, with the base @theme block inherited
# by any theme that does not override a step — which is exactly how the cascade
# resolves it at runtime.
TOKENS=$(printf '%s\n' "$CSSCODE" | awk '
  /^@theme[[:space:]]*\{/                     { blk="terminal"; next }
  /^\[data-theme="[a-z-]+"\][[:space:]]*\{$/  { match($0,/"[a-z-]+"/)
                                                blk=substr($0,RSTART+1,RLENGTH-2); next }
  /^\}/                                       { blk=""; next }
  blk != "" && /--color-[a-z]+-[0-9]+:/ {
      line=$0
      sub(/^[[:space:]]*--color-/,"",line)
      split(line,p,":")
      name=p[1]
      hex=p[2]
      gsub(/[^0-9a-fA-F#]/,"",hex)
      if (hex ~ /^#[0-9a-fA-F]{6}$/) { store[blk,name]=hex; if (!(name in seen)) { seen[name]=1; order[++n]=name } }
  }
  END {
      split("terminal midnight arctic ember clean clean-dark",themes," ")
      for (t=1; t<=6; t++) {
        th=themes[t]
        for (i=1; i<=n; i++) {
          nm=order[i]
          v = ((th,nm) in store) ? store[th,nm] : store["terminal",nm]
          if (v != "") print th, nm, v
        }
      }
  }')

echo "§0 the subjects parsed (an empty subject makes every arm below vacuous)"

CSSLINES=$(printf '%s\n' "$CSSCODE" | grep -c . || true)
if [ "$CSSLINES" -ge 800 ]; then
  ok "stylesheet survives the comment strip: $CSSLINES non-blank lines"
else
  bad "stylesheet stripped to $CSSLINES lines — subject truncated, arms are vacuous"
fi

THEMEN=$(printf '%s\n' "$TOKENS" | awk '{print $1}' | sort -u | grep -c . || true)
if [ "$THEMEN" -eq 6 ]; then
  ok "all six theme blocks parsed out of the stylesheet"
else
  bad "parsed $THEMEN theme blocks, expected 6 — the token table is not trustworthy"
fi

TOKN=$(printf '%s\n' "$TOKENS" | grep -c . || true)
if [ "$TOKN" -ge 200 ]; then
  ok "token table resolved: $TOKN theme/step pairs"
else
  bad "token table has only $TOKN rows — parse failed"
fi

# Every colour utility the app actually consumes, comment-stripped.
USED=$(find "$FE" \( -name '*.tsx' -o -name '*.ts' \) -print0 \
       | xargs -0 sed 's://.*$::' \
       | grep -ohE '(text|bg|border|ring|divide|from|to|via|outline|decoration|placeholder|caret|fill|stroke)-(rust|accent|dark|danger|warn)-[0-9]+' \
       | sed -E 's/^[a-z]+-//' | sort -u)
USEDN=$(printf '%s\n' "$USED" | grep -c . || true)
if [ "$USEDN" -ge 20 ]; then
  ok "consumed-token census found $USEDN distinct steps in use"
else
  bad "consumed-token census found only $USEDN steps — the enumeration is broken, not the code"
fi

echo "§1 every token the app consumes is defined in every theme"

MISSING=$(printf '%s\n' "$TOKENS" | awk -v used="$USED" '
  { have[$1 SUBSEP $2]=1 }
  END {
    n=split(used, steps, /[ \n]+/)
    split("terminal midnight arctic ember clean clean-dark", T, " ")
    for (i=1;i<=n;i++) if (steps[i] != "")
      for (t=1;t<=6;t++)
        if (!((T[t] SUBSEP steps[i]) in have)) out = out " " T[t] "/" steps[i]
    print out
  }')
if [ -z "$MISSING" ]; then
  ok "no consumed step is undefined in any theme"
else
  bad "consumed but undefined — these classes emit NO css:$MISSING"
fi

# CONTROL. This arm is only meaningful if the check above can actually fail, so
# assert a step that is deliberately absent from BOTH sides stays absent. If a
# future change defines it, this control fires and the arm above needs re-reading.
WARN300=$(printf '%s\n' "$TOKENS" | awk '$2=="warn-300"{n++} END{print n+0}')
if [ "$WARN300" -eq 0 ]; then
  ok "control: an undefined step is genuinely absent from the parsed table"
else
  bad "control: warn-300 is now defined — §1's discriminator has changed, re-read it"
fi

echo "§2 no stock Tailwind colour utility survives in the app"

STOCK_RE='(^|[^a-zA-Z0-9-])((hover|focus|active|group-hover|focus-within|disabled|dark|peer-focus|peer-checked|md|lg|sm|xl):)*(text|bg|border|ring|from|to|via|divide|fill|stroke|outline|decoration|placeholder|caret|shadow)-(slate|gray|grey|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-(50|100|200|300|400|500|600|700|800|900|950)(/[0-9]+)?'

STOCKN=$(find "$FE/pages" "$FE/components" \( -name '*.tsx' -o -name '*.ts' \) -print0 \
         | xargs -0 sed 's://.*$::' | grep -cE "$STOCK_RE" || true)
if [ "$STOCKN" -eq 0 ]; then
  ok "0 stock-palette colour utilities in pages/ and components/"
else
  bad "$STOCKN stock-palette colour utilities are back — no theme rule remaps them"
fi

# CONTROL for §2: the census regex has to be able to match something, or a green
# result above is a statement about the regex rather than about the tree.
CTRL=$(printf 'className="bg-emerald-500/10 text-amber-300"\n' | grep -cE "$STOCK_RE" || true)
if [ "$CTRL" -ge 1 ]; then
  ok "control: the stock-palette census pattern fires on a known stock string"
else
  bad "control: the census pattern matches nothing — §2 proved nothing"
fi

echo "§3 the role constraints, computed from the parsed hexes"

# WCAG relative luminance + contrast, in awk so the suite needs no interpreter.
contrast() {
  printf '%s\n' "$TOKENS" | awk -v want="$1" '
    function h2d(c){ return index("0123456789abcdef", tolower(c)) - 1 }
    function chan(s){ return h2d(substr(s,1,1))*16 + h2d(substr(s,2,1)) }
    function lin(v,  c){ c=v/255; return (c<=0.03928) ? c/12.92 : ((c+0.055)/1.055)^2.4 }
    function lum(hex){ return 0.2126*lin(chan(substr(hex,2,2))) \
                            + 0.7152*lin(chan(substr(hex,4,2))) \
                            + 0.0722*lin(chan(substr(hex,6,2))) }
    function ratio(a,b,  la,lb,t){ la=lum(a); lb=lum(b); if(la<lb){t=la;la=lb;lb=t}
                                   return (la+0.05)/(lb+0.05) }
    { tok[$1,$2]=$3; themes[$1]=1 }
    END {
      split("terminal midnight arctic ember clean clean-dark", T, " ")
      split(want, W, ":")          # fg:bg:floor   (fg may be the literal #hex)
      worst=99; worstT=""
      for (i=1;i<=6;i++) {
        th=T[i]
        fg = (W[1] ~ /^#/) ? W[1] : tok[th,W[1]]
        bg = (W[2] ~ /^#/) ? W[2] : tok[th,W[2]]
        if (fg=="" || bg=="") { print "MISSING " th; exit }
        r=ratio(fg,bg)
        if (r<worst) { worst=r; worstT=th }
      }
      printf "%.2f %s\n", worst, worstT
    }'
}

check_floor() {  # label  fg:bg  floor
  local label="$1" spec="$2" floor="$3"
  local out worst where
  out=$(contrast "$spec:$floor")
  worst=${out%% *}; where=${out##* }
  if [ "$worst" = "MISSING" ]; then
    bad "$label — a token in this constraint is undefined ($where)"
    return
  fi
  if awk -v w="$worst" -v f="$floor" 'BEGIN{exit !(w+0 >= f+0)}'; then
    ok "$label — worst theme $where at ${worst}:1 (floor $floor)"
  else
    bad "$label — $where at ${worst}:1, under the $floor floor"
  fi
}

check_floor "brand ink on its own card"        "rust-400:dark-800"    4.5
check_floor "interactive ink on its own card"  "accent-400:dark-800"  4.5
check_floor "the ink hover step on the card"   "rust-300:dark-800"    4.5
check_floor "danger ink on the card"           "danger-400:dark-800"  4.5
check_floor "warn ink on the card"             "warn-400:dark-800"    4.5
check_floor "muted ink on the card"            "dark-400:dark-800"    4.5
check_floor "form border against the field"    "dark-400:dark-900"    3.0
check_floor "white on the destructive fill"    "#ffffff:danger-500"   4.5
check_floor "white on the destructive hover"   "#ffffff:danger-600"   4.5

echo "§4 a hover step must be a different colour from the step it hovers"

hover_moves() {  # from  to
  printf '%s\n' "$TOKENS" | awk -v a="$1" -v b="$2" '
    { tok[$1,$2]=$3 }
    END {
      split("terminal midnight arctic ember clean clean-dark", T, " ")
      for (i=1;i<=6;i++) if (tolower(tok[T[i],a]) == tolower(tok[T[i],b])) dead = dead " " T[i]
      print (dead=="" ? "OK" : dead)
    }'
}
D=$(hover_moves rust-500 rust-600)
if [ "$D" = "OK" ]; then ok "bg-rust-500 and its hover step differ in all six themes"
else bad "hover:bg-rust-600 is a visual no-op in:$D"; fi

D=$(hover_moves rust-600 rust-700)
if [ "$D" = "OK" ]; then ok "bg-rust-600 and its hover step differ in all six themes"
else bad "hover:bg-rust-700 is a visual no-op in:$D"; fi

D=$(hover_moves danger-500 danger-600)
if [ "$D" = "OK" ]; then ok "the destructive fill and its hover step differ in all six themes"
else bad "the destructive hover is a visual no-op in:$D"; fi

echo "§5 the attribute that four call sites write now has a reader"

CS_L=$(printf '%s\n' "$CSSCODE" | grep -cE 'color-scheme:[[:space:]]*light' || true)
CS_D=$(printf '%s\n' "$CSSCODE" | grep -cE 'color-scheme:[[:space:]]*dark' || true)
if [ "$CS_L" -ge 1 ] && [ "$CS_D" -ge 1 ]; then
  ok "the stylesheet declares color-scheme for both the light and the dark case"
else
  bad "color-scheme is undeclared — the UA paints every native control light-scheme"
fi

WRITERS=$(find "$FE" panel/frontend/public \( -name '*.tsx' -o -name '*.ts' -o -name '*.js' \) -print0 2>/dev/null \
          | xargs -0 grep -l 'data-color-scheme' 2>/dev/null | grep -c . || true)
if [ "$WRITERS" -ge 3 ]; then
  ok "the attribute still has $WRITERS writers, so the new rules have something to read"
else
  bad "only $WRITERS writers set data-color-scheme — the new rules are dead"
fi

echo "§6 the destructive hover was corrected everywhere, not in the file someone opened"

LIGHTEN=$(find "$FE" \( -name '*.tsx' -o -name '*.ts' \) -print0 \
          | xargs -0 sed 's://.*$::' | grep -c 'hover:bg-danger-400' || true)
if [ "$LIGHTEN" -eq 0 ]; then
  ok "no destructive button hovers to the lighter fill"
else
  bad "$LIGHTEN destructive buttons still hover to a fill that fails AA with white text"
fi

echo "§7 the markdown surface is theme-driven (it is live on the monitoring page)"

MDLIT=$(printf '%s\n' "$CSSCODE" | grep 'markdown-body' | grep -cE 'rgba\(255, ?255, ?255|rgba\(0, ?0, ?0' || true)
if [ "$MDLIT" -eq 0 ]; then
  ok "no white-on-dark literal survives in the markdown rules"
else
  bad "$MDLIT markdown rules still hardcode a literal — invisible on the light themes"
fi

echo "§8 the scoped fill override cannot leak onto the tinted chips"

EMBER=$(printf '%s\n' "$CSSCODE" | grep -c 'data-theme="ember"\] \.bg-rust-500\.text-white' || true)
if [ "$EMBER" -ge 1 ]; then
  ok "the fill override is written as a compound class selector"
else
  bad "the compound-class fill override is gone — the dark-ink label is unscoped or missing"
fi

# The property that matters is narrow: a rule that sets the LABEL colour on the
# brand fill must select that fill as a class, because `[class*="bg-rust-500"]`
# also matches `bg-rust-500/10` and would repaint every tinted chip's text. A
# rule that only sets a BACKGROUND may select however it likes — the progress-bar
# track override has done so since long before this ship, and it is not the
# hazard. So: scan ember's rule blocks, keep the ones declaring a text colour,
# and require none of them to select the fill by substring.
LEAK=$(printf '%s\n' "$CSSCODE" | awk '
  /data-theme="ember"/ && !/^[[:space:]]/ { sel = sel $0; if ($0 ~ /\{[[:space:]]*$/) { inblk=1 } ; next }
  inblk && /^[[:space:]]*color:/ {
      if (sel ~ /class\*="[^"]*bg-rust-500/) n++
  }
  /^\}/ { inblk=0; sel="" }
  END { print n+0 }')
if [ "$LEAK" -eq 0 ]; then
  ok "no ember rule sets a label colour via a substring match on the fill"
else
  bad "$LEAK ember rule(s) set a label colour by substring — the tinted chips match too"
fi

echo "§9 the panel UI does not restate a number it does not own"

# The onboarding checklist carried "One-click deploy from 151 templates" while
# the catalogue went 153 -> 149 -> 148. Nothing pinned it, because the number
# lived on a second surface: FEATURES.md is the single owner of every published
# count, and a copy is the rot class by construction, not a value to update.
RESTATED=$(find "$FE" \( -name '*.tsx' -o -name '*.ts' \) -print0 \
           | xargs -0 sed 's://.*$::' \
           | grep -cE '[0-9]{3}\+? (app )?templates' || true)
if [ "$RESTATED" -eq 0 ]; then
  ok "no hardcoded catalogue count is restated in the panel UI"
else
  bad "$RESTATED hardcoded catalogue count(s) in the panel UI — they rot silently"
fi

echo
echo "──────────────────────────────────────────────────────────────"
printf 'PASS %d   FAIL %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
