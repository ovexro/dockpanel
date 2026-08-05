#!/usr/bin/env bash
# Regression pins for s261 — the update path, and what driving it exposed.
#
# s260 shipped databases-in-backups. s261 asked the next question: does any of
# that reach the installs people already run? Driving a real v2.31.2 box up to
# v2.34.2 said yes — and the same box showed that git deploy had never worked
# on ANY hardened install, in three separate places, all the same shape: the
# agent runs ProtectSystem=strict + ProtectHome=yes, and the build path writes
# outside ReadWritePaths.
#
#   A. THE AGENT CAN WRITE WHAT ITS BUILD TOOLS NEED. `docker build` creates
#      $HOME/.docker and refuses to run without it; HOME=/root is an empty
#      read-only mount under the sandbox, so every Dockerfile deploy died at
#      "mkdir /root/.docker: read-only file system". The fix is one env var in
#      the shared helper, NOT a per-call-site patch — ~77 docker invocations go
#      through it and the one that got missed is how this stayed hidden.
#      Same shape twice more: nixpacks installed itself into /usr/local/bin and
#      cached into /var/cache/dockpanel, neither writable.
#   B. THE UPDATE'S AGENT CHECK ASKS THE AGENT. It curled an AUTHENTICATED
#      panel endpoint with no token, so it returned 401 and warned on every
#      update on every install — identically whether the agent was alive or
#      dead. A check that cannot distinguish those two states is not a check.
#   C. THE INSTALLER INSTALLS THE VERSION IT WAS GIVEN. install.sh reads
#      DOCKPANEL_VERSION and clones that ref; setup.sh, its only consumer,
#      always fetched releases/latest. `DOCKPANEL_VERSION=v2.31.2 install.sh`
#      built a v2.31.2 tree around v2.34.2 binaries and printed "v2.34.2".
#
# These are SOURCE pins and need no running panel. The behavioural proof is in
# /home/ovidiu/dockpanel-update-path-drill-s261.md.
#   run: bash tests/git-deploy-sandbox-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SAFECMD="$REPO/panel/agent/src/safe_cmd.rs"
GITBUILD="$REPO/panel/agent/src/services/git_build.rs"
UNIT="$REPO/panel/agent/dockpanel-agent.service"
UPDATE="$REPO/scripts/update.sh"
SETUP="$REPO/scripts/setup.sh"
INSTALL="$REPO/scripts/install.sh"
WEBINSTALL="$REPO/website/client/public/install.sh"

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
has()  { grep -q  -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
hasE() { grep -qE -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
# Comment-blind variants. These pins describe the defects BY NAME in the
# comments that document each fix, so a raw grep would match the prose
# explaining the bug and pass while the code itself regressed.
#
# The comment marker is per-language: stripping `//` from a shell script would
# also eat every `http://` URL — which is exactly how the first draft of this
# file reported two false failures against correct code.
strip_comments() {
    case "$1" in
        *.rs) sed -E 's|[[:space:]]*//.*$||' "$1" ;;
        *)    sed -E 's|[[:space:]]*#.*$||'  "$1" ;;
    esac
}
code()   { strip_comments "$1" | grep -qE -- "$2" && ok "$3" || bad "$3"; }
nocode() { strip_comments "$1" | grep -qE -- "$2" && bad "$3" || ok "$3"; }

echo
echo "A. The agent can write what its build tools need"
has  "$SAFECMD" 'DOCKER_CONFIG_DIR: &str = "/var/lib/dockpanel/docker"' \
     "the docker CLI config dir is a fixed path under /var/lib/dockpanel"
code "$SAFECMD" 'cmd\.env\("DOCKER_CONFIG", DOCKER_CONFIG_DIR\)' \
     "safe_command sets DOCKER_CONFIG (env_clear means a unit Environment= cannot)"
[ "$(sed -E 's/[[:space:]]*\/\/.*$//' "$SAFECMD" | grep -c 'DOCKER_CONFIG')" -ge 4 ] \
     && ok "every helper family sets it — async, sync, and both unsandboxed" \
     || bad "every helper family sets it — async, sync, and both unsandboxed"
hasE "$UNIT" 'ReadWritePaths=.*/var/lib/dockpanel' \
     "and /var/lib/dockpanel really is writable under the unit's sandbox"
