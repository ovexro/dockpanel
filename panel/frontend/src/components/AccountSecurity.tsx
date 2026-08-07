import { useState, useEffect, useRef, type ReactNode } from "react";
import { api } from "../api";

/**
 * The self-service half of an account: the things a person changes about
 * THEMSELVES rather than about the panel.
 *
 * These lived inline in `pages/Settings.tsx`, whose nav row is `adminOnly`
 * (`data/navItems.ts`), so every non-admin account — `user`, `client`,
 * `reseller` — had no door to its own password, 2FA, passkeys, sessions or API
 * keys. Every endpoint behind them was already `AuthUser` and already scoped to
 * the caller's own id; the only thing missing was a way in. That is why this is
 * an extraction and not a feature: the capability shipped long ago, unreachable.
 *
 * Each card is self-contained — its own state, its own fetch, its own inline
 * message — following `SSHKeys`/`AutoUpdates`/`IPWhitelist` in `Settings.tsx`.
 * That is what lets BOTH `pages/Account.tsx` and the Settings "Account" tab
 * render the same implementation instead of a copy that drifts.
 *
 * Cards deliberately carry NO outer margin: both callers place them inside a
 * `space-y-*` container, which is what actually spaces them today.
 */

interface Passkey {
  id: string;
  name: string;
  created_at: string;
}

interface ApiKey {
  id: string;
  name: string;
  created_at: string;
}

interface Session {
  id: string;
  ip_address: string | null;
  user_agent: string | null;
  created_at: string;
  last_seen_at: string;
  is_current: boolean;
}

interface WebAuthnPublicKeyOptions {
  challenge: string | ArrayBuffer;
  user: { id: string | ArrayBuffer; name: string; displayName: string };
  rp: { name: string; id?: string };
  pubKeyCredParams: { type: string; alg: number }[];
  excludeCredentials?: { id: string | ArrayBuffer; type: string }[];
  [key: string]: unknown;
}

type Msg = { text: string; type: string };

const HEADING = "text-xs font-medium text-dark-300 uppercase font-mono tracking-widest";

function Card({ title, hint, action, children }: { title: string; hint?: string; action?: ReactNode; children: ReactNode }) {
  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className={`px-5 py-3 border-b border-dark-600 ${action ? "flex justify-between items-center" : ""}`}>
        <div>
          <h3 className={HEADING}>{title}</h3>
          {hint && <p className="text-xs text-dark-200 mt-0.5">{hint}</p>}
        </div>
        {action}
      </div>
      {children}
    </div>
  );
}

function Banner({ msg }: { msg: Msg }) {
  if (!msg.text) return null;
  return (
    <div className={`px-3 py-2 rounded text-xs ${msg.type === "success" ? "bg-rust-500/10 text-rust-400" : "bg-danger-500/10 text-danger-400"}`}>
      {msg.text}
    </div>
  );
}

/** The inline confirm bar from `Settings.tsx`, scoped to one card. */
function Confirm({ label, onConfirm, onCancel }: { label: string; onConfirm: () => void; onCancel: () => void }) {
  return (
    <div className="px-4 py-3 rounded-lg border border-danger-500/30 bg-danger-500/5 flex items-center justify-between">
      <span className="text-xs font-mono text-danger-400">{label}</span>
      <div className="flex items-center gap-2 shrink-0 ml-4">
        <button onClick={onConfirm} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">
          Confirm
        </button>
        <button onClick={onCancel} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">
          Cancel
        </button>
      </div>
    </div>
  );
}

const errText = (e: unknown, fallback: string) => (e instanceof Error ? e.message : fallback);

// ── Two-Factor Authentication ────────────────────────────────────────────

