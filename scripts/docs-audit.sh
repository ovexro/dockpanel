#!/bin/bash
# DockPanel docs audit — verify all 10 doc surfaces match reality
# Run after feature work, before release

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'
ISSUES=0

cd "$(dirname "$0")/.." || exit 1

echo "=== DockPanel Docs Audit ==="
echo ""

# ─── Count actual values from source ───

# Template count (from agent docker_apps.rs id: definitions)
ACTUAL_TEMPLATES=$(grep -c 'id: "' panel/agent/src/services/docker_apps.rs 2>/dev/null || echo "?")

# E2E test assertions (across all test files — invocations of test_api, test_it, ok, check, assert, etc.)
ACTUAL_E2E=$(grep -chE '^\s*(test_api|test_contains|test_it|ok|fail|check|assert|expect|skip)\s+' tests/*.sh 2>/dev/null | awk '{s+=$1} END {print s}')

# API endpoint count (.route() registrations in backend + agent)
ACTUAL_ENDPOINTS_BACKEND=$(grep -rcE '\.route\(' panel/backend/src/ 2>/dev/null | awk -F: '{s+=$2} END {print s}')
ACTUAL_ENDPOINTS_AGENT=$(grep -rcE '\.route\(' panel/agent/src/ 2>/dev/null | awk -F: '{s+=$2} END {print s}')
ACTUAL_ENDPOINTS=$((ACTUAL_ENDPOINTS_BACKEND + ACTUAL_ENDPOINTS_AGENT))

# Frontend page count
ACTUAL_PAGES=$(ls panel/frontend/src/pages/*.tsx 2>/dev/null | wc -l)

# Migration count
ACTUAL_MIGRATIONS=$(ls panel/backend/migrations/*.sql 2>/dev/null | wc -l)

echo "Actual values from source:"
echo "  Templates:  $ACTUAL_TEMPLATES"
echo "  E2E tests:  $ACTUAL_E2E"
echo "  Endpoints:  $ACTUAL_ENDPOINTS"
echo "  Pages:      $ACTUAL_PAGES"
echo "  Migrations: $ACTUAL_MIGRATIONS"
echo ""

# ─── Check each doc surface ───

check_file() {
  local file=$1
  local label=$2
  local pattern=$3
  local actual=$4

  if [ ! -f "$file" ]; then
    echo -e "${YELLOW}SKIP: $file not found${NC}"
    return
  fi

  local found=$(grep -oE "$pattern" "$file" 2>/dev/null | head -1)
  if [ -z "$found" ]; then
    return  # Pattern not in this file, skip
  fi

  local num=$(echo "$found" | grep -oE '[0-9]+' | head -1)
  if [ -n "$num" ] && [ "$num" != "$actual" ]; then
    echo -e "${RED}MISMATCH in $label: says $num, actual $actual${NC}"
    echo -e "  File: $file"
    echo -e "  Pattern: $found"
    ISSUES=$((ISSUES + 1))
  fi
}

echo "Checking doc surfaces..."

# Check template counts
for f in README.md FEATURES.md COMPARISON.md docs/getting-started.md docs/api-reference.md website/client/src/pages/Landing.tsx; do
  check_file "$f" "$f (templates)" "[0-9]\+[+ ]*app templates\|[0-9]\+[+ ]*templates" "$ACTUAL_TEMPLATES"
done

# Check E2E counts
for f in README.md FEATURES.md; do
  check_file "$f" "$f (E2E)" "[0-9]\+ E2E\|[0-9]\+ test" "$ACTUAL_E2E"
done

# Check endpoint counts
for f in README.md FEATURES.md docs/api-reference.md website/client/src/pages/Landing.tsx; do
  check_file "$f" "$f (endpoints)" "[0-9]\+[+ ]* API endpoints\|[0-9]\+[+ ]*endpoints" "$ACTUAL_ENDPOINTS"
done

# Check migration counts
check_file "CONTRIBUTING.md" "CONTRIBUTING.md (migrations)" "[0-9]\+ migrations\|[0-9]\+ SQL" "$ACTUAL_MIGRATIONS"

echo ""

# ─── Version consistency ───
echo "Version check:"
V_AGENT=$(grep '^version' panel/agent/Cargo.toml 2>/dev/null | head -1 | sed 's/.*"\(.*\)"/\1/')
V_BACKEND=$(grep '^version' panel/backend/Cargo.toml 2>/dev/null | head -1 | sed 's/.*"\(.*\)"/\1/')
V_CLI=$(grep '^version' panel/cli/Cargo.toml 2>/dev/null | head -1 | sed 's/.*"\(.*\)"/\1/')
V_FRONTEND=$(grep '"version"' panel/frontend/package.json 2>/dev/null | head -1 | sed 's/.*: *"\(.*\)".*/\1/')
LATEST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "none")

echo "  agent=$V_AGENT backend=$V_BACKEND cli=$V_CLI frontend=$V_FRONTEND tag=$LATEST_TAG"

if [ "$V_AGENT" != "$V_BACKEND" ] || [ "$V_AGENT" != "$V_CLI" ] || [ "$V_AGENT" != "$V_FRONTEND" ]; then
  echo -e "${RED}Version mismatch between packages!${NC}"
  ISSUES=$((ISSUES + 1))
fi

if [ "$V_AGENT" != "${LATEST_TAG#v}" ] 2>/dev/null; then
  COMMITS_AHEAD=$(git rev-list "$LATEST_TAG..HEAD" --count 2>/dev/null || echo "?")
  echo -e "${YELLOW}Source version ($V_AGENT) != latest tag ($LATEST_TAG), $COMMITS_AHEAD commits ahead${NC}"
fi

echo ""

# ─── Summary ───
if [ "$ISSUES" -eq 0 ]; then
  echo -e "${GREEN}All doc surfaces consistent.${NC}"
else
  echo -e "${RED}Found $ISSUES issue(s). Fix before releasing.${NC}"
fi

exit $ISSUES
