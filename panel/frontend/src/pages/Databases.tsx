import { useState, useEffect, FormEvent } from "react";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface Site {
  id: string;
  domain: string;
}

interface Database {
  id: string;
  site_id: string;
  name: string;
  engine: string;
  db_user: string;
  container_id: string | null;
  port: number | null;
  created_at: string;
}

interface Credentials {
  host: string;
  port: number;
  database: string;
  username: string;
  password: string;
  engine: string;
  connection_string: string;
  internal_host: string;
}

const engineLabels: Record<string, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL",
  mariadb: "MariaDB",
};

export default function Databases() {
  const [databases, setDatabases] = useState<Database[]>([]);
  const [sites, setSites] = useState<Site[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [error, setError] = useState("");

  // Form state
  const [siteId, setSiteId] = useState("");
  const [dbName, setDbName] = useState("");
  const [engine, setEngine] = useState("postgres");
  const [submitting, setSubmitting] = useState(false);

  // Delete state
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  // Credentials state
  const [credentialsId, setCredentialsId] = useState<string | null>(null);
  const [credentials, setCredentials] = useState<Credentials | null>(null);
  const [credentialsLoading, setCredentialsLoading] = useState(false);
  const [copied, setCopied] = useState("");

  const fetchData = async () => {
    try {
      const [dbs, sitesData] = await Promise.all([
        api.get<Database[]>("/databases"),
        api.get<Site[]>("/sites"),
      ]);
      setDatabases(dbs);
      setSites(sitesData);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load data");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchData();
  }, []);

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      await api.post("/databases", {
        site_id: siteId,
        name: dbName,
        engine,
      });
      setShowForm(false);
      setDbName("");
      fetchData();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create database");
    } finally {
      setSubmitting(false);
    }
  };

  const handleDelete = async (id: string) => {
    if (confirmDeleteId !== id) {
      setConfirmDeleteId(id);
      return;
    }
    setDeletingId(id);
    try {
      await api.delete(`/databases/${id}`);
      setConfirmDeleteId(null);
      if (credentialsId === id) {
        setCredentialsId(null);
        setCredentials(null);
      }
      fetchData();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed");
    } finally {
      setDeletingId(null);
    }
  };

  const toggleCredentials = async (id: string) => {
    if (credentialsId === id) {
      setCredentialsId(null);
      setCredentials(null);
      return;
    }
    setCredentialsId(id);
    setCredentials(null);
    setCredentialsLoading(true);
    try {
      const creds = await api.get<Credentials>(`/databases/${id}/credentials`);
      setCredentials(creds);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load credentials");
      setCredentialsId(null);
    } finally {
      setCredentialsLoading(false);
    }
  };

  const copyToClipboard = (text: string, label: string) => {
    navigator.clipboard.writeText(text);
    setCopied(label);
    setTimeout(() => setCopied(""), 2000);
  };

  const getSiteDomain = (siteId: string) =>
    sites.find((s) => s.id === siteId)?.domain || "Unknown";

  return (
    <div className="p-6 lg:p-8 animate-fade-up">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Databases</h1>
        <button
          onClick={() => setShowForm(!showForm)}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
        >
          {showForm ? "Cancel" : "Create Database"}
        </button>
      </div>

      {error && (
        <div role="alert" className="bg-red-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-red-500/20 mb-4">
          {error}
          <button onClick={() => setError("")} className="float-right font-bold" aria-label="Close error">
            &times;
          </button>
        </div>
      )}

      {/* Create form */}
      {showForm && (
        <form
          onSubmit={handleCreate}
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6 space-y-4"
        >
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            <div>
              <label htmlFor="db-site" className="block text-sm font-medium text-dark-100 mb-1">
                Site
              </label>
              <select
                id="db-site"
                value={siteId}
                onChange={(e) => {
                  const val = e.target.value;
                  setSiteId(val);
                  if (val) {
                    const selectedSite = sites.find((s) => s.id === val);
                    if (selectedSite) {
                      setDbName(selectedSite.domain.replace(/\./g, '_').replace(/-/g, '_'));
                    }
                  }
                }}
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm bg-dark-800"
              >
                <option value="">-- Select a site --</option>
                {sites.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.domain}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label htmlFor="db-name" className="block text-sm font-medium text-dark-100 mb-1">
                Database Name
              </label>
              <input
                id="db-name"
                type="text"
                value={dbName}
                onChange={(e) => setDbName(e.target.value)}
                required
                placeholder="my_database"
                pattern="[a-zA-Z0-9_]+"
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm"
              />
            </div>
            <div>
              <label htmlFor="db-engine" className="block text-sm font-medium text-dark-100 mb-1">
                Engine
              </label>
              <select
                id="db-engine"
                value={engine}
                onChange={(e) => setEngine(e.target.value)}
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm bg-dark-800"
              >
                <option value="postgres">PostgreSQL 16</option>
                <option value="mariadb">MariaDB 11</option>
              </select>
            </div>
          </div>
          <button
            type="submit"
            disabled={submitting}
            className="px-6 py-2.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
          >
            {submitting ? "Creating..." : "Create Database"}
          </button>
        </form>
      )}

      {/* Database list */}
      {loading ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 animate-pulse">
          {[...Array(3)].map((_, i) => (
            <div key={i} className="px-5 py-4 border-b border-dark-600 last:border-0">
              <div className="h-5 bg-dark-700 rounded w-48" />
            </div>
          ))}
        </div>
      ) : databases.length === 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <svg
            className="w-12 h-12 mx-auto text-dark-300 mb-4"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={1}
            aria-hidden="true"
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M20.25 6.375c0 2.278-3.694 4.125-8.25 4.125S3.75 8.653 3.75 6.375m16.5 0c0-2.278-3.694-4.125-8.25-4.125S3.75 4.097 3.75 6.375m16.5 0v11.25c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125V6.375m16.5 0v3.75m-16.5-3.75v3.75m16.5 0v3.75C20.25 16.153 16.556 18 12 18s-8.25-1.847-8.25-4.125v-3.75m16.5 0c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125"
            />
          </svg>
          <p className="text-dark-200 font-medium">No databases yet</p>
          <p className="text-dark-300 text-sm mt-1">
            Create a database for any of your sites
          </p>
          <button onClick={() => setShowForm(true)} className="mt-3 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors">
            Create your first database
          </button>
        </div>
      ) : (
        <div className="space-y-0">
          <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
            <table className="w-full">
              <thead>
                <tr className="border-b border-dark-500 bg-dark-900">
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3">
                    Name
                  </th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3">
                    Engine
                  </th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3 hidden md:table-cell">
                    Site
                  </th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3 hidden sm:table-cell">
                    Port
                  </th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3 hidden lg:table-cell">
                    Created
                  </th>
                  <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3">
                    Actions
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-600">
                {databases.map((db) => (
                  <tr key={db.id} className="hover:bg-dark-700/30 transition-colors">
                    <td className="px-5 py-4 text-sm font-medium text-dark-50 font-mono">
                      {db.name}
                    </td>
                    <td className="px-5 py-4 text-sm text-dark-200">
                      {engineLabels[db.engine] || db.engine}
                    </td>
                    <td className="px-5 py-4 text-sm text-dark-200 hidden md:table-cell font-mono">
                      {getSiteDomain(db.site_id)}
                    </td>
                    <td className="px-5 py-4 text-sm text-dark-200 font-mono hidden sm:table-cell">
                      {db.port || "\u2014"}
                    </td>
                    <td className="px-5 py-4 text-sm text-dark-200 hidden lg:table-cell">
                      {formatDate(db.created_at)}
                    </td>
                    <td className="px-5 py-4 text-right">
                      <div className="flex items-center justify-end gap-2">
                        <button
                          onClick={() => toggleCredentials(db.id)}
                          className={`px-2 py-1 rounded text-xs font-medium transition-colors ${
                            credentialsId === db.id
                              ? "bg-rust-500/15 text-rust-400"
                              : "bg-dark-700 text-dark-200 hover:bg-dark-600"
                          }`}
                        >
                          {credentialsId === db.id ? "Hide" : "Connect"}
                        </button>
                        {confirmDeleteId === db.id ? (
                          <>
                            <span className="text-xs text-danger-400">Sure?</span>
                            <button
                              onClick={() => handleDelete(db.id)}
                              disabled={deletingId === db.id}
                              className="px-2 py-1 bg-red-600 text-white rounded text-xs hover:bg-red-700 disabled:opacity-50"
                            >
                              {deletingId === db.id ? "..." : "Yes"}
                            </button>
                            <button
                              onClick={() => setConfirmDeleteId(null)}
                              className="px-2 py-1 bg-dark-600 text-dark-100 rounded text-xs hover:bg-dark-500"
                            >
                              No
                            </button>
                          </>
                        ) : (
                          <button
                            onClick={() => handleDelete(db.id)}
                            className="text-xs text-danger-500 hover:text-danger-500"
                          >
                            Delete
                          </button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* Credentials panel */}
          {credentialsId && (
            <div className="mt-4 bg-dark-800 rounded-lg border border-dark-500 p-5">
              {credentialsLoading ? (
                <div className="text-center text-dark-300 py-4">Loading credentials...</div>
              ) : credentials ? (
                <div>
                  <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-4">
                    Connection Details
                  </h3>

                  {/* Connection string */}
                  <div className="mb-4">
                    <label className="block text-xs font-medium text-dark-200 mb-1">
                      Connection String
                    </label>
                    <div className="flex items-center gap-2">
                      <code className="flex-1 bg-dark-900 border border-dark-500 rounded-lg px-3 py-2 text-xs font-mono text-dark-100 overflow-x-auto">
                        {credentials.connection_string}
                      </code>
                      <button
                        onClick={() =>
                          copyToClipboard(credentials.connection_string, "connection_string")
                        }
                        className="shrink-0 px-3 py-2 bg-dark-700 text-dark-200 rounded-lg text-xs hover:bg-dark-600 transition-colors"
                      >
                        {copied === "connection_string" ? "Copied!" : "Copy"}
                      </button>
                    </div>
                  </div>

                  {/* Individual fields */}
                  <div className="grid grid-cols-2 md:grid-cols-3 gap-3">
                    {[
                      { label: "Host", value: credentials.host, key: "host" },
                      { label: "Port", value: String(credentials.port), key: "port" },
                      { label: "Database", value: credentials.database, key: "database" },
                      { label: "Username", value: credentials.username, key: "username" },
                      { label: "Password", value: credentials.password, key: "password" },
                      { label: "Internal Host", value: credentials.internal_host, key: "internal_host" },
                    ].map((field) => (
                      <div key={field.key}>
                        <label className="block text-xs font-medium text-dark-200 mb-1">
                          {field.label}
                        </label>
                        <div className="flex items-center gap-1">
                          <code className="flex-1 bg-dark-900 border border-dark-500 rounded px-2 py-1.5 text-xs font-mono text-dark-100 truncate">
                            {field.key === "password" ? "\u2022\u2022\u2022\u2022\u2022\u2022\u2022\u2022" : field.value}
                          </code>
                          <button
                            onClick={() => copyToClipboard(field.value, field.key)}
                            className="shrink-0 p-1.5 text-dark-300 hover:text-dark-200"
                            title={`Copy ${field.label}`}
                          >
                            {copied === field.key ? (
                              <svg className="w-3.5 h-3.5 text-rust-500" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                                <path strokeLinecap="round" strokeLinejoin="round" d="M4.5 12.75l6 6 9-13.5" />
                              </svg>
                            ) : (
                              <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                                <path strokeLinecap="round" strokeLinejoin="round" d="M15.666 3.888A2.25 2.25 0 0 0 13.5 2.25h-3c-1.03 0-1.9.693-2.166 1.638m7.332 0c.055.194.084.4.084.612v0a.75.75 0 0 1-.75.75H9.75a.75.75 0 0 1-.75-.75v0c0-.212.03-.418.084-.612m7.332 0c.646.049 1.288.11 1.927.184 1.1.128 1.907 1.077 1.907 2.185V19.5a2.25 2.25 0 0 1-2.25 2.25H6.75A2.25 2.25 0 0 1 4.5 19.5V6.257c0-1.108.806-2.057 1.907-2.185a48.208 48.208 0 0 1 1.927-.184" />
                              </svg>
                            )}
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              ) : null}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
