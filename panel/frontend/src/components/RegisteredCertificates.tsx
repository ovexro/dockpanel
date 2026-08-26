import { useState, useEffect } from "react";
import { api } from "../api";
import { STATUS_STYLES } from "../pages/Certificates";

/**
 * A certificate registered once, by name, and referenced by that name when a
 * stack claims a domain (#104). Metadata only — the PEM pair lives on the
 * server's disk and never comes back through the API.
 */
interface RegisteredCertificate {
  id: string;
  alias: string;
  dns_names: string[];
  issuer: string | null;
  not_before: string | null;
  not_after: string | null;
  fingerprint_sha256: string | null;
  /** null when the panel does not know this certificate's expiry. */
  days_left: number | null;
  /** The same vocabulary the site table paints — one map, one ladder. */
  status: string;
  /** The stacks whose vhost is written against this pair; delete is refused while non-empty. */
  in_use_by: { id: string; name: string; domain: string | null }[];
  created_at: string;
  updated_at: string;
}

// The same grammar the backend and agent validate — a DNS-label shape, 1–64
// chars, because the alias becomes a directory name on the agent. Checked
// here only so an obvious slip is named before a round trip, never instead of
// the server's own check.
const ALIAS_RE = /^[a-z0-9]([a-z0-9-]{0,62}[a-z0-9])?$/;

const PEM_CLASS =
  "w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-xs font-mono text-dark-100 focus:ring-2 focus:ring-accent-500 outline-none";

/** The site table's colour rule for the countdown column, verbatim. */
function daysLeftClass(days: number | null): string {
  return days === null
    ? "text-dark-300"
    : days <= 7
      ? "text-danger-400 font-bold"
      : days <= 30
        ? "text-warn-400"
        : "text-dark-100";
}

function daysLeftText(days: number | null): string {
  return days === null ? "—" : days < 0 ? `${Math.abs(days)}d overdue` : `${days}d`;
}

