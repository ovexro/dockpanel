import { useState, FormEvent } from "react";
import { Link, Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";

export default function Register() {
  const { user, loading } = useAuth();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [submitting, setSubmitting] = useState(false);

  if (loading) return null;
  if (user) return <Navigate to="/" replace />;

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    if (password !== confirm) {
      setError("Passwords do not match");
      return;
    }
    setSubmitting(true);
    try {
      const res = await api.post<{ message: string }>("/auth/register", {
        email,
        password,
      });
      setSuccess(res.message || "Account created! Check your email to verify.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Registration failed");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="min-h-screen flex items-center justify-center bg-dark-950 px-4">
      <div className="w-full max-w-sm">
        <div className="text-center mb-8">
          <div className="inline-flex items-center justify-center w-14 h-14 bg-rust-500 rounded-xl mb-4">
            <svg className="w-8 h-8 text-white" viewBox="0 0 32 32" fill="currentColor">
              <rect x="4" y="4" width="10" height="10" rx="2" opacity="0.9" />
              <rect x="18" y="4" width="10" height="10" rx="2" opacity="0.7" />
              <rect x="4" y="18" width="10" height="10" rx="2" opacity="0.7" />
              <rect x="18" y="18" width="10" height="10" rx="2" opacity="0.5" />
            </svg>
          </div>
          <h1 className="text-base font-bold text-rust-500 uppercase font-mono tracking-widest">DockPanel</h1>
          <p className="text-dark-200 text-sm mt-1">Start managing your servers</p>
        </div>

        {success ? (
          <div className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4">
            <div className="bg-emerald-50 text-emerald-400 text-sm px-4 py-3 rounded-lg border border-emerald-200">
              {success}
            </div>
            <Link
              to="/login"
              className="block w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 text-center text-sm"
            >
              Go to Login
            </Link>
          </div>
        ) : (
          <form onSubmit={handleSubmit} className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4">
            {error && (
              <div className="bg-red-500/10 text-red-400 text-sm px-4 py-3 rounded-lg border border-red-500/20">
                {error}
              </div>
            )}
            <div>
              <label htmlFor="reg-email" className="block text-sm font-medium text-dark-100 mb-1">Email</label>
              <input
                id="reg-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                required
                autoFocus
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none transition-shadow text-sm"
                placeholder="you@example.com"
              />
            </div>
            <div>
              <label htmlFor="reg-password" className="block text-sm font-medium text-dark-100 mb-1">Password</label>
              <input
                id="reg-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
                minLength={8}
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none transition-shadow text-sm"
              />
            </div>
            <div>
              <label htmlFor="reg-confirm" className="block text-sm font-medium text-dark-100 mb-1">Confirm Password</label>
              <input
                id="reg-confirm"
                type="password"
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
                required
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none transition-shadow text-sm"
              />
            </div>
            <button
              type="submit"
              disabled={submitting}
              className="w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors text-sm"
            >
              {submitting ? "Creating account..." : "Create Account"}
            </button>
          </form>
        )}

        <p className="text-center text-dark-300 text-xs mt-6">
          Already have an account?{" "}
          <Link to="/login" className="text-rust-400 hover:text-rust-300">Sign in</Link>
        </p>
      </div>
    </div>
  );
}
