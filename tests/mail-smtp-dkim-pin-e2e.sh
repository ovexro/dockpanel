#!/usr/bin/env bash
# Regression pins for the s262 SMTP drill.
#
# Two real Vultr boxes, two real dockpanel.dev domains, two real Let's Encrypt
# certificates, and the first message this product has ever been asked to
# actually deliver. What that found:
#
#   A. THE MAIL INSTALLER HAD NEVER COMPLETED. `POST /api/mail/install` died at
#      "Failed to write opendkim.conf: Read-only file system" — the agent runs
#      ProtectSystem=strict and /etc/opendkim.conf is a bare file in /etc, the
#      one mail path the unit's ReadWritePaths does not cover. Everything from
#      that line on never ran: the DKIM tables, the milter socket directory,
#      and the service enable. apt's postinst started the daemons anyway, so
#      the box looked healthy.
#   B. NOTHING HAD EVER BEEN DKIM-SIGNED. OpenDKIM's KeyTable and SigningTable
#      were written empty and no code anywhere else ever wrote them. Keys were
#      generated, published in real DNS, and verified green by the panel's own
#      dns-check — while every delivered message carried zero DKIM-Signature
#      headers. Proven on the wire, then fixed and re-proven: dkim=pass.
#   C. THE MAIL PORTS WERE NEVER OPENED. setup.sh allows 80/443/panel; the mail
#      installer started Postfix and Dovecot behind a firewall that dropped
#      every packet to 25/587/993/143.
#   D. THE HELO NAME WAS NEVER SET, costing six Rspamd points before any
#      content was read — and setting it without narrowing mydestination makes
#      Postfix treat a hosted mail domain as local and bounce real mailboxes.
#   E. A RE-RUN OF THE INSTALLER ERASED every domain, mailbox and password by
#      truncating the map files — and since the install returned 500, re-running
#      it was the ordinary thing to do.
#   F. DOVECOT WAS NEVER GIVEN THE BOX'S CERTIFICATE, so Roundcube could not log
#      in at all ("unknown ca"), and the panel's own frame-ancestors 'none'
#      applies to the webmail it installs.
#
# No running panel needed.
#   run: bash tests/mail-smtp-dkim-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAIL="$REPO/panel/agent/src/routes/mail.rs"
AGENT_UNIT="$REPO/panel/agent/dockpanel-agent.service"
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Strip comments per language. s261 shipped a stripper that removed `//` from
# SHELL files and ate every http:// URL, reporting two false failures against
# correct code — a comment-blind pin needs a comment model that matches the file.
strip_rust()  { sed -E 's;[[:space:]]*//.*$;;' "$1"; }
strip_shell() { sed -E 's/(^|[[:space:]])#.*$//' "$1"; }

CODE="$(strip_rust "$MAIL")"
# systemd units comment with '#', same as shell.
UNIT="$(strip_shell "$AGENT_UNIT")"

has()  { grep -qF -- "$2" <<<"$1"; }
hasre(){ grep -qE -- "$2" <<<"$1"; }

echo "── A. the installer must not write a path the sandbox refuses ──"
if has "$CODE" 'const OPENDKIM_CONF: &str = "/etc/dockpanel/opendkim.conf"'; then
  ok "OpenDKIM config lives under /etc/dockpanel (inside ReadWritePaths)"
else
  bad "OPENDKIM_CONF is no longer the ReadWritePaths-safe path"
fi
if hasre "$CODE" 'write_file_atomic\("/etc/opendkim\.conf"'; then
  bad "installer writes /etc/opendkim.conf again — EROFS under ProtectSystem=strict"
else
  ok "nothing writes the distro /etc/opendkim.conf"
fi
if hasre "$CODE" 'ExecStart=/usr/sbin/opendkim (-f )?-x '; then
  ok "systemd drop-in points opendkim at our config"
else
  bad "drop-in missing — daemon would read the stock config"
fi
# s262 pinned `-f` ABSENT, correctly for Debian, whose packaged unit is
# Type=forking. s268 measured EPEL's unit as Type=simple, where a flagless
# ExecStart makes opendkim background itself, systemd reap the parent, and the
# unit go INACTIVE while logging "Deactivated successfully" — a failure that
# does not register as failed. So the invariant is not the flag, it is the
# AGREEMENT between the Type the drop-in declares and the flag it passes.
dropin_type=$(grep -oE 'Type=[a-z]+\\nExecStart=' <<<"$CODE" | head -1 | sed 's/Type=//; s/\\nExecStart=//')
dropin_f=$(grep -cE 'ExecStart=/usr/sbin/opendkim -f ' <<<"$CODE")
if [ "$dropin_type" = "simple" ] && [ "$dropin_f" -ge 1 ]; then
  ok "drop-in declares Type=simple and passes -f — they agree, on both families"
elif [ "$dropin_type" = "forking" ] && [ "$dropin_f" -eq 0 ]; then
  ok "drop-in declares Type=forking and omits -f — they agree"
elif [ -z "$dropin_type" ]; then
  bad "the drop-in does not declare Type, so whether -f is right depends on the DISTRO's unit"
