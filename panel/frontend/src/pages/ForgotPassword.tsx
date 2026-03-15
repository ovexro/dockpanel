import { useState, FormEvent } from "react";
import { Link, Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";

export default function ForgotPassword() {
  const { user, loading } = useAuth();
  const [email, setEmail] = useState("");
  const [error, setError] = useState("");
  const [success, setSuccess] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  if (loading) return null;
  if (user) return <Navigate to="/" replace />;

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      await api.post("/auth/forgot-password", { email });
      setSuccess(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to send reset email");
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
          <p className="text-dark-200 text-sm mt-1">We'll send you a reset link</p>
        </div>

        {success ? (
          <div className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4">
            <div className="bg-rust-50 text-rust-400 text-sm px-4 py-3 rounded-lg border border-rust-200">
              If an account exists with that email, a reset link has been sent. Check your inbox.
            </div>
            <Link
              to="/login"
              className="block w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 text-center text-sm"
            >
              Back to Login
            </Link>
          </div>
        ) : (
          <form onSubmit={handleSubmit} className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4">
            {error && (
              <div className="bg-red-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-red-500/20">
                {error}
              </div>
            )}
            <div>
              <label htmlFor="forgot-email" className="block text-sm font-medium text-dark-100 mb-1">Email</label>
              <input
                id="forgot-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                required
                autoFocus
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none transition-shadow text-sm"
                placeholder="you@example.com"
              />
            </div>
            <button
              type="submit"
              disabled={submitting}
              className="w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors text-sm"
            >
              {submitting ? "Sending..." : "Send Reset Link"}
            </button>
          </form>
        )}

        <p className="text-center text-dark-300 text-xs mt-6">
          <Link to="/login" className="text-rust-400 hover:text-rust-300">Back to login</Link>
        </p>
      </div>
    </div>
  );
}