nocode "$UNIT" 'ReadWritePaths=.*(/root|/usr/local/bin)' \
     "the fix did NOT widen the sandbox to /root or /usr/local/bin"

has  "$GITBUILD" 'NIXPACKS_BIN: &str = "/var/lib/dockpanel/bin/nixpacks"' \
     "nixpacks installs inside ReadWritePaths, not /usr/local/bin"
has  "$GITBUILD" 'NIXPACKS_CACHE_ROOT: &str = "/var/lib/dockpanel/nixpacks-cache"' \
     "its build cache too (/var/cache/dockpanel was never in ReadWritePaths)"
nocode "$GITBUILD" 'mv /tmp/nixpacks /usr/local/bin' \
     "the download no longer moves the binary into a read-only prefix"
nocode "$GITBUILD" '/var/cache/dockpanel/nixpacks' \
     "no code path still points the cache at the unwritable location"
code "$GITBUILD" 'Path::new\(NIXPACKS_BIN\)\.is_file\(\)' \
     "an already-downloaded nixpacks is found again (it is off the agent's PATH)"

echo
echo "B. The update's agent check asks the agent"
code "$UPDATE" 'curl .*--unix-socket .*AGENT_SOCK.*localhost/health' \
     "the check probes the agent's own socket"
nocode "$UPDATE" 'curl.*api/system/info' \
     "it no longer curls an authenticated panel endpoint with no token"
has  "$UPDATE" 'AGENT_SOCK=/run/dockpanel/agent.sock' \
     "the modern socket path is tried first"
has  "$UPDATE" 'AGENT_SOCK=/var/run/dockpanel/agent.sock' \
     "with the legacy path as fallback, so older boxes still probe correctly"
has  "$REPO/panel/agent/src/routes/mod.rs" 'if request.uri().path() == "/health"' \
     "and /health is genuinely exempt from the agent's auth middleware"

echo
echo "C. The installer installs the version it was given"
code "$SETUP" 'DOCKPANEL_VERSION' \
     "download_binaries reads the pin instead of ignoring it"
code "$SETUP" 'Pinned release' \
     "and says so, rather than printing a version it did not install"
code "$SETUP" 'releases/latest' \
     "unpinned installs still resolve the latest release"
code "$INSTALL" 'git clone .*-b "\$VERSION"' \
     "install.sh still selects the tree by the same variable"
[ -n "$(diff "$INSTALL" "$WEBINSTALL" 2>&1)" ] \
     && bad "the advertised website copy of install.sh matches scripts/install.sh" \
     || ok "the advertised website copy of install.sh matches scripts/install.sh"

echo
echo "D. The FIRST deploy of a panel-created site (s307)"
# s261 found that git deploy had never worked on a hardened install. s307 found
# that its first run had never worked on ANY install: creating a site writes a
# filler page into the document root, and git refuses a destination that exists
# and is not empty — so the opening deploy of every site the panel itself made
# failed, taking with it every step gated on a successful deploy.
#
# Windows are bound on the declaration's own closing brace, never on a fixed
# -A count, and an empty extraction FAILS rather than passing silently.
DEPLOYSVC="$REPO/panel/agent/src/services/deploy.rs"
NGINXSVC="$REPO/panel/agent/src/services/nginx.rs"
fn_body() { perl -0777 -ne 'print $1 if /\n((?:pub )?(?:async )?fn '"$2"'\b.*?\n\})\n/s' "$1"; }

CLONE_FN="$(fn_body "$DEPLOYSVC" clone_or_pull)"
if [ "$(printf '%s' "$CLONE_FN" | wc -l)" -ge 40 ]; then
  ok "extracted the clone_or_pull body ($(printf '%s' "$CLONE_FN" | wc -l) lines) — the arms below read real code"
else
  bad "could not extract clone_or_pull — every arm below is vacuous"
fi

CLONE_DEST="$(printf '%s' "$CLONE_FN" | sed -E 's|[[:space:]]*//.*$||' | grep -E '"clone",' || true)"
if [ -n "$CLONE_DEST" ] && ! grep -qE '&site_dir\]' <<< "$CLONE_DEST"; then
  ok "the fresh clone does not target the site directory git will refuse"
