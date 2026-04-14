#!/bin/bash
# Post-push gate reminder hook for DockPanel
# Fires on Stop event — checks if a git push happened and prints reminders

INPUT=$(cat)

# Check if the transcript context mentions git push was executed
# The Stop hook doesn't receive tool history, so we check recent git state instead
LAST_PUSH=$(git reflog show --date=relative -1 2>/dev/null | head -1)

# Check if there was a push in the last 2 minutes (rough heuristic)
REMOTE_HEAD=$(git rev-parse origin/main 2>/dev/null)
LOCAL_HEAD=$(git rev-parse HEAD 2>/dev/null)

# If local and remote match AND there are recent commits, a push likely just happened
if [ "$REMOTE_HEAD" = "$LOCAL_HEAD" ] 2>/dev/null; then
  # Check what changed in the last commit
  CHANGED=$(git diff --name-only HEAD~1 2>/dev/null)

  REMINDERS=""

  if echo "$CHANGED" | grep -qE '^website/|^docs/'; then
    REMINDERS="${REMINDERS}\n- website/ or docs/ changed: deploy dockpanel.dev and/or docs.dockpanel.dev"
  fi

  if echo "$CHANGED" | grep -qE '^panel/(agent|backend|cli)/src/'; then
    LATEST_TAG=$(git describe --tags --abbrev=0 2>/dev/null)
    COMMITS_SINCE=$(git rev-list "${LATEST_TAG}..HEAD" --count 2>/dev/null || echo "?")
    REMINDERS="${REMINDERS}\n- panel/ source changed (${COMMITS_SINCE} commits since ${LATEST_TAG}): release binaries may be stale"
  fi

  # Always remind about memory
  REMINDERS="${REMINDERS}\n- Update memory files (dev_history, polish_tracker, tech_debt)"

  if [ -n "$REMINDERS" ]; then
    echo "{\"hookSpecificOutput\":{\"hookEventName\":\"Stop\",\"additionalContext\":\"POST-PUSH GATE REMINDERS:${REMINDERS}\"}}"
    exit 0
  fi
fi

# No push detected — silent exit
echo "{}"
exit 0
