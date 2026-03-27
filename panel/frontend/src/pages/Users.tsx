import { useState, useEffect } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";

interface User {
  id: string;
  email: string;
  role: string;
  created_at: string;
  site_count: number;
}

export default function Users() {
  const { user: authUser } = useAuth();
  const [users, setUsers] = useState<User[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState("user");
  const [creating, setCreating] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [editTarget, setEditTarget] = useState<string | null>(null);
  const [editRole, setEditRole] = useState("");
  const [message, setMessage] = useState({ text: "", type: "" });
  const [search, setSearch] = useState("");
  const [suspendingId, setSuspendingId] = useState<string | null>(null);
  const [resetPwTarget, setResetPwTarget] = useState<string | null>(null);
  const [resetPwValue, setResetPwValue] = useState("");
  const [resettingPw, setResettingPw] = useState(false);

  const loadUsers = async () => {
    try {
      const data = await api.get<User[]>("/users");
      setUsers(data);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load users");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadUsers();
  }, []);

  if (authUser?.role !== "admin") return <Navigate to="/" replace />;

  const handleCreate = async () => {
    setCreating(true);
    setMessage({ text: "", type: "" });
    try {
      await api.post("/users", { email, password, role });
      setShowCreate(false);
      setEmail("");
      setPassword("");
      setRole("user");
      setMessage({ text: "User created successfully", type: "success" });
      loadUsers();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to create user",
        type: "error",
      });
    } finally {
      setCreating(false);
    }
  };

  const handleUpdateRole = async (id: string, newRole: string) => {
    try {
      await api.put(`/users/${id}`, { role: newRole });
      setEditTarget(null);
      setMessage({ text: "Role updated", type: "success" });
      loadUsers();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to update",
        type: "error",
      });
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await api.delete(`/users/${id}`);
      setDeleteTarget(null);
      setMessage({ text: "User deleted", type: "success" });
      loadUsers();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to delete",
        type: "error",
      });
    }
  };

  const handleToggleSuspend = async (user: User) => {
    setSuspendingId(user.id);
    setMessage({ text: "", type: "" });
    try {
      const data = await api.post<{ suspended: boolean; email: string }>(`/users/${user.id}/toggle-suspend`);
      setMessage({
        text: data.suspended ? `${user.email} has been suspended` : `${user.email} has been unsuspended`,
        type: "success",
      });
      loadUsers();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to update user",
        type: "error",
      });
    } finally {
      setSuspendingId(null);
    }
  };

  const handleResetPassword = async (id: string) => {
    if (resetPwValue.length < 8) {
      setMessage({ text: "Password must be at least 8 characters", type: "error" });
      return;
    }
    setResettingPw(true);
    setMessage({ text: "", type: "" });
    try {
      await api.post(`/users/${id}/reset-password`, { password: resetPwValue });
      setResetPwTarget(null);
      setResetPwValue("");
      setMessage({ text: "Password reset successfully", type: "success" });
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to reset password",
        type: "error",
      });
    } finally {
      setResettingPw(false);
    }
  };

  const isSelf = (id: string) => authUser?.id === id;

  if (error) {
    return (
      <div>
        <div className="bg-danger-500/10 text-danger-400 px-4 py-3 rounded-lg border border-danger-500/20">
          {error}
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Users</h1>
          <p className="page-header-subtitle">Manage user accounts and permissions</p>
        </div>
        <div className="flex items-center gap-2">
          {users.length >= 2 && (
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search users..."
              className="px-3 py-1.5 bg-dark-800 border border-dark-600 rounded-lg text-sm text-dark-100 placeholder-dark-400 focus:outline-none focus:border-dark-400"
            />
          )}
          <button
            onClick={() => setShowCreate(true)}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors flex items-center gap-2"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" />
            </svg>
            Add User
          </button>
        </div>
      </div>

      <div className="p-6 lg:p-8">

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
              : "bg-danger-500/10 text-danger-400 border-danger-500/20"
          }`}
        >
          {message.text}
        </div>
      )}

      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
        {loading ? (
          <div className="p-6 space-y-3 animate-pulse">
            {[...Array(4)].map((_, i) => (
              <div key={i} className="h-12 bg-dark-700 rounded w-full" />
            ))}
          </div>
        ) : !showCreate && users.length === 0 ? (
          <div className="p-12 text-center">
            <p className="text-dark-200 font-medium">No users</p>
          </div>
        ) : (
          <>
          {/* Desktop Table */}
          <table className="w-full hidden sm:table">
            <thead>
              <tr className="bg-dark-900 border-b border-dark-500">
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">Email</th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-28">Role</th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-20">Sites</th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-36">Created</th>
                <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-44">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-600">
              {users.filter((u) => u.email.toLowerCase().includes(search.toLowerCase())).map((user) => (
                <tr key={user.id} className={`table-row-hover ${user.role === "suspended" ? "opacity-60" : ""}`}>
                  <td className="px-5 py-4">
                    <div className="flex items-center gap-3">
                      <div className={`w-8 h-8 rounded-full flex items-center justify-center text-sm font-medium ${user.role === "suspended" ? "bg-dark-600 text-dark-400" : "bg-rust-500/15 text-rust-500"}`}>{user.email[0].toUpperCase()}</div>
                      <div className="flex items-center gap-2">
                        <span className="text-sm text-dark-50 font-mono">{user.email}</span>
                        {user.role === "suspended" && (
                          <span className="inline-flex px-2 py-0.5 rounded-full text-xs font-medium bg-danger-500/15 text-danger-400">Suspended</span>
                        )}
                      </div>
                    </div>
                  </td>
                  <td className="px-5 py-4">
                    {user.role === "suspended" ? (
                      <span className="inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium bg-danger-500/15 text-danger-400">suspended</span>
                    ) : editTarget === user.id ? (
                      <select value={editRole} onChange={(e) => setEditRole(e.target.value)} onBlur={() => { if (editRole !== user.role) handleUpdateRole(user.id, editRole); else setEditTarget(null); }} autoFocus className="text-sm border border-dark-500 rounded px-2 py-1">
                        <option value="admin">admin</option>
                        <option value="reseller">reseller</option>
                        <option value="user">user</option>
                      </select>
                    ) : (
                      <button onClick={() => { setEditTarget(user.id); setEditRole(user.role); }} className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium cursor-pointer ${user.role === "admin" ? "bg-accent-600/15 text-accent-400" : user.role === "reseller" ? "bg-rust-500/15 text-rust-400" : "bg-accent-500/15 text-accent-400"}`}>{user.role}</button>
                    )}
                  </td>
                  <td className="px-5 py-4 text-sm text-dark-200">{user.site_count}</td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">{new Date(user.created_at).toLocaleDateString()}</td>
                  <td className="px-5 py-4 text-right">
                    {deleteTarget === user.id ? (
                      <div className="flex items-center justify-end gap-1">
                        <button onClick={() => handleDelete(user.id)} className="px-2 py-1 bg-danger-500 text-white rounded text-xs">Confirm</button>
                        <button onClick={() => setDeleteTarget(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs">Cancel</button>
                      </div>
                    ) : (
                      <div className="flex items-center justify-end gap-1">
                        {!isSelf(user.id) && (
                          <>
                            <button
                              onClick={() => handleToggleSuspend(user)}
                              disabled={suspendingId === user.id}
                              className={`px-2 py-1 text-xs rounded transition-colors disabled:opacity-50 flex items-center gap-1 ${
                                user.role === "suspended"
                                  ? "text-rust-400 bg-rust-500/10 hover:bg-rust-500/20"
                                  : "text-warn-400 bg-warn-500/10 hover:bg-warn-500/20"
                              }`}
                              title={user.role === "suspended" ? "Unsuspend user" : "Suspend user"}
                            >
                              {suspendingId === user.id && <span className="w-3 h-3 border-2 border-current/30 border-t-current rounded-full animate-spin" />}
                              {user.role === "suspended" ? "Unsuspend" : "Suspend"}
                            </button>
                            <button
                              onClick={() => { setResetPwTarget(user.id); setResetPwValue(""); }}
                              className="px-2 py-1 text-xs text-dark-300 hover:text-dark-50 bg-dark-700 rounded hover:bg-dark-600 transition-colors"
                              title="Reset password"
                            >
                              Reset PW
                            </button>
                          </>
                        )}
                        <button onClick={() => setDeleteTarget(user.id)} className="text-dark-300 hover:text-danger-500 transition-colors" title="Delete user">
                          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 00-7.5 0" /></svg>
                        </button>
                      </div>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>

          {/* Mobile Cards */}
          <div className="sm:hidden divide-y divide-dark-600">
            {users.filter((u) => u.email.toLowerCase().includes(search.toLowerCase())).map((user) => (
              <div key={user.id} className={`px-4 py-3 ${user.role === "suspended" ? "opacity-60" : ""}`}>
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2.5 min-w-0">
                    <div className={`w-8 h-8 rounded-full flex items-center justify-center text-sm font-medium shrink-0 ${user.role === "suspended" ? "bg-dark-600 text-dark-400" : "bg-rust-500/15 text-rust-500"}`}>{user.email[0].toUpperCase()}</div>
                    <div className="min-w-0">
                      <div className="flex items-center gap-1.5">
                        <span className="text-sm text-dark-50 font-mono block truncate">{user.email}</span>
                        {user.role === "suspended" && (
                          <span className="inline-flex px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-danger-500/15 text-danger-400 shrink-0">Suspended</span>
                        )}
                      </div>
                      <span className="text-xs text-dark-300 font-mono">{new Date(user.created_at).toLocaleDateString()}</span>
                    </div>
                  </div>
                  <div className="flex items-center gap-2 shrink-0 ml-2">
                    <button onClick={() => { setEditTarget(user.id); setEditRole(user.role); }} className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${user.role === "admin" ? "bg-accent-600/15 text-accent-400" : user.role === "suspended" ? "bg-danger-500/15 text-danger-400" : "bg-accent-500/15 text-accent-400"}`}>{user.role}</button>
                    {!isSelf(user.id) && (
                      <button
                        onClick={() => handleToggleSuspend(user)}
                        disabled={suspendingId === user.id}
                        className={`px-2 py-1 text-xs rounded transition-colors disabled:opacity-50 ${
                          user.role === "suspended"
                            ? "text-rust-400 bg-rust-500/10"
                            : "text-warn-400 bg-warn-500/10"
                        }`}
                      >
                        {user.role === "suspended" ? "Unsuspend" : "Suspend"}
                      </button>
                    )}
                    {!isSelf(user.id) && (
                      <button
                        onClick={() => { setResetPwTarget(user.id); setResetPwValue(""); }}
                        className="p-1.5 text-dark-300 hover:text-dark-50"
                        title="Reset password"
                      >
                        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M15.75 5.25a3 3 0 0 1 3 3m3 0a6 6 0 0 1-7.029 5.912c-.563-.097-1.159.026-1.563.43L10.5 17.25H8.25v2.25H6v2.25H2.25v-2.818c0-.597.237-1.17.659-1.591l6.499-6.499c.404-.404.527-1 .43-1.563A6 6 0 1 1 21.75 8.25Z" /></svg>
                      </button>
                    )}
                    {deleteTarget === user.id ? (
                      <div className="flex items-center gap-1">
                        <button onClick={() => handleDelete(user.id)} className="px-2 py-1 bg-danger-500 text-white rounded text-xs">Del</button>
                        <button onClick={() => setDeleteTarget(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs">No</button>
                      </div>
                    ) : (
                      <button onClick={() => setDeleteTarget(user.id)} className="p-1.5 text-dark-300 hover:text-danger-500">
                        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" /></svg>
                      </button>
                    )}
                  </div>
                </div>
              </div>
            ))}
          </div>
          </>
        )}
      </div>
      </div>

      {/* Create dialog */}
      {showCreate && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 dp-modal-overlay"
          role="dialog"
          aria-labelledby="create-user-title"
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              setShowCreate(false);
              setEmail("");
              setPassword("");
            }
          }}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-[420px] dp-modal">
            <h3 id="create-user-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-4">
              Create User
            </h3>
            <div className="space-y-4">
              <div>
                <label htmlFor="create-user-email" className="block text-sm font-medium text-dark-100 mb-1">
                  Email
                </label>
                <input
                  id="create-user-email"
                  type="email"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                  placeholder="user@example.com"
                  autoFocus
                />
                <p className="text-xs text-dark-400 mt-1">User's email address for login</p>
              </div>
              <div>
                <label htmlFor="create-user-password" className="block text-sm font-medium text-dark-100 mb-1">
                  Password
                </label>
                <input
                  id="create-user-password"
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                  placeholder="Minimum 8 characters"
                />
              </div>
              <div>
                <label htmlFor="create-user-role" className="block text-sm font-medium text-dark-100 mb-1">
                  Role
                </label>
                <select
                  id="create-user-role"
                  value={role}
                  onChange={(e) => setRole(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                >
                  <option value="user">User</option>
                  <option value="reseller">Reseller</option>
                  <option value="admin">Admin</option>
                </select>
                <p className="text-xs text-dark-400 mt-1">Admin has full access, User has limited access</p>
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-6">
              <button
                onClick={() => {
                  setShowCreate(false);
                  setEmail("");
                  setPassword("");
                }}
                className="px-4 py-2 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors"
              >
                Cancel
              </button>
              <button
                onClick={handleCreate}
                disabled={creating || !email || password.length < 8}
                className="flex items-center gap-2 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {creating && <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />}
                {creating ? "Creating..." : "Create User"}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Reset Password dialog */}
      {resetPwTarget && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 dp-modal-overlay"
          role="dialog"
          aria-labelledby="reset-pw-title"
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              setResetPwTarget(null);
              setResetPwValue("");
            }
          }}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-[420px] dp-modal">
            <h3 id="reset-pw-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-4">
              Reset Password
            </h3>
            <p className="text-sm text-dark-200 mb-4">
              Set a new password for <span className="font-mono text-dark-50">{users.find(u => u.id === resetPwTarget)?.email}</span>
            </p>
            <div>
              <label htmlFor="reset-pw-input" className="block text-sm font-medium text-dark-100 mb-1">
                New Password
              </label>
              <input
                id="reset-pw-input"
                type="password"
                value={resetPwValue}
                onChange={(e) => setResetPwValue(e.target.value)}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                placeholder="Minimum 8 characters"
                autoFocus
                onKeyDown={(e) => {
                  if (e.key === "Enter" && resetPwValue.length >= 8) {
                    handleResetPassword(resetPwTarget);
                  }
                }}
              />
            </div>
            <div className="flex justify-end gap-2 mt-6">
              <button
                onClick={() => {
                  setResetPwTarget(null);
                  setResetPwValue("");
                }}
                className="px-4 py-2 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors"
              >
                Cancel
              </button>
              <button
                onClick={() => handleResetPassword(resetPwTarget)}
                disabled={resettingPw || resetPwValue.length < 8}
                className="flex items-center gap-2 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {resettingPw && <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />}
                {resettingPw ? "Resetting..." : "Reset Password"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