export function TwoFactorCard() {
  const [enabled, setEnabled] = useState(false);
  const [setup, setSetup] = useState<{ secret: string; qr_svg: string } | null>(null);
  const [code, setCode] = useState("");
  // One field for both actions below, because both take the same thing: a live
  // TOTP code OR one of the recovery codes. It was named for disabling back when
  // turning 2FA off was the only thing a code could do here.
  const [authCode, setAuthCode] = useState("");
  const [recoveryCodes, setRecoveryCodes] = useState<string[]>([]);
  const [remaining, setRemaining] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [msg, setMsg] = useState<Msg>({ text: "", type: "" });
  const qrRef = useRef<HTMLDivElement>(null);

  // 6 = a code from the authenticator. 16 = a recovery code. 8 = a recovery code
  // issued before v2.84.0, when they were half this width; those are still valid
  // and must still be typeable, because the people holding them are exactly the
  // people this screen exists for.
  const codeReady = authCode.length === 6 || authCode.length === 8 || authCode.length === 16;

  useEffect(() => {
    api.get<{ enabled: boolean; recovery_codes_remaining?: number }>("/auth/2fa/status")
      .then(d => {
        setEnabled(d.enabled);
        setRemaining(d.recovery_codes_remaining ?? null);
      })
      .catch(() => {});
  }, []);

  // Render the QR SVG as a data URI image, which sandboxes anything embedded in it.
  useEffect(() => {
    if (qrRef.current && setup?.qr_svg) {
      const encoded = btoa(unescape(encodeURIComponent(setup.qr_svg)));
      qrRef.current.innerHTML = "";
      const img = document.createElement("img");
      img.src = `data:image/svg+xml;base64,${encoded}`;
      img.alt = "2FA QR Code";
      img.width = 200;
      img.height = 200;
      qrRef.current.appendChild(img);
    }
  }, [setup?.qr_svg]);

  return (
    <Card title="Two-Factor Authentication" hint="Add an extra layer of security to your account">
      <div className="p-5 space-y-3">
        <Banner msg={msg} />
        {enabled ? (
          <div className="space-y-4">
            <div className="flex items-center gap-2">
              <div className="w-3 h-3 rounded-full bg-rust-500" />
              <span className="text-sm text-rust-400 font-medium">2FA is enabled</span>
            </div>
            {/* How many recovery codes are LEFT. They are spent one per recovery
                login and nothing ever counted them, so an account could reach zero
                — no fallback at all — while this card still said only "2FA is
                enabled". Zero is called out in its own words because it is the
                state that turns a lost phone into a lost account. */}
            {remaining !== null && (
              <p className={`text-xs ${remaining === 0 ? "text-danger-400" : remaining <= 2 ? "text-warn-400" : "text-dark-300"}`}>
                {remaining === 0
                  ? "No recovery codes left. If you lose your authenticator you will need an administrator to reset 2FA for you — issue a new set now."
                  : `${remaining} recovery ${remaining === 1 ? "code" : "codes"} remaining.`}
              </p>
            )}
            <p className="text-xs text-dark-300">
              Lost your authenticator? Enter one of your recovery codes below to turn 2FA off, then enrol again.
            </p>
            <div className="flex items-center gap-2 flex-wrap">
              {/* Accepts a code from the authenticator OR a recovery code —
                  both are hex, and the backend tries the authenticator first
                  then the recovery codes. This stripped everything non-numeric
                  and required exactly 6, so a recovery code could not even be
                  TYPED here: losing the authenticator meant losing the account,
                  because there was no admin-side 2FA reset either. The cap must
                  stay at or above the widest code the backend issues, or that
                  same defect comes back the moment the codes get longer. */}
              <input
                type="text"
                inputMode="text"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                value={authCode}
                onChange={(e) => setAuthCode(e.target.value.replace(/[^0-9a-fA-F]/g, "").toLowerCase().slice(0, 16))}
                placeholder="TOTP or recovery code"
                aria-label="TOTP code or recovery code"
                className="px-3 py-2 border border-dark-500 rounded-lg text-sm w-48 focus:ring-2 focus:ring-accent-500 outline-none font-mono"
              />
              {/* Issues a fresh set WITHOUT losing the enrolment. This is the
                  repair for every account that enrolled before the codes were
                  drawn on screen: it holds ten codes it has never seen, so its
                  fallback exists only in the database. */}
              <button
                disabled={loading || !codeReady}
                onClick={async () => {
                  setLoading(true);
                  setMsg({ text: "", type: "" });
                  try {
                    const res = await api.post<{ recovery_codes: string[] }>("/auth/2fa/recovery-codes", { code: authCode });
                    setAuthCode("");
                    setRecoveryCodes(res.recovery_codes);
                    setRemaining(res.recovery_codes.length);
                    setMsg({ text: "New recovery codes issued. The previous set no longer works.", type: "success" });
                  } catch (e) {
                    setMsg({ text: errText(e, "Failed"), type: "error" });
                  } finally {
                    setLoading(false);
                  }
                }}
                className="px-4 py-2 bg-dark-700 text-dark-100 border border-dark-500 rounded-lg text-sm font-medium hover:border-dark-400 disabled:opacity-50"
              >
                New recovery codes
              </button>
              <button
                disabled={loading || !codeReady}
                onClick={async () => {
                  setLoading(true);
                  setMsg({ text: "", type: "" });
                  try {
                    await api.post("/auth/2fa/disable", { code: authCode });
                    setEnabled(false);
                    setAuthCode("");
                    setRemaining(null);
                    setMsg({ text: "2FA disabled", type: "success" });
                  } catch (e) {
                    setMsg({ text: errText(e, "Failed"), type: "error" });
                  } finally {
                    setLoading(false);
                  }
                }}
                className="px-4 py-2 bg-danger-500/20 text-danger-400 rounded-lg text-sm font-medium hover:bg-danger-500/30 disabled:opacity-50"
              >
                Disable 2FA
              </button>
            </div>
          </div>
        ) : setup ? (
          <div className="space-y-4">
            <p className="text-sm text-dark-100">Scan this QR code with your authenticator app:</p>
            <div ref={qrRef} className="flex justify-center bg-white rounded-lg p-4 w-fit mx-auto" />
            <p className="text-xs text-dark-300 text-center font-mono break-all">
              Manual entry: {setup.secret}
            </p>
            <div className="flex items-center gap-2">
              <input
                type="text"
                inputMode="numeric"
                value={code}
                onChange={(e) => setCode(e.target.value.replace(/\D/g, "").slice(0, 6))}
                placeholder="Enter 6-digit code"
                aria-label="TOTP verification code"
                className="px-3 py-2 border border-dark-500 rounded-lg text-sm w-48 focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                autoFocus
              />
              <button
                disabled={loading || code.length < 6}
                onClick={async () => {
                  setLoading(true);
                  setMsg({ text: "", type: "" });
                  try {
                    const res = await api.post<{ recovery_codes: string[] }>("/auth/2fa/enable", { code });
                    setEnabled(true);
                    setSetup(null);
                    setCode("");
                    setRecoveryCodes(res.recovery_codes);
                    setRemaining(res.recovery_codes.length);
                    setMsg({ text: "2FA enabled! Save your recovery codes.", type: "success" });
                  } catch (e) {
                    setMsg({ text: errText(e, "Invalid code"), type: "error" });
                  } finally {
                    setLoading(false);
                  }
                }}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                Verify &amp; Enable
              </button>
              <button
                onClick={() => { setSetup(null); setCode(""); }}
                className="px-4 py-2 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400"
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <div className="space-y-3">
            <p className="text-sm text-dark-200">
              Protect your account with time-based one-time passwords (TOTP).
              Works with Google Authenticator, Authy, 1Password, etc.
            </p>
            <button
              disabled={loading}
              onClick={async () => {
                setLoading(true);
                setMsg({ text: "", type: "" });
                try {
                  const res = await api.post<{ secret: string; qr_svg: string }>("/auth/2fa/setup");
                  setSetup(res);
                } catch (e) {
                  setMsg({ text: errText(e, "Failed"), type: "error" });
                } finally {
                  setLoading(false);
                }
              }}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
            >
              {loading ? "Setting up..." : "Enable 2FA"}
            </button>
          </div>
        )}
        {recoveryCodes.length > 0 && (
          <div className="mt-4 p-4 bg-warn-500/10 rounded-lg border border-warn-500/20">
            <p className="text-sm font-medium text-warn-400 mb-2">Recovery Codes (save these!)</p>
            <div className="grid grid-cols-2 gap-1">
              {recoveryCodes.map((c, i) => (
                <code key={i} className="text-xs text-dark-100 font-mono bg-dark-900 px-2 py-1 rounded">{c}</code>
              ))}
            </div>
            <p className="text-xs text-dark-300 mt-2">Each code can only be used once. Store them securely.</p>
          </div>
        )}
      </div>
    </Card>
  );
}

