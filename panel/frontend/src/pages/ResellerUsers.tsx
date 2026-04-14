import { useState, useEffect, useCallback } from "react";
import { api, ApiError } from "../api";

interface UserItem {
  id: string;
  email: string;
  role: string;
  created_at: string;
  site_count: number;
}

export default function ResellerUsers() {
  const [users, setUsers] = useState<UserItem[]>([]);
  const [creating, setCreating] = useState(false);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [loading, setLoading] = useState(true);
  const [pendingDelete, setPendingDelete] = useState<{ id: string; email: string } | null>(null);
  const [resetTarget, setResetTarget] = useState<{ id: string; email: string } | null>(null);
  const [resetPassword, setResetPassword] = useState("");

  const fetchUsers = useCallback(async () => {
    try {
      const data = await api.get<UserItem[]>("/reseller/users");
      setUsers(data);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to load users");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { fetchUsers(); }, [fetchUsers]);

  const handleCreate = async () => {
    if (!email.trim() || !password) return;
    setError("");
    try {
      await api.post("/reseller/users", { email: email.trim(), password });
      setEmail("");
      setPassword("");
      setCreating(false);
      await fetchUsers();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to create user");
    }
  };

  const handleDelete = (id: string, userEmail: string) => {
    setPendingDelete({ id, email: userEmail });
  };

  const executeDelete = async () => {
    if (!pendingDelete) return;
    const { id } = pendingDelete;
    setPendingDelete(null);
    setError("");
    try {
      await api.delete(`/reseller/users/${id}`);
      await fetchUsers();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to delete user");
    }
  };

  const handleResetPassword = async () => {
    if (!resetTarget || resetPassword.length < 8) {
      if (resetPassword) setError("Password must be at least 8 characters");
      return;
    }
    setError("");
    try {
      await api.put(`/reseller/users/${resetTarget.id}`, { password: resetPassword });
      setSuccess("Password updated successfully");
      setResetTarget(null);
      setResetPassword("");
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to update password");
    }
  };

  if (loading) return <div className="p-6"><div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" /></div>;

  return (
    <div className="p-6 space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-50 font-mono">My Users</h1>
          <p className="text-sm text-dark-300 mt-1">{users.length} user{users.length !== 1 ? "s" : ""} under your reseller account</p>
        </div>
        <button
          onClick={() => { setCreating(!creating); setError(""); }}
          className="px-4 py-2 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors"
        >
          + Create User
        </button>
      </div>

      {error && (
        <div className="px-4 py-3 bg-danger-500/10 border border-danger-500/30 rounded-lg text-sm text-danger-400">{error}</div>
      )}
      {success && (
        <div className="px-4 py-3 bg-rust-500/10 border border-rust-500/30 rounded-lg text-sm text-rust-400">{success}</div>
      )}

      {pendingDelete && (
        <div className="border border-danger-500/30 bg-danger-500/5 rounded-lg px-4 py-3 flex items-center justify-between">
          <span className="text-xs text-danger-400 font-mono">Delete user "{pendingDelete.email}"? Their sites and databases will also be deleted.</span>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            <button onClick={executeDelete} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">Confirm</button>
            <button onClick={() => setPendingDelete(null)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      {creating && (
        <div className="bg-dark-800 border border-dark-600 rounded-lg p-5 space-y-4">
          <h2 className="text-lg font-bold text-dark-50 font-mono">Create User</h2>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div>
              <label className="block text-sm text-dark-200 mb-1">Email</label>
              <input
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="user@example.com"
                type="email"
                className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none"
              />
            </div>
            <div>
              <label className="block text-sm text-dark-200 mb-1">Password</label>
              <input
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="Min 8 characters"
                type="password"
                className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none"
              />
            </div>
          </div>
          <div className="flex gap-3">
            <button onClick={handleCreate} className="px-4 py-2 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors">
              Create
            </button>
            <button onClick={() => setCreating(false)} className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg text-sm hover:bg-dark-600 transition-colors">
              Cancel
            </button>
          </div>
        </div>
      )}

      {/* User table */}
      <div className="bg-dark-800 border border-dark-600 rounded-lg overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-dark-600 text-dark-300 text-left">
              <th className="px-4 py-3 font-medium">Email</th>
              <th className="px-4 py-3 font-medium">Sites</th>
              <th className="px-4 py-3 font-medium">Created</th>
              <th className="px-4 py-3 font-medium text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {users.map((u) => (
              <tr key={u.id} className="border-b border-dark-700/50 hover:bg-dark-700/30">
                <td className="px-4 py-3 text-dark-50 font-mono">{u.email}</td>
                <td className="px-4 py-3 text-dark-300">{u.site_count}</td>
                <td className="px-4 py-3 text-dark-300">{new Date(u.created_at).toLocaleDateString()}</td>
                <td className="px-4 py-3 text-right space-x-2">
                  {resetTarget?.id === u.id ? (
                    <span className="inline-flex items-center gap-1.5">
                      <input
                        type="password"
                        value={resetPassword}
                        onChange={(e) => setResetPassword(e.target.value)}
                        onKeyDown={(e) => { if (e.key === "Enter") handleResetPassword(); if (e.key === "Escape") { setResetTarget(null); setResetPassword(""); } }}
                        autoFocus
                        className="w-28 px-2 py-1 bg-dark-900 border border-dark-500 rounded text-xs text-dark-100"
                        placeholder="New password"
                      />
                      <button onClick={handleResetPassword} disabled={resetPassword.length < 8} className="px-2 py-1 bg-rust-500 text-white rounded text-xs font-medium disabled:opacity-50">Set</button>
                      <button onClick={() => { setResetTarget(null); setResetPassword(""); }} className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs">Cancel</button>
                    </span>
                  ) : (
                    <button
                      onClick={() => { setResetTarget({ id: u.id, email: u.email }); setResetPassword(""); }}
                      className="px-2 py-1 text-xs text-dark-300 hover:text-dark-50 bg-dark-700 rounded hover:bg-dark-600 transition-colors"
                    >
                      Reset Password
                    </button>
                  )}
                  <button
                    onClick={() => handleDelete(u.id, u.email)}
                    className="px-2 py-1 text-xs text-danger-400 bg-danger-500/10 rounded hover:bg-danger-500/20 transition-colors"
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
            {users.length === 0 && (
              <tr>
                <td colSpan={4} className="px-4 py-8 text-center text-dark-400">
                  No users yet. Click "Create User" to add your first client.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