export default function RegisteredCertificates() {
  const [rows, setRows] = useState<RegisteredCertificate[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [message, setMessage] = useState({ text: "", type: "" });

  // Register form.
  const [alias, setAlias] = useState("");
  const [certPem, setCertPem] = useState("");
  const [keyPem, setKeyPem] = useState("");
  const [registering, setRegistering] = useState(false);

  // Replace: one row at a time reveals its own pair of textareas.
  const [replaceTarget, setReplaceTarget] = useState<string | null>(null);
  const [replaceCert, setReplaceCert] = useState("");
  const [replaceKey, setReplaceKey] = useState("");
  const [replacingId, setReplacingId] = useState<string | null>(null);

  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);

  const load = () => {
    api.get<RegisteredCertificate[]>("/tls-certificates")
      .then((data) => {
        setRows(data || []);
        // Clears a previous failure, same as the site list above: the banner
        // must not keep describing a request that has since succeeded.
        setError("");
      })
      .catch((e) => setError(e instanceof Error ? e.message : "Failed to load registered certificates"))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    load();
  }, []);

  const handleRegister = async () => {
    const name = alias.trim().toLowerCase();
    setMessage({ text: "", type: "" });
    if (!ALIAS_RE.test(name)) {
      setMessage({
        text: "The alias must be 1–64 characters of lowercase letters, digits and hyphens, starting and ending with a letter or digit.",
        type: "error",
      });
      return;
    }
    setRegistering(true);
    try {
      await api.post("/tls-certificates", { alias: name, certificate: certPem, private_key: keyPem });
      setMessage({ text: `Certificate "${name}" registered`, type: "success" });
      setAlias("");
      setCertPem("");
      setKeyPem("");
      load();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to register certificate", type: "error" });
    } finally {
      setRegistering(false);
    }
  };

  const handleReplace = async (row: RegisteredCertificate) => {
    setReplacingId(row.id);
    setMessage({ text: "", type: "" });
    try {
      await api.put(`/tls-certificates/${row.id}`, { certificate: replaceCert, private_key: replaceKey });
      setReplaceTarget(null);
      setReplaceCert("");
      setReplaceKey("");
      setMessage({ text: `Certificate "${row.alias}" replaced`, type: "success" });
      load();
    } catch (e) {
      // The backend passes the agent's sentence through: a key that does not
      // match, or a new certificate that no longer covers a claiming stack.
      setMessage({ text: e instanceof Error ? e.message : "Failed to replace certificate", type: "error" });
    } finally {
      setReplacingId(null);
    }
  };

  const handleDelete = async (row: RegisteredCertificate) => {
    setDeletingId(row.id);
    setMessage({ text: "", type: "" });
    try {
      await api.delete(`/tls-certificates/${row.id}`);
      setDeleteTarget(null);
      setMessage({ text: `Certificate "${row.alias}" deleted`, type: "success" });
      load();
    } catch (e) {
      // A 409 arrives with the sentence naming the stacks still using it;
      // shown as-is so the operator knows what to redeploy first.
      setMessage({ text: e instanceof Error ? e.message : "Failed to delete certificate", type: "error" });
      setDeleteTarget(null);
    } finally {
      setDeletingId(null);
    }
  };

  const canRegister = alias.trim() !== "" && certPem.trim() !== "" && keyPem.trim() !== "";

  return (
    <div className="mt-8">
      <div className="flex items-center justify-between gap-4 mb-4">
        <div>
          <h2 className="text-sm font-medium text-dark-50 font-mono">Registered certificates</h2>
          <p className="text-xs text-dark-300 mt-1">
            Upload a certificate once under a name, then pick that name when a stack claims a domain.
            The key pair is written to this server only; the panel keeps the metadata.
          </p>
        </div>
        <p className="text-sm text-dark-200 font-mono whitespace-nowrap">
          {error
            ? "Registry unavailable"
            : loading
              ? "Loading…"
              : rows.length > 0
                ? `${rows.length} registered`
                : "None registered"}
        </p>
      </div>

      {error && (
        <div className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20 mb-4">
          {error}
          <button onClick={() => setError("")} className="ml-2 font-medium hover:underline">Dismiss</button>
        </div>
      )}

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
              : "bg-danger-500/10 text-danger-400 border-danger-500/20"
          }`}
        >
          {message.text}
          <button onClick={() => setMessage({ text: "", type: "" })} className="ml-2 font-medium hover:underline">Dismiss</button>
        </div>
      )}

      {!loading && !error && rows.length === 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-8 text-center mb-4">
          <p className="text-dark-200 text-sm">
            No certificates registered on this server. Register one below to reference it by name from a stack.
          </p>
        </div>
      ) : rows.length > 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto mb-4">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-dark-600">
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Alias</th>
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Names</th>
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Issuer</th>
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Expiry</th>
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Days Left</th>
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Status</th>
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">In Use</th>
                <th className="text-right px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Actions</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => {
                const style = STATUS_STYLES[row.status] || STATUS_STYLES.unknown;
                const inUse = row.in_use_by.length;
                const busy = replacingId === row.id || deletingId === row.id;
                return (
                  <tr key={row.id} className="border-b border-dark-600/50 last:border-b-0 align-top">
                    <td className="px-4 py-3 font-mono text-dark-50" title={row.fingerprint_sha256 ? `SHA-256 ${row.fingerprint_sha256}` : undefined}>
                      {row.alias}
                    </td>
                    <td className="px-4 py-3 font-mono text-dark-200 break-all">
                      {row.dns_names.length > 0 ? row.dns_names.join(", ") : <span className="text-dark-300">—</span>}
                    </td>
                    <td className="px-4 py-3 text-dark-200">{row.issuer ?? <span className="text-dark-300">—</span>}</td>
                    <td className="px-4 py-3 text-dark-200 font-mono">
                      {row.not_after ? new Date(row.not_after).toLocaleDateString() : "Unknown"}
                    </td>
                    <td className="px-4 py-3 font-mono">
                      <span className={daysLeftClass(row.days_left)}>{daysLeftText(row.days_left)}</span>
                    </td>
                    <td className="px-4 py-3">
                      <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${style.bg} ${style.text}`}>
                        {style.label}
                      </span>
                    </td>
                    <td className="px-4 py-3 text-dark-200">
                      <span
                        className={inUse > 0 ? "text-dark-100" : "text-dark-300"}
                        title={inUse > 0 ? row.in_use_by.map((s) => (s.domain ? `${s.name} (${s.domain})` : s.name)).join("\n") : "No stack references this certificate"}
                      >
                        in use by {inUse} stack{inUse === 1 ? "" : "s"}
                      </span>
                    </td>
                    <td className="px-4 py-3 text-right">
                      {deleteTarget === row.id ? (
                        <div className="flex items-center justify-end gap-1">
                          <button
                            onClick={() => handleDelete(row)}
                            disabled={deletingId === row.id}
                            className="px-2 py-1 bg-danger-500 text-white rounded text-xs disabled:opacity-50 flex items-center gap-1"
                          >
                            {deletingId === row.id && <span className="w-3 h-3 border-2 border-white/30 border-t-white rounded-full animate-spin" />}
                            Confirm
                          </button>
                          <button
                            onClick={() => setDeleteTarget(null)}
                            className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs"
                          >
                            Cancel
                          </button>
                        </div>
                      ) : replaceTarget === row.id ? (
                        <div className="w-72 ml-auto space-y-2 text-left">
                          <textarea
                            value={replaceCert}
                            onChange={(e) => setReplaceCert(e.target.value)}
                            placeholder={"-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----"}
                            rows={4}
                            spellCheck={false}
                            className={PEM_CLASS}
                          />
                          <textarea
                            value={replaceKey}
                            onChange={(e) => setReplaceKey(e.target.value)}
                            placeholder={"-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----"}
                            rows={4}
                            spellCheck={false}
                            className={PEM_CLASS}
                          />
                          {inUse > 0 && (
                            <p className="text-[11px] text-dark-300">
                              The new certificate must still cover every domain of the {inUse} stack{inUse === 1 ? "" : "s"} using it; live vhosts are reloaded on success.
                            </p>
                          )}
                          <div className="flex items-center justify-end gap-1">
                            <button
                              onClick={() => handleReplace(row)}
                              disabled={replacingId === row.id || !replaceCert.trim() || !replaceKey.trim()}
                              className="px-2 py-1 bg-rust-500 text-white rounded text-xs disabled:opacity-50 flex items-center gap-1"
                            >
                              {replacingId === row.id && <span className="w-3 h-3 border-2 border-white/30 border-t-white rounded-full animate-spin" />}
                              {replacingId === row.id ? "Replacing..." : "Replace"}
                            </button>
                            <button
                              onClick={() => { setReplaceTarget(null); setReplaceCert(""); setReplaceKey(""); }}
                              className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs"
                            >
                              Cancel
                            </button>
                          </div>
                        </div>
                      ) : (
                        <div className="flex items-center justify-end gap-1">
                          <button
                            onClick={() => { setReplaceTarget(row.id); setReplaceCert(""); setReplaceKey(""); }}
                            disabled={busy}
                            className="px-2 py-1 text-xs text-dark-300 hover:text-dark-50 bg-dark-700 rounded hover:bg-dark-600 transition-colors disabled:opacity-50"
                            title="Upload a new certificate and key under this name"
                          >
                            Replace
                          </button>
                          <button
                            onClick={() => setDeleteTarget(row.id)}
                            disabled={busy}
                            className="text-dark-300 hover:text-danger-500 transition-colors p-1 disabled:opacity-50"
                            title={inUse > 0 ? "Refused while a stack still uses it" : "Delete certificate"}
                          >
                            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                              <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                            </svg>
                          </button>
                        </div>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : null}

      <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
        <h3 className="text-sm font-medium text-dark-50 mb-3">Register a certificate</h3>
        <div className="mb-3">
          <label htmlFor="registry-alias" className="block text-xs font-medium text-dark-200 mb-1">Alias</label>
          <input
            id="registry-alias"
            type="text"
            value={alias}
            onChange={(e) => setAlias(e.target.value)}
            placeholder="wildcard-example-com"
            autoComplete="off"
            spellCheck={false}
            className="w-full md:w-1/2 px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-100 focus:ring-2 focus:ring-accent-500 outline-none"
          />
          <p className="text-xs text-dark-300 mt-1">
            Lowercase letters, digits and hyphens, 1–64 characters. This is the name a stack references.
          </p>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-3">
          <div>
            <label htmlFor="registry-cert" className="block text-xs font-medium text-dark-200 mb-1">Certificate (full chain, PEM)</label>
            <textarea
              id="registry-cert"
              value={certPem}
              onChange={(e) => setCertPem(e.target.value)}
              placeholder={"-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----"}
              rows={4}
              spellCheck={false}
              className={PEM_CLASS}
            />
          </div>
          <div>
            <label htmlFor="registry-key" className="block text-xs font-medium text-dark-200 mb-1">Private key (PEM)</label>
            <textarea
              id="registry-key"
              value={keyPem}
              onChange={(e) => setKeyPem(e.target.value)}
              placeholder={"-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----"}
              rows={4}
              spellCheck={false}
              className={PEM_CLASS}
            />
          </div>
        </div>
        <div className="flex items-center justify-between gap-4">
          <p className="text-xs text-dark-300">
            The key must belong to the certificate; both are checked before anything is written.
          </p>
          <button
            onClick={handleRegister}
            disabled={registering || !canRegister}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 flex items-center gap-2"
          >
            {registering && <span className="w-3 h-3 border-2 border-white/30 border-t-white rounded-full animate-spin" />}
            {registering ? "Registering..." : "Register"}
          </button>
        </div>
      </div>
    </div>
  );
}
