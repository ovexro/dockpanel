#!/usr/bin/env bash
# backup-encryption-integrity-pin-e2e.sh — s441
#
# ONE PROPERTY: an encrypted database backup that has been corrupted or
# tampered with is REJECTED before it is ever decrypted, instead of
# decrypting silently into wrong bytes.
#
# `panel/agent/src/services/encryption.rs` used plain AES-256-CBC with no
# integrity check (CWE-353). PKCS7 padding bounds only the final ciphertext
# block, so a bit-flip anywhere earlier decrypts without error into
# corrupted plaintext — a database dump that silently fails to import, or
# worse, imports partially. Fix: encrypt-then-MAC. `encrypt_file` appends a
# 32-byte HMAC-SHA256 tag (over the ciphertext, keyed by a passphrase-
# derived subkey independent of the AES key openssl derives internally) plus
# an 8-byte magic marker. `decrypt_file` verifies that tag BEFORE handing
# anything to openssl. A backup encrypted before this fix has no trailer;
# `decrypt_file` detects the absence and falls back to the old,
# unauthenticated path so existing backups keep working.
#
# §A the crypto helpers + format constants exist.
# §B encrypt_file: waits for openssl success, THEN computes+appends the tag
#    (position — tagging a failed/partial encrypt would tag garbage).
# §C decrypt_file: verifies the tag BEFORE the openssl child is spawned
#    (position — the core property; decrypting-then-checking would already
#    have written attacker-controlled plaintext to disk by the time a check
#    ran).
# §D decrypt_file: a backup with no trailer (legacy, pre-fix) still
#    decrypts — the compatibility half of the fix.
# §E docs no longer claim a mode ("GCM") this file has never implemented.
# §F a live round trip against the REAL openssl binary + a Python replica of
#    the exact HMAC construction: encrypt, tag, verify+decrypt recovers the
#    original bytes; a single flipped ciphertext byte is caught by the tag
#    before decryption would even run.
#
# Position arms (B, C), not just presence, for the same reason
# project_dockpanel_tech_debt_p178/p179 drew from their own mutations: a
# check that exists in the function but runs at the wrong point is not the
# same guarantee as the prose describing it claims.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  Backup encryption integrity — pins + live round trip (s441)"
echo "=================================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip comments before matching, or an arm can be satisfied by the prose
# that NARRATES the check rather than the check itself.
code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }

# A function body, bounded on ITS OWN braces. Anchored on `fn NAME(`, not the
# bare name, so a rename to a prefixed/suffixed sibling cannot satisfy it.
fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn "(") && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

offset() {
  local body="$1" needle="$2"
  local head="${body%%"$needle"*}"
  if [ "$head" = "$body" ]; then
    echo -1
  else
    echo "${#head}"
  fi
}

before() {
  local body="$1" first="$2" second="$3"
  local o1 o2
  o1=$(offset "$body" "$first")
  o2=$(offset "$body" "$second")
  [ "$o1" != -1 ] && [ "$o2" != -1 ] && [ "$o1" -lt "$o2" ]
}

ENC=panel/agent/src/services/encryption.rs
ENC_C=$(code "$ENC")

ENCRYPT_BODY=$(fnbody "$ENC_C" "encrypt_file")
DECRYPT_BODY=$(fnbody "$ENC_C" "decrypt_file")

# ── §A crypto helpers + format constants exist ───────────────────────────
echo "── §A HMAC helpers and format constants exist ──"

if has "$ENC_C" 'fn derive_mac_key'; then
  ok "A1 derive_mac_key exists"
else
  bad "A1 derive_mac_key is missing — nothing below can be measuring the real fix"
fi

if has "$ENC_C" 'fn compute_tag'; then
  ok "A2 compute_tag exists"
else
  bad "A2 compute_tag is missing"
fi

if has "$ENC_C" 'fn verify_tag'; then
  ok "A3 verify_tag exists"