// ── Passkeys / WebAuthn ──────────────────────────────────────────────────

export function PasskeysCard() {
  const [passkeys, setPasskeys] = useState<Passkey[]>([]);
  const [loading, setLoading] = useState(false);
  const [name, setName] = useState("My Passkey");
  const [supported] = useState(() => !!window.PublicKeyCredential);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [msg, setMsg] = useState<Msg>({ text: "", type: "" });

  const load = () => {
    api.get<{ passkeys: Passkey[] }>("/auth/passkeys")
      .then(d => setPasskeys(d.passkeys || []))
      .catch(() => {});
  };
  useEffect(load, []);

  const rename = async (pk: Passkey) => {
    if (!renameValue || renameValue === pk.name) { setRenaming(null); return; }
    try {
      await api.put(`/auth/passkeys/${pk.id}`, { name: renameValue });
      setPasskeys(prev => prev.map(p => (p.id === pk.id ? { ...p, name: renameValue } : p)));
      setMsg({ text: "Passkey renamed", type: "success" });
    } catch (e) { setMsg({ text: errText(e, "Failed"), type: "error" }); }
    setRenaming(null);
  };

  return (
    <Card title="Passkeys" hint="Passwordless sign-in with biometrics or security keys">
      <div className="p-5 space-y-3">
        <Banner msg={msg} />
        {!supported ? (
          <p className="text-sm text-dark-300">Your browser does not support WebAuthn/Passkeys.</p>
        ) : (
          <div className="space-y-4">
            {passkeys.length > 0 && (
              <div className="space-y-2">
                {passkeys.map((pk) => (
                  <div key={pk.id} className="flex items-center justify-between px-3 py-2 bg-dark-700 rounded-lg border border-dark-600">
                    <div className="flex items-center gap-3">
                      <svg className="w-4 h-4 text-rust-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                        <path d="M2 18v3c0 .6.4 1 1 1h4v-3h3v-3h2l1.4-1.4a6.5 6.5 0 1 0-4-4Z" />
                        <circle cx="16.5" cy="7.5" r=".5" fill="currentColor" />
                      </svg>
                      <div>
                        <p className="text-sm text-dark-100">{pk.name}</p>
                        <p className="text-xs text-dark-400">Added {new Date(pk.created_at).toLocaleDateString()}</p>
                      </div>
                    </div>
                    <div className="flex items-center gap-3">
                      {renaming === pk.id ? (
                        <span className="inline-flex items-center gap-1">
                          <input
                            type="text"
                            value={renameValue}
                            onChange={(e) => setRenameValue(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") rename(pk);
                              if (e.key === "Escape") setRenaming(null);
                            }}
                            autoFocus
                            className="w-28 px-2 py-0.5 bg-dark-900 border border-dark-500 rounded text-xs text-dark-100"
                          />
                          <button onClick={() => rename(pk)} className="px-1.5 py-0.5 bg-rust-500 text-white rounded text-[10px] font-medium">Save</button>
                          <button onClick={() => setRenaming(null)} className="px-1.5 py-0.5 bg-dark-600 text-dark-200 rounded text-[10px]">Cancel</button>
                        </span>
                      ) : (
                        <button
                          onClick={() => { setRenaming(pk.id); setRenameValue(pk.name); }}
                          className="text-xs text-accent-400 hover:text-accent-300"
                        >
                          Rename
                        </button>
                      )}
                      <button
                        onClick={async () => {
                          try {
                            await api.delete(`/auth/passkeys/${pk.id}`);
                            setPasskeys(prev => prev.filter(p => p.id !== pk.id));
                            setMsg({ text: "Passkey removed", type: "success" });
                          } catch (e) { setMsg({ text: errText(e, "Failed"), type: "error" }); }
                        }}
                        className="text-xs text-danger-400 hover:text-danger-300"
                      >
                        Remove
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}

            {passkeys.length < 10 && (
              <div className="flex items-end gap-3">
                <div className="flex-1">
                  <label className="block text-xs text-dark-300 mb-1">Passkey name</label>
                  <input
                    type="text"
                    value={name}
                    onChange={e => setName(e.target.value)}
                    className="w-full px-3 py-2 text-sm border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                    placeholder="My Passkey"
                  />
                </div>
                <button
                  disabled={loading}
                  onClick={async () => {
                    setLoading(true);
                    try {
                      const beginData = await api.post<{ publicKey: WebAuthnPublicKeyOptions }>("/auth/passkey/register/begin", {});
                      const publicKey = beginData.publicKey;

                      const b64toBuf = (b64: string) => {
                        const pad = b64.length % 4 === 0 ? "" : "=".repeat(4 - (b64.length % 4));
                        const raw = atob((b64 + pad).replace(/-/g, "+").replace(/_/g, "/"));
                        const arr = new Uint8Array(raw.length);
                        for (let i = 0; i < raw.length; i++) arr[i] = raw.charCodeAt(i);
                        return arr.buffer;
                      };
                      const bufTo64 = (buf: ArrayBuffer) => {
                        const arr = new Uint8Array(buf);
                        let bin = "";
                        for (let i = 0; i < arr.length; i++) bin += String.fromCharCode(arr[i]);
                        return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
                      };

                      publicKey.challenge = b64toBuf(publicKey.challenge as string);
                      publicKey.user.id = b64toBuf(publicKey.user.id as string);
                      if (publicKey.excludeCredentials) {
                        publicKey.excludeCredentials = (publicKey.excludeCredentials as { id: string; type: string }[]).map((c) => ({
                          ...c, id: b64toBuf(c.id),
                        }));
                      }

                      const credential = await navigator.credentials.create({ publicKey: publicKey as unknown as PublicKeyCredentialCreationOptions }) as PublicKeyCredential;
                      const attestation = credential.response as AuthenticatorAttestationResponse;

                      const transports = "getTransports" in attestation && typeof (attestation as AuthenticatorAttestationResponse & { getTransports?: () => string[] }).getTransports === "function"
                        ? (attestation as AuthenticatorAttestationResponse & { getTransports: () => string[] }).getTransports() : undefined;

                      await api.post("/auth/passkey/register/complete", {
                        id: credential.id,
                        rawId: bufTo64(credential.rawId),
                        response: {
                          attestationObject: bufTo64(attestation.attestationObject),
                          clientDataJson: bufTo64(attestation.clientDataJSON),
                        },
                        name: name || "My Passkey",
                        transports,
                      });

                      setMsg({ text: "Passkey registered!", type: "success" });
                      setName("My Passkey");
                      load();
                    } catch (e) {
                      if (e instanceof DOMException && e.name === "NotAllowedError") {
                        // User cancelled — ignore
                      } else {
                        setMsg({ text: errText(e, "Failed to register passkey"), type: "error" });
                      }
                    } finally {
                      setLoading(false);
                    }
                  }}
                  className="px-4 py-2 text-sm bg-rust-500 text-white rounded-lg hover:bg-rust-600 disabled:opacity-50 transition-colors whitespace-nowrap"
                >
                  {loading ? "Registering..." : "Add Passkey"}
                </button>
              </div>
            )}

            {passkeys.length === 0 && (
              <p className="text-xs text-dark-400">No passkeys registered. Add one to enable passwordless login.</p>
            )}
          </div>
        )}
      </div>
    </Card>
  );
}

// ── Change Password ──────────────────────────────────────────────────────

export function ChangePasswordCard() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [msg, setMsg] = useState<Msg>({ text: "", type: "" });

  return (
    <Card title="Change Password">
      <div className="p-5 space-y-3">
        <Banner msg={msg} />
        <input type="password" value={current} onChange={e => setCurrent(e.target.value)} placeholder="Current password"
          className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
        <input type="password" value={next} onChange={e => setNext(e.target.value)} placeholder="New password (min 8 chars)"
          className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
        <input type="password" value={confirm} onChange={e => setConfirm(e.target.value)} placeholder="Confirm new password"
          className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
        <button disabled={!current || !next || next !== confirm || next.length < 8} onClick={async () => {
          try {
            await api.post("/auth/change-password", { current_password: current, new_password: next });
            setMsg({ text: "Password changed successfully", type: "success" });
            setCurrent(""); setNext(""); setConfirm("");
          } catch (e) { setMsg({ text: errText(e, "Failed"), type: "error" }); }
        }} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">
          Change Password
        </button>
      </div>
    </Card>
  );
}

// ── Sessions ─────────────────────────────────────────────────────────────

/**
 * Only the caller's OWN sessions. The panel-wide "revoke everything" button is a
 * different capability on an `AdminUser` route (`POST /api/auth/revoke-all`
 * writes a global `sessions_revoked_at`), so it stays on the admin Settings page
 * rather than riding along into a client's account.
 */
export function SessionsCard() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [pending, setPending] = useState<Session | null>(null);
  const [msg, setMsg] = useState<Msg>({ text: "", type: "" });

  useEffect(() => {
    api.get<{ sessions: Session[] }>("/auth/sessions")
      .then(r => setSessions(r.sessions))
      .catch(() => {});
  }, []);

  return (
    <Card title="Sessions" hint="Devices currently signed in to your account">
      {(msg.text || pending) && (
        <div className="px-5 pt-4 space-y-3">
          <Banner msg={msg} />
          {pending && (
            <Confirm
              label={`Revoke the session from ${pending.ip_address || "unknown"}?`}
              onCancel={() => setPending(null)}
              onConfirm={async () => {
                const s = pending;
                setPending(null);
                try {
                  await api.delete(`/auth/sessions/${s.id}`);
                  setSessions(prev => prev.filter(x => x.id !== s.id));
                  setMsg({ text: "Session revoked", type: "success" });
                } catch (e) { setMsg({ text: errText(e, "Action failed"), type: "error" }); }
              }}
            />
          )}
        </div>
      )}
      <div className="divide-y divide-dark-600">
        {sessions.length === 0 && (
          <div className="px-5 py-4 text-xs text-dark-400">No active sessions recorded.</div>
        )}
        {sessions.map((s) => (
          <div key={s.id} className="px-5 py-3 flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm text-dark-100 flex items-center gap-2">
                <span className="font-mono">{s.ip_address || "unknown"}</span>
                {s.is_current && (
                  <span className="px-1.5 py-0.5 bg-ok-500/10 text-ok-400 rounded text-[10px] font-medium uppercase tracking-wider">
                    This device
                  </span>
                )}
              </p>
              <p className="text-xs text-dark-300 mt-0.5 truncate" title={s.user_agent || ""}>
                {s.user_agent || "unknown client"}
              </p>
              <p className="text-[11px] text-dark-400 mt-0.5 font-mono">
                last seen {new Date(s.last_seen_at).toLocaleString()}
              </p>
            </div>
            {!s.is_current && (
              <button
                onClick={() => setPending(s)}
                className="shrink-0 px-3 py-1.5 bg-danger-500/10 text-danger-400 rounded text-xs font-medium hover:bg-danger-500/20"
              >
                Revoke
              </button>
            )}
          </div>
        ))}
      </div>
    </Card>
  );
}

// ── API Keys ─────────────────────────────────────────────────────────────

export function ApiKeysCard() {
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [showNew, setShowNew] = useState(false);
  const [newName, setNewName] = useState("");
  const [newResult, setNewResult] = useState<string | null>(null);
  const [pending, setPending] = useState<ApiKey | null>(null);
  const [msg, setMsg] = useState<Msg>({ text: "", type: "" });

  const load = () => { api.get<ApiKey[]>("/api-keys").then(setKeys).catch(() => {}); };
  useEffect(load, []);

  return (
    <Card
      title="API Keys"
      action={
        <button onClick={() => setShowNew(!showNew)} className="text-xs text-rust-400 hover:text-rust-300">
          {showNew ? "Cancel" : "+ Create Key"}
        </button>
      }
    >
      {(msg.text || pending) && (
        <div className="px-5 pt-4 space-y-3">
          <Banner msg={msg} />
          {pending && (
            <Confirm
              label="Revoke this API key?"
              onCancel={() => setPending(null)}
              onConfirm={async () => {
                const k = pending;
                setPending(null);
                try {
                  await api.delete(`/api-keys/${k.id}`);
                  setKeys(prev => prev.filter(x => x.id !== k.id));
                  setMsg({ text: "API key revoked", type: "success" });
                } catch (e) { setMsg({ text: errText(e, "Action failed"), type: "error" }); }
              }}
            />
          )}
        </div>
      )}
      {showNew && (
        <div className="px-5 py-3 border-b border-dark-600 flex gap-2">
          <input value={newName} onChange={e => setNewName(e.target.value)} placeholder="Key name"
            className="flex-1 px-3 py-1.5 border border-dark-500 rounded text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
          <button onClick={async () => {
            try {
              const result = await api.post<{ key: string }>("/api-keys", { name: newName || "API Key" });
              setNewResult(result.key);
              setNewName("");
              setShowNew(false);
              load();
            } catch (e) { setMsg({ text: errText(e, "Failed"), type: "error" }); }
          }} className="px-3 py-1.5 bg-rust-500 text-white rounded text-xs font-medium hover:bg-rust-600">Create</button>
        </div>
      )}
      {newResult && (
        <div className="px-5 py-3 border-b border-dark-600 bg-rust-500/5">
          <p className="text-xs text-rust-400 mb-1">Copy this key now — it won't be shown again. Note: API keys do not yet authenticate requests; use your session or a JWT.</p>
          <div className="flex gap-2">
            <code className="flex-1 px-2 py-1 bg-dark-900 rounded text-xs font-mono text-dark-100 break-all">{newResult}</code>
            <button onClick={() => { navigator.clipboard.writeText(newResult); setNewResult(null); }} className="px-2 py-1 bg-dark-700 rounded text-xs text-dark-200 shrink-0">Copy</button>
          </div>
        </div>
      )}
      <div className="divide-y divide-dark-600">
        {keys.map((k) => (
          <div key={k.id} className="px-5 py-2.5 flex items-center justify-between text-xs">
            <div>
              <span className="text-dark-100">{k.name}</span>
              <span className="text-dark-400 ml-2">Created {new Date(k.created_at).toLocaleDateString()}</span>
            </div>
            <div className="flex gap-2">
              <button onClick={async () => {
                try {
                  const r = await api.post<{ key: string }>(`/api-keys/${k.id}/rotate`);
                  setNewResult(r.key);
                  load();
                } catch (e) { setMsg({ text: errText(e, "Failed"), type: "error" }); }
              }} className="text-dark-300 hover:text-dark-100">Rotate</button>
              <button onClick={() => setPending(k)} className="text-danger-400 hover:text-danger-300">Revoke</button>
            </div>
          </div>
        ))}
        {keys.length === 0 && <p className="px-5 py-3 text-xs text-dark-300">No API keys created</p>}
      </div>
    </Card>
  );
}

// ── Export my data ───────────────────────────────────────────────────────

export function ExportMyDataCard() {
  const [msg, setMsg] = useState<Msg>({ text: "", type: "" });

  return (
    <Card title="My Data">
      <div className="p-5 space-y-3">
        <Banner msg={msg} />
        <div className="flex items-center justify-between gap-4">
          <div>
            <p className="text-sm text-dark-100">Export my data</p>
            <p className="text-xs text-dark-300 mt-0.5">Everything this panel holds about your account, as JSON.</p>
          </div>
          <button
            onClick={async () => {
              try {
                const d = await api.get<Record<string, unknown>>("/auth/export-my-data");
                const url = URL.createObjectURL(new Blob([JSON.stringify(d, null, 2)], { type: "application/json" }));
                const a = document.createElement("a");
                a.href = url;
                a.download = "dockpanel-my-data.json";
                a.click();
                URL.revokeObjectURL(url);
              } catch (e) {
                setMsg({ text: errText(e, "Export failed"), type: "error" });
              }
            }}
            className="shrink-0 px-3 py-1.5 bg-dark-600 text-dark-100 rounded text-xs font-medium hover:bg-dark-500"
          >
            Export My Data
          </button>
        </div>
      </div>
    </Card>
  );
}