else
  bad "the fresh clone targets the populated site directory again — first deploy fails"
fi
printf '%s' "$CLONE_FN" | sed -E 's|[[:space:]]*//.*$||' | grep -qE 'deploy-staging' \
  && ok "it stages beside the site, where nginx cannot serve a half-finished checkout" \
  || bad "the staging directory is gone — a partial clone is reachable over HTTP"
printf '%s' "$CLONE_FN" | sed -E 's|[[:space:]]*//.*$||' | grep -qE 'foreign_entries\(' \
  && ok "a destination holding files the panel did not write is refused" \
  || bad "nothing checks the destination — a real site can be destroyed by a deploy"
printf '%s' "$CLONE_FN" | grep -qE 'Refusing to deploy into' \
  && ok "and the refusal names what it found instead of failing blank" \
  || bad "the refusal no longer tells the operator which files blocked it"

# The merge must be entry by entry. A whole-directory rename cannot work — the
# destination is non-empty, which is the entire reason this path exists — and
# the entries the clone does NOT supply must survive, or a repository with no
# public/ leaves a static site with no document root at all.
MERGE_FN="$(fn_body "$DEPLOYSVC" merge_into)"
if [ -n "$MERGE_FN" ]; then
  ok "extracted the merge_into body — the arms below read real code"
else
  bad "merge_into is gone — the entries the clone does not supply are unprotected"
fi
printf '%s' "$MERGE_FN" | sed -E 's|[[:space:]]*//.*$||' | grep -qE 'read_dir\(from\)' \
  && ok "the merge walks the staged clone entry by entry" \
  || bad "the merge no longer iterates — an unreplaced document root is at risk"
# An ABSENCE arm over a window that does not exist passes having examined
# nothing, which reads exactly like a clean sweep. Assert the subject first.
if [ -z "$MERGE_FN" ]; then
  bad "the whole-destination-wipe arm has no subject to read — it cannot report a catch"
elif printf '%s' "$MERGE_FN" | sed -E 's|[[:space:]]*//.*$||' | grep -qE 'remove_dir_all\((&)?to\)'; then
  bad "the merge clears the whole destination — a repo without public/ loses its document root"
else
  ok "the merge never clears the whole destination, so an unreplaced document root survives"
fi

