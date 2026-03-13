import { useState, FormEvent } from "react";
import { Link, Navigate, useSearchParams } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";

export default function ResetPassword() {
  const { user, loading } = useAuth();
  const [params] = useSearchParams();
  const token = params.get("token") || "";
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState("");
  const [success, setSuccess] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  if (loading) return null;
  if (user) return <Navigate to="/" replace />;

  if (!token) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-dark-950 px-4">
        <div className="bg-dark-800 rounded-xl shadow-lg p-6 max-w-sm w-full">
          <div className="bg-red-500/10 text-red-400 text-sm px-4 py-3 rounded-lg border border-red-500/20">
            Invalid reset link. Please request a new one.
          </div>
          <Link
            to="/forgot-password"
            className="block w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 text-center text-sm mt-4"
          >
            Request New Link
          </Link>
        </div>
      </div>
    );
  }

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    if (password !== confirm) {
      setError("Passwords do not match");
      return;
    }
    setSubmitting(true);
    try {
      await api.post("/auth/reset-password", { token, password });
      setSuccess(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Reset failed");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="min-h-screen flex items-center justify-center bg-dark-950 px-4">
      <div className="w-full max-w-sm">
        <div className="text-center mb-8">
          <h1 className="text-2xl font-bold text-white">Set New Password</h1>
          <p className="text-dark-200 text-sm mt-1">Enter your new password below</p>
        </div>

        {success ? (
          <div className="bg-dark-800 rounded-xl shadow-lg p-6 space-y-4">
            <div className="bg-emerald-50 text-emerald-400 text-sm px-4 py-3 rounded-lg border border-emerald-200">
              Password reset successfully!
            </div>
            <Link
              to="/login"
              className="block w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 text-center text-sm"
            >
              Sign in
            </Link>
          </div>
        ) : (
          <form onSubmit={handleSubmit} className="bg-dark-800 rounded-xl shadow-lg p-6 space-y-4">
            {error && (
              <div className="bg-red-500/10 text-red-400 text-sm px-4 py-3 rounded-lg border border-red-500/20">
                {error}
              </div>
            )}
            <div>
              <label htmlFor="new-password" className="block text-sm font-medium text-dark-100 mb-1">New Password</label>
              <input
                id="new-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
                minLength={8}
                autoFocus
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none transition-shadow text-sm"
              />
            </div>
            <div>
              <label htmlFor="confirm-password" className="block text-sm font-medium text-dark-100 mb-1">Confirm Password</label>
              <input
                id="confirm-password"
                type="password"
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
                required
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none transition-shadow text-sm"
              />
            </div>
            <button
              type="submit"
              disabled={submitting}
              className="w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors text-sm"
            >
              {submitting ? "Resetting..." : "Reset Password"}
            </button>
          </form>
        )}
      </div>
    </div>
  );
}