else
  ok "no -f flag (unit is Type=forking)"
fi

echo "── B. every domain must be bound to its key ──"
if has "$CODE" 'async fn rebuild_dkim_tables()'; then
  ok "rebuild_dkim_tables() exists"
else
  bad "rebuild_dkim_tables() gone — nothing populates the DKIM tables"
fi
for caller in 'DKIM keys generated for {domain} but tables not rebuilt' \
              'DKIM keys removed for {domain} but tables not rebuilt'; do
  if has "$CODE" "$caller"; then
    ok "table rebuild wired into: ${caller:5:24}…"
  else
    bad "a key-mutating path no longer rebuilds the tables"
  fi
done
# The whole point of the helper: derived from disk, so callers cannot drift.
if hasre "$CODE" 'key_lines\.push\(format!\("\{selector\}\._domainkey'; then
  ok "key.table entries are derived from the keys on disk"
else
  bad "key.table no longer derived from disk"
fi
if hasre "$CODE" 'signing_lines\.push\(format!\("\*@\{domain\}'; then
  ok "signing.table maps every address of the domain to its key"
else
  bad "signing.table entry shape changed"
fi

echo "── C. the ports the mail stack listens on must be opened ──"
if has "$CODE" 'async fn open_mail_ports()' && has "$CODE" 'open_mail_ports().await'; then
  ok "open_mail_ports() exists and is called by the installer"
else
  bad "mail ports are no longer opened — inbound mail cannot arrive"
fi
for p in '"25"' '"587"' '"993"' '"143"'; do
  if has "$CODE" "$p"; then ok "port $p in MAIL_PORTS"; else bad "port $p missing from MAIL_PORTS"; fi
done

echo "── D. HELO name, and the mydestination trap it opens ──"
if has "$CODE" 'myhostname={host}'; then
  ok "myhostname set from the panel vhost"
else
  bad "myhostname unset — HFILTER_HELO_5 costs 3 spam points"
fi
if has "$CODE" 'mydestination=localhost' && has "$CODE" 'mydestination = localhost'; then
  ok "mydestination narrowed to localhost in BOTH the postconf call and main.cf"
else
  bad "mydestination not narrowed — a hosted domain becomes local and bounces"
fi

echo "── E. a re-run must never erase hosted mail ──"
if hasre "$CODE" 'if !Path::new\(path\).exists\(\)'; then
  ok "map files are created only when absent"
else
  bad "map files truncated unconditionally — a re-run erases every mailbox"
fi
if hasre "$CODE" 'let has_active_submission'; then
  ok "submission guard tests for an ACTIVE entry, not the commented stock line"
else
  bad "submission guard back to substring match — re-runs duplicate the service"
fi

echo "── F. TLS the clients can actually verify ──"
if has "$CODE" 'fn panel_tls_paths()'; then
  ok "panel_tls_paths() exists"
else
  bad "panel_tls_paths() gone — Dovecot falls back to snakeoil"
fi
if has "$CODE" 'ssl_cert = <{cert}'; then
  ok "Dovecot is handed the real certificate when the box has one"
else
  bad "Dovecot ssl_cert no longer wired — Roundcube gets 'unknown ca'"
fi
if has "$CODE" "frame-ancestors 'self'"; then
  ok "webmail location allows same-origin framing"
else
  bad "webmail inherits frame-ancestors 'none' — the inbox renders empty"
fi

echo "── G. honest status ──"
if has "$CODE" '"configured": configured'; then
  ok "mail_status reports whether the install actually configured the box"
else
  bad "mail_status back to package-presence only — a failed install reads healthy"
fi

echo "── H. invariants this fix must not break ──"
if has "$UNIT" 'ProtectSystem=strict'; then
  ok "agent unit still hardened with ProtectSystem=strict"
else
  bad "sandbox weakened — the fix was supposed to work WITHIN it"
fi
# The fix must not have quietly bought its way out by widening the sandbox.
if hasre "$UNIT" '^ReadWritePaths=.*/etc/opendkim\.conf'; then
  bad "ReadWritePaths widened to /etc/opendkim.conf instead of relocating the config"
else
  ok "ReadWritePaths NOT widened for opendkim"
fi
# The property that matters is that Postfix can REACH the milter, not the
# mechanism. A unix socket under Postfix's spool is group-owned, so it needs
# gpasswd AND the unsandboxed escape (/etc/group is read-only to the agent). A
# loopback port needs neither — and is what s268 moved to, because the spool
# socket is a Debian-chroot artifact that SELinux forbids on RHEL. Accept
# either, reject a spool socket with no group grant.
if hasre "$CODE" 'OPENDKIM_SOCKET: &str = "inet:[0-9]+@127\.0\.0\.1"'; then
  ok "milter is a loopback port — reachable from inside Postfix's chroot and out, no group needed"
elif has "$CODE" 'safe_command_unsandboxed("gpasswd"'; then
  ok "group edit uses the documented unsandboxed escape (/etc/group is read-only)"
else
  bad "milter socket is group-owned but nothing adds postfix to the group — Postfix cannot reach it"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