# CLASS ARM, subjects derived FROM SOURCE. The filler page is WRITTEN when a
# site is created and READ BACK when a deploy decides whether a document root
# holds a real site. If those two ever spell it separately they can drift, and
# a deploy would then either refuse every fresh site or adopt an operator's
# file. Read the sentence out of the constant, then count it across the crate.
MARKER="$(perl -0777 -ne 'print $1 if /PLACEHOLDER_MARKER: &str = "([^"]+)"/' "$NGINXSVC")"
if [ -n "$MARKER" ]; then
  ok "the filler page's identifying sentence is declared once, as a constant"
else
  bad "PLACEHOLDER_MARKER is gone — the writer and the reader can drift apart"
fi
# An empty needle makes `grep -F` match every line, so the count arm would go
# red with a meaningless five-figure number and hide WHY. Guard it explicitly.
if [ -z "$MARKER" ]; then
  bad "the re-inlining arm has no sentence to count — blocked by the missing constant above"
else
  MARKER_N=$(grep -rF -- "$MARKER" "$REPO/panel/agent/src" 2>/dev/null | wc -l)
  if [ "$MARKER_N" -eq 1 ]; then
    ok "and nothing re-inlines it — $MARKER_N occurrence in panel/agent/src, the declaration"
  else
    bad "the sentence appears $MARKER_N times in panel/agent/src — writer and reader can now disagree"
  fi
fi
code "$NGINXSVC" 'read_to_string\(path\)' \
     "the page is recognised by CONTENT — an operator's own index.html is not ours to delete"
code "$REPO/panel/agent/src/routes/nginx.rs" 'placeholder_page\(' \
     "and put_site writes it through that same helper rather than a second literal"

# CLASS ARM: every path in this file that materialises a checkout on disk must
# hand it to the user the web server runs as. atomic_deploy always did; the
# non-atomic path never did, so a green deploy left a tree the application
# could not write to. Subjects are the clone invocations, found here rather
# than listed — but only the ones that SHIP. This arm's first run went red on
# two subjects inside the file's own test module, which run git against scratch
# directories and hold no site to hand over; the arm was right and the class
# definition was loose, so the boundary is drawn here and the count is printed
# rather than filtered away in silence.
SHIPPED=$(sed -E 's|[[:space:]]*//.*$||' "$DEPLOYSVC" | sed -n '1,/#\[cfg(test)\]/p')
ALL_CLONES=$(sed -E 's|[[:space:]]*//.*$||' "$DEPLOYSVC" | grep -cE '"clone",')
CHOWN_SUBJECTS=$(grep -cE '"clone",' <<< "$SHIPPED")
CHOWN_N=$(grep -cE 'hand_tree_to_web_user\(' <<< "$SHIPPED")
# The helper is called once per shipped checkout path plus once at its own
# declaration, so the subject count is the callers.
CHOWN_CALLS=$(grep -cE '^\s*hand_tree_to_web_user\(&' <<< "$SHIPPED")
if [ "$CHOWN_SUBJECTS" -ge 2 ] && [ "$CHOWN_CALLS" -ge "$CHOWN_SUBJECTS" ]; then
  ok "all $CHOWN_SUBJECTS shipped checkout paths reach hand_tree_to_web_user ($((ALL_CLONES - CHOWN_SUBJECTS)) test-only clones excluded by name)"
else
  bad "$CHOWN_SUBJECTS shipped checkout paths but only $CHOWN_CALLS reach the handover — a deploy succeeds and the app cannot write"
fi

# PAIRED WITH THE ABOVE: the ownership work moved into a helper at v2.64.2, and
# an arm that only checks the callers reach it would stay green if the helper
# stopped doing the job. Extracting a helper moves the subject of every pin that
# measured it, so both halves are pinned: the callers reach it, and it still
# hands the tree over.
if [ "$CHOWN_N" -ge 3 ] \
   && grep -qE 'args\(\["-R", "www-data:www-data"' <<< "$SHIPPED"; then
  ok "and hand_tree_to_web_user still chowns the tree to www-data"
else
  bad "hand_tree_to_web_user no longer chowns to www-data — every caller is now a no-op"
fi

# THE ESCALATION ARM. v2.64.0 chowned the whole checkout, `.git` included, and
# v2.64.1 then told git to accept a repository it did not own. Together those
# let a compromised site write `.git/config` and have root execute it on the
# next deploy — reproduced end to end with no hook file at all, via one
# `core.fsmonitor` line. The handover must therefore SKIP the repository, and
# take it back for root afterwards. Both halves, because either alone is the
# defect.
if grep -qE 'OsStr::new\("\.git"\)' <<< "$SHIPPED" \
   && grep -qE 'continue;' <<< "$SHIPPED"; then
  ok "the handover skips .git rather than handing the repository to the web user"
else
  bad "the handover no longer skips .git — the site can write config and hooks that root executes"
fi
if grep -qE 'args\(\["-R", "root:root", &git_dir\]\)' <<< "$SHIPPED" \
   && grep -qE 'args\(\["-R", "go-rwx", &git_dir\]\)' <<< "$SHIPPED"; then
  ok "and the repository is taken back for root, unreadable to anyone else"
else
  bad "the repository is not reclaimed as root-only — a checkout inherited from v2.64.x stays writable"
fi

# The site directory carries the sticky bit, or the application can simply
# unlink a root-owned `.git` and leave a repository of its own in its place —
# which would defeat every arm above without touching one of them.
if grep -qE 'args\(\["3775", dir\]\)' <<< "$SHIPPED"; then
  ok "and the site directory is setgid+sticky, so the site cannot replace .git wholesale"
else
  bad "the site directory is not sticky — the application can unlink .git and plant its own"
fi

# A repository root does not own is DISCARDED, never repaired in place:
# chowning content the web user wrote back to root and then executing it is the
# same defect with an extra step.
if grep -qE 'discard_repo_we_do_not_own\(&site_dir' <<< "$SHIPPED" \
   && grep -qE 'remove_dir_all\(&git_dir\)' <<< "$SHIPPED"; then
  ok "a repository root does not own is discarded before any git runs against it"
else
  bad "a foreign repository is adopted rather than discarded — the escalation survives an upgrade"
fi


# CLASS ARM: chowning a checkout to the web server's user and then reading it
# back as root is a pair, and the second half is easy to forget. Git refuses a
# repository owned by somebody else, so EVERY `git -C` in this file that points
# at a path the agent hands to www-data must also declare that path safe — and
# it must be command scope, because git ignores `safe.directory` written in the
# repository's own config. Found by driving: the FIRST deploy worked and the
# SECOND failed, and `list_releases` had been answering None on every atomic
# release since the chown was written. Subjects are found, not listed.
#
# ⚠ Subjects are GIT invocations, which is narrower than "lines containing -C".
# This arm's first run counted 6 and found 5, because `generate_deploy_key`
# passes ssh-keygen a `-C` too — and there it means the key's COMMENT, not a
# directory. Two programs, one flag spelling, one wrong class. The subject list
# is therefore resolved back to the command being run, and the non-git match is
# printed rather than dropped in silence.
CLASSIFY='
  my @lines = split /\n/, $src; my ($git, $safe, $other) = (0,0,0); my $cur = "";
  for my $l (@lines) {
    $cur = "git"     if $l =~ /safe_command(?:_sync)?\("git"\)/;
    $cur = "notgit"  if $l =~ /safe_command(?:_sync)?\("(?!git")[^"]+"\)/;
    next unless $l =~ /"-C",/;
    if ($cur eq "git") { $git++; }
    else               { $other++; }
  }
  for my $l (@lines) { $safe++ if $l =~ /"-c",\s*&(?:safe_dir|format!\("safe\.directory)/; }
  print "$git $safe $other";
'
read -r CGIT CSAFE COTHER <<< "$(SRC="$SHIPPED" perl -e '$src=$ENV{SRC};'"$CLASSIFY")"
# ⚠ INVERTED AT v2.64.2, and the inversion is the point. This arm used to demand
# a `safe.directory` on every shipped `git -C`. That demand was the escalation:
# the exception exists to let root operate on a tree somebody else owns, and the
# somebody else here is the web user. The requirement is now the opposite — the
# ownership guard stays switched ON, and the repository is kept root's instead.
if [ "${CGIT:-0}" -ge 4 ] && [ "${CSAFE:-0}" -eq 0 ]; then
  ok "none of the $CGIT shipped git -C invocations lifts the ownership guard ($COTHER non-git -C excluded by name)"
else
  bad "$CSAFE of $CGIT shipped git -C invocations still declare a directory safe — root will run git out of a tree the site controls"
fi

# The suppression pair, on every shipped git invocation. Ownership is the real
# control; this is what protects a site upgrading from v2.64.x, whose repository
# was writable and may already carry a payload. Command scope beats repo config,
# so it is inert from the first deploy after this version.
GITCMDS=$(grep -cE 'safe_command(_sync)?\("git"\)' <<< "$SHIPPED")
NOEXEC_N=$(grep -cE '\.args\(GIT_NO_EXEC\)' <<< "$SHIPPED")
if [ "$GITCMDS" -ge 4 ] && [ "$NOEXEC_N" -ge "$GITCMDS" ]; then
  ok "all $GITCMDS shipped git invocations carry GIT_NO_EXEC"
else
  bad "$GITCMDS shipped git invocations but only $NOEXEC_N carry GIT_NO_EXEC — repo config can still execute"
fi
if grep -qE 'core\.hooksPath=' <<< "$SHIPPED" && grep -qE '"core\.fsmonitor="' <<< "$SHIPPED"; then
  ok "and GIT_NO_EXEC suppresses BOTH hooks and fsmonitor — a hook file is not the only vector"
else
  bad "GIT_NO_EXEC no longer covers both vectors; core.fsmonitor alone needs no hook file and no executable bit"
fi

# The reset's exit status is READ. It was not, so a reset that failed — most
# reachably on a branch change over a checkout holding local commits — reported
# a successful deploy over the previous release.
if grep -qE 'if !reset\.status\.success\(\)' <<< "$SHIPPED"; then
  ok "a failed git reset fails the deploy instead of reporting success"
else
  bad "reset.status is never read — a failed reset still reports a successful deploy"
fi

echo
printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
