import { useState, FormEvent } from "react";
import { useNavigate, Link } from "react-router-dom";
import { api, ApiError } from "../api";

export default function Setup() {
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError("");

    if (password !== confirm) {
      setError("Passwords do not match");
      return;
    }
    if (password.length < 8) {
      setError("Password must be at least 8 characters");
      return;
    }

    setSubmitting(true);
    try {
      await api.post("/auth/setup", { email, password });
      navigate("/login");
    } catch (err) {
      if (err instanceof ApiError && err.status === 403) {
        setError("Setup already completed. Please log in.");
      } else {
        setError(err instanceof Error ? err.message : "Setup failed");
      }
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
          <p className="text-dark-200 text-sm mt-1">Set up your DockPanel administrator</p>
        </div>

        <form onSubmit={handleSubmit} className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4">
          {error && (
            <div role="alert" className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20">
              {error}
            </div>
          )}

          <div>
            <label htmlFor="setup-email" className="block text-sm font-medium text-dark-100 mb-1">Email</label>
            <input
              id="setup-email"
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              required
              autoFocus
              className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none transition-shadow text-sm"
            />
          </div>

          <div>
            <label htmlFor="setup-password" className="block text-sm font-medium text-dark-100 mb-1">Password</label>
            <input
              id="setup-password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
              minLength={8}
              className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none transition-shadow text-sm"
            />
          </div>

          <div>
            <label htmlFor="setup-confirm-password" className="block text-sm font-medium text-dark-100 mb-1">Confirm Password</label>
            <input
              id="setup-confirm-password"
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
            className="w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 disabled:opacity-50 disabled:cursor-not-allowed transition-colors text-sm"
          >
            {submitting ? "Creating..." : "Create Admin Account"}
          </button>
        </form>

        <p className="text-center text-dark-300 text-xs mt-6">
          Already set up?{" "}
          <Link to="/login" className="text-rust-400 hover:text-rust-300">
            Sign in
          </Link>
        </p>
      </div>
    </div>
  );
}
