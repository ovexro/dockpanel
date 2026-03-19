import { useState, useEffect } from "react";
import { api } from "../api";

interface Certificate {
  domain: string;
  expiry: string | null;
  days_left: number;
  status: string;
}

const STATUS_STYLES: Record<string, { bg: string; text: string; label: string }> = {
  expired: { bg: "bg-danger-500/15", text: "text-danger-400", label: "Expired" },
  critical: { bg: "bg-danger-500/15", text: "text-danger-400", label: "Critical" },
  warning: { bg: "bg-warn-500/15", text: "text-warn-400", label: "Warning" },
  ok: { bg: "bg-rust-500/15", text: "text-rust-400", label: "OK" },
};

export default function Certificates() {
  const [certs, setCerts] = useState<Certificate[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    api.get<{ certificates: Certificate[] }>("/monitors/certificates")
      .then((data) => setCerts(data.certificates || []))
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, []);

  if (loading) {
    return (
      <div className="animate-fade-up">
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div>
      <p className="text-sm text-dark-200 font-mono mb-4">
        {certs.length > 0
          ? `${certs.length} SSL certificate${certs.length > 1 ? "s" : ""} tracked`
          : "No SSL certificates found"}
      </p>

      {error && (
        <div className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20 mb-4">
          {error}
          <button onClick={() => setError("")} className="ml-2 font-medium hover:underline">Dismiss</button>
        </div>
      )}

      {certs.length === 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <svg className="w-12 h-12 text-dark-300 mx-auto mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75 11.25 15 15 9.75m-3-7.036A11.959 11.959 0 0 1 3.598 6 11.99 11.99 0 0 0 3 9.749c0 5.592 3.824 10.29 9 11.623 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.571-.598-3.751h-.152c-3.196 0-6.1-1.248-8.25-3.285Z" />
          </svg>
          <p className="text-dark-200 text-sm">No SSL certificates. Enable SSL on your sites to see them here.</p>
        </div>
      ) : (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-dark-600">
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Domain</th>
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Expiry</th>
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Days Left</th>
                <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase font-mono">Status</th>
              </tr>
            </thead>
            <tbody>
              {certs.map((cert) => {
                const style = STATUS_STYLES[cert.status] || STATUS_STYLES.ok;
                return (
                  <tr key={cert.domain} className="border-b border-dark-600/50 last:border-b-0">
                    <td className="px-4 py-3 font-mono text-dark-50">{cert.domain}</td>
                    <td className="px-4 py-3 text-dark-200 font-mono">
                      {cert.expiry ? new Date(cert.expiry).toLocaleDateString() : "Unknown"}
                    </td>
                    <td className="px-4 py-3 font-mono">
                      <span className={cert.days_left <= 7 ? "text-danger-400 font-bold" : cert.days_left <= 30 ? "text-warn-400" : "text-dark-100"}>
                        {cert.days_left < 0 ? `${Math.abs(cert.days_left)}d overdue` : `${cert.days_left}d`}
                      </span>
                    </td>
                    <td className="px-4 py-3">
                      <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${style.bg} ${style.text}`}>
                        {style.label}
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
