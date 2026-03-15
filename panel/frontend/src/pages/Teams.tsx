import { useState, useEffect, FormEvent } from "react";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface TeamMember {
  id: string;
  user_id: string;
  role: string;
  email: string;
  joined_at: string;
}

interface Team {
  id: string;
  name: string;
  owner_id: string;
  owner_email: string;
  is_owner: boolean;
  members: TeamMember[];
  created_at: string;
}

const roleColors: Record<string, string> = {
  owner: "bg-rust-500/15 text-rust-400",
  admin: "bg-violet-500/15 text-violet-400",
  developer: "bg-rust-500/15 text-rust-400",
  viewer: "bg-dark-700 text-dark-200",
};

export default function Teams() {
  const [teams, setTeams] = useState<Team[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [teamName, setTeamName] = useState("");
  const [submitting, setSubmitting] = useState(false);

  // Invite state
  const [inviteTeamId, setInviteTeamId] = useState<string | null>(null);
  const [inviteEmail, setInviteEmail] = useState("");
  const [inviteRole, setInviteRole] = useState("developer");

  // Accept invite from URL
  const [acceptToken, setAcceptToken] = useState("");

  const fetchTeams = () => {
    api.get<Team[]>("/teams")
      .then(setTeams)
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const token = params.get("token");
    if (token) {
      setAcceptToken(token);
      window.history.replaceState({}, "", "/teams");
    }
    fetchTeams();
  }, []);

  useEffect(() => {
    if (acceptToken) {
      api.post<{ team: string }>("/teams/accept", { token: acceptToken })
        .then((res) => {
          setSuccess(`Joined team "${res.team}"!`);
          setAcceptToken("");
          fetchTeams();
        })
        .catch((e) => setError(e.message));
    }
  }, [acceptToken]);

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      await api.post("/teams", { name: teamName });
      setShowCreate(false);
      setTeamName("");
      fetchTeams();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create team");
    } finally {
      setSubmitting(false);
    }
  };

  const handleDelete = async (id: string, name: string) => {
    if (!confirm(`Delete team "${name}"? All members will be removed.`)) return;
    try {
      await api.delete(`/teams/${id}`);
      fetchTeams();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed");
    }
  };

  const handleInvite = async (e: FormEvent) => {
    e.preventDefault();
    if (!inviteTeamId) return;
    setError("");
    try {
      await api.post(`/teams/${inviteTeamId}/invite`, { email: inviteEmail, role: inviteRole });
      setSuccess(`Invitation sent to ${inviteEmail}`);
      setInviteTeamId(null);
      setInviteEmail("");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Invite failed");
    }
  };

  const handleRoleChange = async (teamId: string, memberId: string, role: string) => {
    try {
      await api.put(`/teams/${teamId}/members/${memberId}`, { role });
      fetchTeams();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Role update failed");
    }
  };

  const handleRemoveMember = async (teamId: string, memberId: string, email: string) => {
    if (!confirm(`Remove ${email} from the team?`)) return;
    try {
      await api.delete(`/teams/${teamId}/members/${memberId}`);
      fetchTeams();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Remove failed");
    }
  };

  if (loading) {
    return (
      <div className="p-6 lg:p-8">
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Teams</h1>
          <p className="text-sm text-dark-200 font-mono mt-1">Collaborate with your team</p>
        </div>
        <button
          onClick={() => setShowCreate(!showCreate)}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
        >
          Create Team
        </button>
      </div>

      {error && (
        <div className="bg-red-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-red-500/20 mb-4">
          {error}
          <button onClick={() => setError("")} className="ml-2 font-medium hover:underline">Dismiss</button>
        </div>
      )}
      {success && (
        <div className="bg-rust-500/10 text-rust-400 text-sm px-4 py-3 rounded-lg border border-rust-500/20 mb-4">
          {success}
          <button onClick={() => setSuccess("")} className="ml-2 font-medium hover:underline">Dismiss</button>
        </div>
      )}

      {showCreate && (
        <form onSubmit={handleCreate} className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-3">New Team</h3>
          <div className="flex items-end gap-3">
            <div className="flex-1">
              <label htmlFor="team-name" className="block text-xs font-medium text-dark-200 mb-1">Team Name</label>
              <input id="team-name" type="text" value={teamName} onChange={(e) => setTeamName(e.target.value)} required placeholder="e.g. Engineering" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
            </div>
            <button type="submit" disabled={submitting} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">
              {submitting ? "Creating..." : "Create"}
            </button>
            <button type="button" onClick={() => setShowCreate(false)} className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600">Cancel</button>
          </div>
        </form>
      )}

      {teams.length === 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <svg className="w-12 h-12 text-dark-300 mx-auto mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M18 18.72a9.094 9.094 0 0 0 3.741-.479 3 3 0 0 0-4.682-2.72m.94 3.198.001.031c0 .225-.012.447-.037.666A11.944 11.944 0 0 1 12 21c-2.17 0-4.207-.576-5.963-1.584A6.062 6.062 0 0 1 6 18.719m12 0a5.971 5.971 0 0 0-.941-3.197m0 0A5.995 5.995 0 0 0 12 12.75a5.995 5.995 0 0 0-5.058 2.772m0 0a3 3 0 0 0-4.681 2.72 8.986 8.986 0 0 0 3.74.477m.94-3.197a5.971 5.971 0 0 0-.94 3.197M15 6.75a3 3 0 1 1-6 0 3 3 0 0 1 6 0Zm6 3a2.25 2.25 0 1 1-4.5 0 2.25 2.25 0 0 1 4.5 0Zm-13.5 0a2.25 2.25 0 1 1-4.5 0 2.25 2.25 0 0 1 4.5 0Z" />
          </svg>
          <p className="text-dark-200 text-sm">No teams yet. Create one to start collaborating.</p>
        </div>
      ) : (
        <div className="space-y-4">
          {teams.map((t) => (
            <div key={t.id} className="bg-dark-800 rounded-lg border border-dark-500 p-5">
              <div className="flex items-center justify-between mb-4">
                <div>
                  <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">{t.name}</h3>
                  <p className="text-xs text-dark-300 mt-0.5">{t.members.length} member{t.members.length !== 1 ? "s" : ""} · Created {formatDate(t.created_at)}</p>
                </div>
                <div className="flex items-center gap-2">
                  {t.is_owner && (
                    <button
                      onClick={() => setInviteTeamId(inviteTeamId === t.id ? null : t.id)}
                      className="px-3 py-1.5 bg-rust-500/10 text-rust-400 rounded-lg text-xs font-medium hover:bg-rust-500/20"
                    >
                      Invite
                    </button>
                  )}
                  {t.is_owner && (
                    <button
                      onClick={() => handleDelete(t.id, t.name)}
                      className="p-1.5 text-dark-300 hover:text-danger-500 transition-colors"
                      title="Delete team"
                      aria-label="Delete team"
                    >
                      <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                      </svg>
                    </button>
                  )}
                </div>
              </div>

              {/* Invite form */}
              {inviteTeamId === t.id && (
                <form onSubmit={handleInvite} className="bg-dark-900 rounded-lg p-4 mb-4">
                  <div className="flex items-end gap-3">
                    <div className="flex-1">
                      <label className="block text-xs font-medium text-dark-200 mb-1">Email</label>
                      <input type="email" value={inviteEmail} onChange={(e) => setInviteEmail(e.target.value)} required placeholder="user@example.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-200 mb-1">Role</label>
                      <select value={inviteRole} onChange={(e) => setInviteRole(e.target.value)} className="px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none">
                        <option value="admin">Admin</option>
                        <option value="developer">Developer</option>
                        <option value="viewer">Viewer</option>
                      </select>
                    </div>
                    <button type="submit" className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600">Send</button>
                  </div>
                </form>
              )}

              {/* Members */}
              <div className="space-y-2">
                {t.members.map((m) => (
                  <div key={m.id} className="flex items-center justify-between py-1.5">
                    <div className="flex items-center gap-3">
                      <div className="w-7 h-7 bg-dark-600 rounded-full flex items-center justify-center text-xs font-medium text-dark-200">
                        {m.email[0].toUpperCase()}
                      </div>
                      <span className="text-sm text-dark-100 font-mono">{m.email}</span>
                      <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${roleColors[m.role] || "bg-dark-700 text-dark-200"}`}>
                        {m.role}
                      </span>
                    </div>
                    {t.is_owner && m.role !== "owner" && (
                      <div className="flex items-center gap-2">
                        <select
                          value={m.role}
                          onChange={(e) => handleRoleChange(t.id, m.id, e.target.value)}
                          className="text-xs border border-dark-500 rounded px-2 py-1 outline-none"
                        >
                          <option value="admin">Admin</option>
                          <option value="developer">Developer</option>
                          <option value="viewer">Viewer</option>
                        </select>
                        <button
                          onClick={() => handleRemoveMember(t.id, m.id, m.email)}
                          className="text-xs text-danger-500 hover:text-danger-500"
                        >
                          Remove
                        </button>
                      </div>
                    )}
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
