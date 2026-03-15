import { useState, FormEvent } from "react";
import { useNavigate, Link, Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";

export default function Login() {
  const { user, login, verify2fa, loading } = useAuth();
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  // 2FA state
  const [twoFaToken, setTwoFaToken] = useState("");
  const [twoFaCode, setTwoFaCode] = useState("");

  if (loading) return null;
  if (user) return <Navigate to="/" replace />;

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
    <div className="min-h-screen flex items-center justify-center bg-dark-950 px-4">
      <div className="w-full max-w-sm">
        {/* Logo */}
        <div className="text-center mb-8">
          <div className="inline-flex items-center justify-center w-14 h-14 bg-rust-500 mb-4 logo-icon-glow">
            <svg className="w-8 h-8 text-dark-950" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
              <path d="M5 16h4" strokeLinecap="square" />
              <path d="M5 12h8" strokeLinecap="square" />
              <path d="M5 8h6" strokeLinecap="square" />
              <rect x="16" y="7" width="4" height="4" fill="currentColor" stroke="none" />
              <rect x="16" y="13" width="4" height="4" fill="currentColor" stroke="none" />
            </svg>
          </div>
          <h1 className="text-lg font-bold uppercase font-mono tracking-widest logo-glow"><span className="text-rust-500">Dock</span><span className="text-dark-50">Panel</span></h1>
          <p className="text-dark-200 text-sm mt-1">
            {twoFaToken ? "Enter your 2FA code" : "Sign in to your panel"}
          </p>
        </div>

        {/* 2FA Form */}
        {twoFaToken ? (
          <form onSubmit={handle2fa} className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4">
            {error && (
              <div role="alert" className="bg-red-500/10 text-red-400 text-sm px-4 py-3 rounded-lg border border-red-500/20">
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
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none transition-shadow text-sm text-center tracking-[0.5em] font-mono text-lg"
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
          <form onSubmit={handleSubmit} className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4">
            {error && (
              <div role="alert" className="bg-red-500/10 text-red-400 text-sm px-4 py-3 rounded-lg border border-red-500/20">
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
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none transition-shadow text-sm"
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
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none transition-shadow text-sm"
              />
            </div>

            <button
              type="submit"
              disabled={submitting}
              className="w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors text-sm"
            >
              {submitting ? "Signing in..." : "Sign in"}
            </button>
          </form>
        )}

        <p className="text-center text-dark-300 text-xs mt-6">
          Don't have an account?{" "}
          <Link to="/register" className="text-rust-400 hover:text-rust-300">
            Register
          </Link>
        </p>
      </div>
    </div>
  );
}
