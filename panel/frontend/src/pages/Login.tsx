import { useState, useEffect, FormEvent, useCallback } from "react";
import { useNavigate, Link, Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { useBranding } from "../context/BrandingContext";

function base64urlToBuffer(b64: string): ArrayBuffer {
  const pad = b64.length % 4 === 0 ? "" : "=".repeat(4 - (b64.length % 4));
  const base64 = (b64 + pad).replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(base64);
  const arr = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) arr[i] = raw.charCodeAt(i);
  return arr.buffer;
}

function bufferToBase64url(buf: ArrayBuffer): string {
  const arr = new Uint8Array(buf);
  let binary = "";
  for (let i = 0; i < arr.length; i++) binary += String.fromCharCode(arr[i]);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export default function Login() {
  const { user, login, verify2fa, loading } = useAuth();
  const navigate = useNavigate();
  const branding = useBranding();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [needsSetup, setNeedsSetup] = useState(false);

  // 2FA state
  const [twoFaToken, setTwoFaToken] = useState("");
  const [twoFaCode, setTwoFaCode] = useState("");
  const [passkeySupported, setPasskeySupported] = useState(false);

  // Check if WebAuthn is available
  useEffect(() => {
    if (window.PublicKeyCredential) {
      setPasskeySupported(true);
    }
  }, []);

  const handlePasskeyLogin = useCallback(async () => {
    setError("");
    setSubmitting(true);
    try {
      // 1. Get challenge from server
      const beginRes = await fetch("/api/auth/passkey/auth/begin", { method: "POST" });
      if (!beginRes.ok) throw new Error("Failed to start passkey authentication");
      const { publicKey } = await beginRes.json();

      // 2. Convert base64url fields to ArrayBuffer
      publicKey.challenge = base64urlToBuffer(publicKey.challenge);
      if (publicKey.allowCredentials) {
        publicKey.allowCredentials = publicKey.allowCredentials.map((c: { id: string; type: string }) => ({
          ...c, id: base64urlToBuffer(c.id),
        }));
      }

      // 3. Call browser WebAuthn API
      const credential = await navigator.credentials.get({ publicKey }) as PublicKeyCredential;
      const response = credential.response as AuthenticatorAssertionResponse;

      // 4. Send response to server
      const completeRes = await fetch("/api/auth/passkey/auth/complete", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          id: credential.id,
          rawId: bufferToBase64url(credential.rawId),
          response: {
            authenticatorData: bufferToBase64url(response.authenticatorData),
            clientDataJson: bufferToBase64url(response.clientDataJSON),
            signature: bufferToBase64url(response.signature),
            userHandle: response.userHandle ? bufferToBase64url(response.userHandle) : null,
          },
        }),
      });

      if (!completeRes.ok) {
        const data = await completeRes.json();
        throw new Error(data.error || "Passkey authentication failed");
      }

      // Refresh auth state and navigate
      window.location.href = "/";
    } catch (err) {
      if (err instanceof Error && err.name !== "NotAllowedError") {
        setError(err.message || "Passkey authentication failed");
      }
    } finally {
      setSubmitting(false);
    }
  }, []);

  // Check if setup is needed (no users exist)
  useEffect(() => {
    fetch("/api/auth/setup-status")
      .then(r => r.json())
      .then(d => { if (d.needs_setup) setNeedsSetup(true); })
      .catch(() => {});
  }, []);

  if (loading) return (
    <div className="min-h-screen flex items-center justify-center">
      <div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" />
    </div>
  );
  if (user) return <Navigate to="/" replace />;
  if (needsSetup) return <Navigate to="/setup" replace />;

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      const challenge = await login(email, password);
      if (challenge) {
        // 2FA required
        setTwoFaToken(challenge.temp_token);
      } else {
        navigate("/");
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Login failed");
    } finally {
      setSubmitting(false);
    }
  };

  const handle2fa = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      await verify2fa(twoFaToken, twoFaCode);
      navigate("/");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Invalid 2FA code");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <main className="min-h-screen flex items-center justify-center login-bg px-4">
      <div className="w-full max-w-sm">
        {/* Logo */}
        <div className="text-center mb-8">
          {branding.logoUrl ? (
            <img src={branding.logoUrl} alt={branding.panelName} className="h-14 mx-auto mb-4 object-contain" />
          ) : (
            <div className="inline-flex items-center justify-center w-14 h-14 bg-rust-500 mb-4 logo-icon-glow">
              <svg className="w-8 h-8 text-dark-950" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                <path d="M5 16h4" strokeLinecap="square" />
                <path d="M5 12h8" strokeLinecap="square" />
                <path d="M5 8h6" strokeLinecap="square" />
                <rect x="16" y="7" width="4" height="4" fill="currentColor" stroke="none" />
                <rect x="16" y="13" width="4" height="4" fill="currentColor" stroke="none" />
              </svg>
            </div>
          )}
          {!branding.hideBranding && (
            <h1 className="text-lg font-bold uppercase font-mono tracking-widest logo-glow">
              {branding.panelName === "DockPanel" ? <><span className="text-rust-500">Dock</span><span className="text-dark-50">Panel</span></> : <span className="text-dark-50">{branding.panelName}</span>}
            </h1>
          )}
          <p className="text-dark-200 text-sm mt-1">
            {twoFaToken ? "Enter your 2FA code" : "Sign in to your panel"}
          </p>
        </div>

        {/* 2FA Form */}
        {twoFaToken ? (
          <form onSubmit={handle2fa} className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4 elevation-2">
            {error && (
              <div role="alert" className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20">
                {error}
              </div>
            )}

            <div>
              <label htmlFor="totp-code" className="block text-sm font-medium text-dark-100 mb-1">
                Authentication Code
              </label>
              <input
                id="totp-code"
                type="text"
                inputMode="numeric"
                autoComplete="one-time-code"
                value={twoFaCode}
                onChange={(e) => setTwoFaCode(e.target.value.replace(/\D/g, "").slice(0, 8))}
                required
                autoFocus
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none transition-shadow text-sm text-center tracking-[0.5em] font-mono text-lg"
                placeholder="000000"
              />
              <p className="text-xs text-dark-300 mt-2">
                Enter the 6-digit code from your authenticator app, or a recovery code.
              </p>
            </div>

            <button
              type="submit"
              disabled={submitting || twoFaCode.length < 6}
              className="w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors text-sm"
            >
              {submitting ? "Verifying..." : "Verify"}
            </button>

            <button
              type="button"
              onClick={() => { setTwoFaToken(""); setTwoFaCode(""); setError(""); }}
              className="w-full py-2 text-dark-300 text-sm hover:text-dark-100 transition-colors"
            >
              Back to login
            </button>
          </form>
        ) : (
          /* Login Form */
          <form onSubmit={handleSubmit} className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4 elevation-2">
            {error && (
              <div role="alert" className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20">
                {error}
              </div>
            )}

            <div>
              <label htmlFor="login-email" className="block text-sm font-medium text-dark-100 mb-1">Email</label>
              <input
                id="login-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                required
                autoFocus
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none transition-shadow text-sm"
                placeholder="admin@example.com"
              />
            </div>

            <div>
              <div className="flex items-center justify-between mb-1">
                <label htmlFor="login-password" className="block text-sm font-medium text-dark-100">Password</label>
                <Link to="/forgot-password" className="text-xs text-rust-400 hover:text-rust-300">
                  Forgot password?
                </Link>
              </div>
              <input
                id="login-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none transition-shadow text-sm"
              />
            </div>

            <button
              type="submit"
              disabled={submitting}
              className="w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors text-sm"
            >
              {submitting ? "Signing in..." : "Sign in"}
            </button>

            {passkeySupported && (
              <>
                <div className="flex items-center gap-3 my-1">
                  <div className="flex-1 h-px bg-dark-600" />
                  <span className="text-xs text-dark-400">or</span>
                  <div className="flex-1 h-px bg-dark-600" />
                </div>
                <button
                  type="button"
                  onClick={handlePasskeyLogin}
                  disabled={submitting}
                  className="w-full py-2.5 bg-dark-700 text-dark-100 rounded-lg font-medium text-sm hover:bg-dark-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors border border-dark-500 flex items-center justify-center gap-2"
                >
                  <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M2 18v3c0 .6.4 1 1 1h4v-3h3v-3h2l1.4-1.4a6.5 6.5 0 1 0-4-4Z" />
                    <circle cx="16.5" cy="7.5" r=".5" fill="currentColor" />
                  </svg>
                  Sign in with passkey
                </button>
              </>
            )}
          </form>
        )}

        {/* OAuth Buttons */}
        {branding.oauthProviders.length > 0 && !twoFaToken && (
          <div className="mt-4">
            <div className="flex items-center gap-3 my-4">
              <div className="flex-1 h-px bg-dark-600" />
              <span className="text-xs text-dark-400 uppercase">or continue with</span>
              <div className="flex-1 h-px bg-dark-600" />
            </div>
            <div className="flex flex-col gap-2">
              {branding.oauthProviders.includes("google") && (
                <a href="/api/auth/oauth/google" className="flex items-center justify-center gap-2 w-full py-2.5 bg-white text-dark-50 rounded-lg font-medium text-sm hover:bg-dark-800 transition-colors">
                  <svg className="w-4 h-4" viewBox="0 0 24 24"><path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 0 1-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.1z" fill="#4285F4"/><path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853"/><path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z" fill="#FBBC05"/><path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335"/></svg>
                  Google
                </a>
              )}
              {branding.oauthProviders.includes("github") && (
                <a href="/api/auth/oauth/github" className="flex items-center justify-center gap-2 w-full py-2.5 bg-dark-700 text-dark-50 rounded-lg font-medium text-sm hover:bg-dark-600 transition-colors border border-dark-500">
                  <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 24 24"><path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0 1 12 6.844a9.59 9.59 0 0 1 2.504.337c1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.02 10.02 0 0 0 22 12.017C22 6.484 17.522 2 12 2z"/></svg>
                  GitHub
                </a>
              )}
              {branding.oauthProviders.includes("gitlab") && (
                <a href="/api/auth/oauth/gitlab" className="flex items-center justify-center gap-2 w-full py-2.5 bg-[#FC6D26] text-white rounded-lg font-medium text-sm hover:bg-[#e5622b] transition-colors">
                  <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 24 24"><path d="m23.6 9.593-.033-.086L20.3.98a.851.851 0 0 0-.336-.384.859.859 0 0 0-.995.053.874.874 0 0 0-.29.387l-2.2 6.723H7.528L5.328 1.036a.857.857 0 0 0-.29-.387.86.86 0 0 0-.994-.053.854.854 0 0 0-.337.384L.44 9.507l-.033.086a6.066 6.066 0 0 0 2.012 7.01l.01.008.028.02 4.984 3.73 2.466 1.866 1.502 1.135a1.012 1.012 0 0 0 1.22 0l1.502-1.135 2.466-1.866 5.012-3.75.013-.01a6.072 6.072 0 0 0 2.008-7.008z"/></svg>
                  GitLab
                </a>
              )}
            </div>
          </div>
        )}

        <p className="text-center text-dark-300 text-xs mt-6">
          Don't have an account?{" "}
          <Link to="/register" className="text-rust-400 hover:text-rust-300">
            Register
          </Link>
        </p>

        <p className="text-center text-dark-400 text-[10px] mt-8 tracking-wider uppercase">
          Powered by Rust
        </p>
      </div>
    </main>
  );
}
