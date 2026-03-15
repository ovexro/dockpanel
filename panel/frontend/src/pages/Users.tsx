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

  if (error) {
    return (
      <div className="p-6 lg:p-8 animate-fade-up">
        <div className="bg-red-500/10 text-red-400 px-4 py-3 rounded-lg border border-red-500/20">
          {error}
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Users</h1>
          <p className="text-sm text-dark-200 font-mono mt-1">
            Manage user accounts and permissions
          </p>
        </div>
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

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
              : "bg-red-500/10 text-red-400 border-red-500/20"
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
        ) : users.length === 0 ? (
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
                <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-28">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-600">
              {users.map((user) => (
                <tr key={user.id} className="hover:bg-dark-700/30 transition-colors">
                  <td className="px-5 py-4">
                    <div className="flex items-center gap-3">
                      <div className="w-8 h-8 rounded-full bg-rust-500/15 text-rust-500 flex items-center justify-center text-sm font-medium">{user.email[0].toUpperCase()}</div>
                      <span className="text-sm text-dark-50 font-mono">{user.email}</span>
                    </div>
                  </td>
                  <td className="px-5 py-4">
                    {editTarget === user.id ? (
                      <select value={editRole} onChange={(e) => setEditRole(e.target.value)} onBlur={() => { if (editRole !== user.role) handleUpdateRole(user.id, editRole); else setEditTarget(null); }} autoFocus className="text-sm border border-dark-500 rounded px-2 py-1">
                        <option value="admin">admin</option>
                        <option value="user">user</option>
                      </select>
                    ) : (
                      <button onClick={() => { setEditTarget(user.id); setEditRole(user.role); }} className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium cursor-pointer ${user.role === "admin" ? "bg-purple-500/15 text-purple-400" : "bg-blue-500/15 text-blue-400"}`}>{user.role}</button>
                    )}
                  </td>
                  <td className="px-5 py-4 text-sm text-dark-200">{user.site_count}</td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">{new Date(user.created_at).toLocaleDateString()}</td>
                  <td className="px-5 py-4 text-right">
                    {deleteTarget === user.id ? (
                      <div className="flex items-center justify-end gap-1">
                        <button onClick={() => handleDelete(user.id)} className="px-2 py-1 bg-red-600 text-white rounded text-xs">Confirm</button>
                        <button onClick={() => setDeleteTarget(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs">Cancel</button>
                      </div>
                    ) : (
                      <button onClick={() => setDeleteTarget(user.id)} className="text-dark-300 hover:text-red-600 transition-colors" title="Delete user">
                        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" /></svg>
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>

          {/* Mobile Cards */}
          <div className="sm:hidden divide-y divide-dark-600">
            {users.map((user) => (
              <div key={user.id} className="px-4 py-3">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2.5 min-w-0">
                    <div className="w-8 h-8 rounded-full bg-rust-500/15 text-rust-500 flex items-center justify-center text-sm font-medium shrink-0">{user.email[0].toUpperCase()}</div>
                    <div className="min-w-0">
                      <span className="text-sm text-dark-50 font-mono block truncate">{user.email}</span>
                      <span className="text-xs text-dark-300 font-mono">{new Date(user.created_at).toLocaleDateString()}</span>
                    </div>
                  </div>
                  <div className="flex items-center gap-2 shrink-0 ml-2">
                    <button onClick={() => { setEditTarget(user.id); setEditRole(user.role); }} className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${user.role === "admin" ? "bg-purple-500/15 text-purple-400" : "bg-blue-500/15 text-blue-400"}`}>{user.role}</button>
                    {deleteTarget === user.id ? (
                      <div className="flex items-center gap-1">
                        <button onClick={() => handleDelete(user.id)} className="px-2 py-1 bg-red-600 text-white rounded text-xs">Del</button>
                        <button onClick={() => setDeleteTarget(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs">No</button>
                      </div>
                    ) : (
                      <button onClick={() => setDeleteTarget(user.id)} className="p-1.5 text-dark-300 hover:text-red-600">
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

      {/* Create dialog */}
      {showCreate && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50"
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
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-[420px]">
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
                  <option value="admin">Admin</option>
                </select>
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-6">
              <button
                onClick={() => {
                  setShowCreate(false);
                  setEmail("");
                  setPassword("");
                }}
                className="px-4 py-2 text-sm text-dark-200 hover:text-dark-100"
              >
                Cancel
              </button>
              <button
                onClick={handleCreate}
                disabled={creating || !email || password.length < 8}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {creating ? "Creating..." : "Create User"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