else
  bad "A3 verify_tag is missing"
fi

if has "$ENC_C" 'MAC_MAGIC'; then
  ok "A4 a magic trailer marker distinguishes tagged from legacy files"
else
  bad "A4 no magic marker constant — legacy detection has nothing to key off"
fi

if has "$ENC_C" '"dockpanel-backup-hmac-v1"'; then
  ok "A5 the MAC key is domain-separated from the AES key by a fixed label"
else
  bad "A5 no domain-separation label — the MAC subkey derivation changed shape"
fi

# ── §B encrypt_file: tag AFTER confirmed success, not before ────────────
echo "── §B encrypt_file tags only a confirmed-good ciphertext ──"

if has "$ENCRYPT_BODY" 'compute_tag'; then
  ok "B1 encrypt_file calls compute_tag"
else
  bad "B1 encrypt_file does not call compute_tag — no tag is ever written"
fi

if before "$ENCRYPT_BODY" "output.status.success" "compute_tag("; then
  ok "B2 the openssl success check precedes compute_tag — a failed/partial encrypt is never tagged"
else
  bad "B2 compute_tag runs before (or without) confirming openssl succeeded — could tag garbage"
fi

if has "$ENCRYPT_BODY" '\.append\(true\)'; then
  ok "B3 the tag is appended to the ciphertext file, not written to a side channel"
else
  bad "B3 no append-mode write found — where does the tag actually go?"
fi

# ── §C decrypt_file: verify BEFORE decrypting ────────────────────────────
echo "── §C decrypt_file verifies the tag before openssl ever runs ──"

if has "$DECRYPT_BODY" 'verify_tag'; then
  ok "C1 decrypt_file calls verify_tag"
else
  bad "C1 decrypt_file does not call verify_tag — a tampered backup would decrypt unchecked"
fi

if before "$DECRYPT_BODY" "verify_tag(" "safe_command(\"openssl\")"; then
  ok "C2 verify_tag precedes the openssl child spawn — verify-then-decrypt, not the reverse"
else
  bad "C2 verify_tag runs at or after the openssl spawn — present-but-too-late is the same as absent"
fi

if has "$DECRYPT_BODY" "integrity check failed"; then
  ok "C3 a failed check returns a specific error, not a generic decrypt failure"
else
  bad "C3 no specific integrity-check error message found"
fi

# ── §D legacy (pre-fix, untagged) backups still decrypt ─────────────────
echo "── §D a backup with no trailer falls back to the old path ──"

if has "$DECRYPT_BODY" 'has_tag'; then
  ok "D1 decrypt_file branches on whether a trailer is present"
else
  bad "D1 no has_tag-style branch — every old backup would now fail to decrypt"
fi

if has "$DECRYPT_BODY" 'predates the integrity'; then
  ok "D2 the legacy path is deliberate and logged, not an accidental fallthrough"
else
  bad "D2 no warning/log for the legacy (unauthenticated) decrypt path"
fi

# ── §E docs describe the algorithm actually implemented ──────────────────
echo "── §E docs no longer claim a mode this file never implemented ──"

DOC=docs/guides/backup-orchestrator.md
if [ -f "$DOC" ] && ! grep -q "AES-256-GCM" "$DOC"; then
  ok "E1 $DOC no longer claims AES-256-GCM for backup encryption"
else
  bad "E1 $DOC still claims AES-256-GCM — openssl enc on this project's target OpenSSL refuses AEAD ciphers entirely"
fi

if [ -f "$DOC" ] && grep -qi "HMAC-SHA256" "$DOC"; then
  ok "E2 $DOC describes the encrypt-then-MAC scheme actually shipped"
else
  bad "E2 $DOC does not mention the HMAC integrity tag"
fi

# ── §F live round trip: real openssl + a faithful HMAC replica ──────────
echo "── §F live round trip (real openssl subprocess + Python HMAC replica) ──"

WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT

PYOUT=$(python3 - "$WORKDIR" <<'PYEOF'
import hashlib
import hmac
import os
import subprocess
import sys

workdir = sys.argv[1]
passphrase = "s441-integrity-test-passphrase"
plain_path = os.path.join(workdir, "plain.bin")
enc_path = os.path.join(workdir, "plain.bin.enc")

with open(plain_path, "wb") as f:
    f.write(os.urandom(4096))

# Same openssl invocation shape as encrypt_file (key via stdin, not argv).
r = subprocess.run(
    ["openssl", "enc", "-aes-256-cbc", "-salt", "-pbkdf2", "-iter", "100000",
     "-in", plain_path, "-out", enc_path, "-pass", "stdin"],
    input=passphrase.encode(), capture_output=True,
)
if r.returncode != 0:
    print(f"FAIL openssl encrypt: {r.stderr.decode()}")
    sys.exit(0)

ciphertext = open(enc_path, "rb").read()

def compute_tag(passphrase: str, ciphertext: bytes) -> bytes:
    mac_key = hmac.new(passphrase.encode(), b"dockpanel-backup-hmac-v1", hashlib.sha256).digest()
    return hmac.new(mac_key, ciphertext, hashlib.sha256).digest()

tag = compute_tag(passphrase, ciphertext)
tagged = ciphertext + tag + b"DPHMAC01"

# ---- happy path: verify then decrypt recovers the original plaintext ----
split = len(tagged) - 40
recovered_ct, recovered_tag = tagged[:split], tagged[split:split + 32]
if compute_tag(passphrase, recovered_ct) != recovered_tag:
    print("FAIL happy-path tag did not verify")
    sys.exit(0)

stripped_path = os.path.join(workdir, "stripped.enc")
open(stripped_path, "wb").write(recovered_ct)
dec_path = os.path.join(workdir, "plain.bin.dec")
r = subprocess.run(
    ["openssl", "enc", "-d", "-aes-256-cbc", "-pbkdf2", "-iter", "100000",
     "-in", stripped_path, "-out", dec_path, "-pass", "stdin"],
    input=passphrase.encode(), capture_output=True,
)
if r.returncode != 0:
    print(f"FAIL openssl decrypt: {r.stderr.decode()}")
    sys.exit(0)
if open(dec_path, "rb").read() != open(plain_path, "rb").read():
    print("FAIL decrypted bytes do not match the original plaintext")
    sys.exit(0)
print("OK roundtrip")

# ---- tamper path: a single flipped ciphertext byte must be caught ----
tampered = bytearray(tagged)
tampered[10] ^= 0xFF
t_split = len(tampered) - 40
t_ct, t_tag = bytes(tampered[:t_split]), bytes(tampered[t_split:t_split + 32])
if compute_tag(passphrase, t_ct) == t_tag:
    print("FAIL tamper went undetected")
    sys.exit(0)
print("OK tamper-detected")

# ---- wrong passphrase must not verify ----
if compute_tag("a different passphrase", recovered_ct) == recovered_tag:
    print("FAIL wrong-passphrase tag verified")
    sys.exit(0)
print("OK wrong-passphrase-rejected")
PYEOF
)

grep -q "^OK roundtrip$" <<< "$PYOUT" \
  && ok "F1 encrypt -> tag -> verify -> decrypt recovers the exact original bytes" \
  || bad "F1 live round trip failed: $PYOUT"

grep -q "^OK tamper-detected$" <<< "$PYOUT" \
  && ok "F2 a single flipped ciphertext byte is caught by the tag" \
  || bad "F2 tamper detection failed: $PYOUT"

grep -q "^OK wrong-passphrase-rejected$" <<< "$PYOUT" \
  && ok "F3 a backup encrypted under a different passphrase does not verify" \
  || bad "F3 wrong-passphrase check failed: $PYOUT"

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
