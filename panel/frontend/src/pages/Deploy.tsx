import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface DeployConfig {
  id: string;
  site_id: string;
  repo_url: string;
  branch: string;
  deploy_script: string;
  auto_deploy: boolean;
  webhook_secret: string;
  deploy_key_public: string | null;
  deploy_key_path: string | null;
  last_deploy: string | null;
  last_status: string | null;
  created_at: string;
  updated_at: string;
}

interface DeployLog {
  id: string;
  site_id: string;
  commit_hash: string | null;
  status: string;
  output: string | null;
  triggered_by: string;
  duration_ms: number | null;
  created_at: string;
}

export default function Deploy() {
  const { id } = useParams<{ id: string }>();
  const [config, setConfig] = useState<DeployConfig | null>(null);
  const [logs, setLogs] = useState<DeployLog[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [deploying, setDeploying] = useState(false);
  const [generatingKey, setGeneratingKey] = useState(false);
  const [message, setMessage] = useState("");
  const [expandedLog, setExpandedLog] = useState<string | null>(null);
  const [showSecret, setShowSecret] = useState(false);

  // Form fields
  const [repoUrl, setRepoUrl] = useState("");
  const [branch, setBranch] = useState("main");
  const [deployScript, setDeployScript] = useState("");
  const [autoDeploy, setAutoDeploy] = useState(false);

  const load = async () => {
    try {
      const [cfg, logData] = await Promise.all([
        api.get<DeployConfig | null>(`/sites/${id}/deploy`),
        api.get<DeployLog[]>(`/sites/${id}/deploy/logs?limit=20`),
      ]);
      setConfig(cfg);
      setLogs(logData);
      if (cfg) {
        setRepoUrl(cfg.repo_url);
        setBranch(cfg.branch);
        setDeployScript(cfg.deploy_script);
        setAutoDeploy(cfg.auto_deploy);
      }
    } catch {
      // No config yet — that's fine
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, [id]);

  const handleSave = async () => {
    setSaving(true);
    setMessage("");
    try {
      const cfg = await api.put<DeployConfig>(`/sites/${id}/deploy`, {
        repo_url: repoUrl,
        branch: branch || "main",
        deploy_script: deployScript || undefined,
        auto_deploy: autoDeploy,
      });
      setConfig(cfg);
      setMessage("Deploy configuration saved.");
    } catch (err) {
      setMessage(err instanceof Error ? err.message : "Save failed");
    } finally {
      setSaving(false);
    }
  };

  const handleDeploy = async () => {
    setDeploying(true);
    setMessage("");
    try {
      const log = await api.post<DeployLog>(`/sites/${id}/deploy/trigger`);
      setMessage(log.status === "success" ? "Deployment completed successfully!" : "Deployment failed. Check logs below.");
      await load();
    } catch (err) {
      setMessage(err instanceof Error ? err.message : "Deploy failed");
    } finally {
      setDeploying(false);
    }
  };

  const handleKeygen = async () => {
    setGeneratingKey(true);
    setMessage("");
    try {
      const result = await api.post<{ public_key: string }>(`/sites/${id}/deploy/keygen`);
      setMessage("Deploy key generated. Add it to your repository's deploy keys.");
      setConfig((prev) => prev ? { ...prev, deploy_key_public: result.public_key } : prev);
    } catch (err) {
      setMessage(err instanceof Error ? err.message : "Key generation failed");
    } finally {
      setGeneratingKey(false);
    }
  };

  const handleRemove = async () => {
    if (!confirm("Remove deploy configuration? This won't delete your site files.")) return;
    try {
      await api.delete(`/sites/${id}/deploy`);
      setConfig(null);
      setRepoUrl("");
      setBranch("main");
      setDeployScript("");
      setAutoDeploy(false);
      setMessage("Deploy configuration removed.");
    } catch (err) {
      setMessage(err instanceof Error ? err.message : "Remove failed");
    }
  };

  const copyText = (text: string) => {
    navigator.clipboard.writeText(text);
    setMessage("Copied to clipboard.");
    setTimeout(() => setMessage(""), 2000);
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
    <div className="p-6 lg:p-8 space-y-6">
      {/* Breadcrumb */}
      <div>
        <Link to="/sites" className="text-sm text-dark-200 hover:text-gray-700">Sites</Link>
        <span className="text-sm text-dark-300 mx-2">/</span>
        <Link to={`/sites/${id}`} className="text-sm text-dark-200 hover:text-gray-700">Site</Link>
        <span className="text-sm text-dark-300 mx-2">/</span>
        <span className="text-sm text-dark-50 font-medium">Git Deploy</span>
      </div>

      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold text-dark-50">Git Deploy</h1>
        {config && (
          <div className="flex items-center gap-3">
            <button
              onClick={handleDeploy}
              disabled={deploying}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
            >
              {deploying ? "Deploying..." : "Deploy Now"}
            </button>
            <button
              onClick={handleRemove}
              className="px-4 py-2 bg-red-500/10 text-red-400 rounded-lg text-sm font-medium hover:bg-red-100 transition-colors"
            >
              Remove
            </button>
          </div>
        )}
      </div>

      {/* Message */}
      {message && (
        <div className={`px-4 py-3 rounded-lg text-sm border ${
          message.includes("success") || message.includes("saved") || message.includes("Copied") || message.includes("generated") || message.includes("removed")
            ? "bg-emerald-50 text-emerald-400 border-emerald-200"
            : "bg-red-500/10 text-red-400 border-red-500/20"
        }`}>
          {message}
        </div>
      )}

      {/* Configuration */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <div className="px-5 py-4 border-b border-dark-600">
          <h2 className="text-lg font-semibold text-dark-50">Repository</h2>
        </div>
        <div className="p-5 space-y-4">
          <div>
            <label className="block text-sm font-medium text-dark-100 mb-1">Repository URL</label>
            <input
              type="text"
              value={repoUrl}
              onChange={(e) => setRepoUrl(e.target.value)}
              placeholder="https://github.com/user/repo.git or git@github.com:user/repo.git"
              className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
            />
          </div>
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium text-dark-100 mb-1">Branch</label>
              <input
                type="text"
                value={branch}
                onChange={(e) => setBranch(e.target.value)}
                placeholder="main"
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
              />
            </div>
            <div className="flex items-end">
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={autoDeploy}
                  onChange={(e) => setAutoDeploy(e.target.checked)}
                  className="w-4 h-4 text-rust-500 border-dark-500 rounded focus:ring-rust-500"
                />
                <span className="text-sm text-dark-100">Auto-deploy on webhook push</span>
              </label>
            </div>
          </div>
          <div>
            <label className="block text-sm font-medium text-dark-100 mb-1">Deploy Script (optional)</label>
            <textarea
              value={deployScript}
              onChange={(e) => setDeployScript(e.target.value)}
              placeholder={"# Runs after git pull. Example:\nnpm install\nnpm run build"}
              rows={4}
              className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
            />
          </div>
          <div className="flex items-center gap-3">
            <button
              onClick={handleSave}
              disabled={saving || !repoUrl.trim()}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
            >
              {saving ? "Saving..." : config ? "Update Config" : "Save Config"}
            </button>
          </div>
        </div>
      </div>

      {/* Deploy Key */}
      {config && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-4 border-b border-dark-600">
            <h2 className="text-lg font-semibold text-dark-50">Deploy Key</h2>
            <p className="text-xs text-dark-200 mt-1">For private repos using SSH. Add this key as a deploy key in your Git provider.</p>
          </div>
          <div className="p-5">
            {config.deploy_key_public ? (
              <div className="space-y-3">
                <div className="relative">
                  <pre className="bg-dark-900 border border-dark-500 rounded-lg p-3 text-xs font-mono text-dark-100 overflow-x-auto whitespace-pre-wrap break-all">
                    {config.deploy_key_public}
                  </pre>
                  <button
                    onClick={() => copyText(config.deploy_key_public!)}
                    className="absolute top-2 right-2 px-2 py-1 bg-dark-800 border border-dark-500 rounded text-xs text-dark-200 hover:bg-dark-800"
                  >
                    Copy
                  </button>
                </div>
                <button
                  onClick={handleKeygen}
                  disabled={generatingKey}
                  className="text-sm text-dark-200 hover:text-gray-800"
                >
                  {generatingKey ? "Regenerating..." : "Regenerate key"}
                </button>
              </div>
            ) : (
              <button
                onClick={handleKeygen}
                disabled={generatingKey}
                className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-gray-200 disabled:opacity-50 transition-colors"
              >
                {generatingKey ? "Generating..." : "Generate Deploy Key"}
              </button>
            )}
          </div>
        </div>
      )}

      {/* Webhook URL */}
      {config && config.auto_deploy && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-4 border-b border-dark-600">
            <h2 className="text-lg font-semibold text-dark-50">Webhook URL</h2>
            <p className="text-xs text-dark-200 mt-1">Add this URL to your Git provider's webhook settings (push events).</p>
          </div>
          <div className="p-5">
            <div className="relative">
              <pre className="bg-dark-900 border border-dark-500 rounded-lg p-3 text-xs font-mono text-dark-100 overflow-x-auto pr-24">
                {showSecret
                  ? `${window.location.origin}/api/webhooks/deploy/${config.site_id}/${config.webhook_secret}`
                  : `${window.location.origin}/api/webhooks/deploy/${config.site_id}/${"●".repeat(8)}`}
              </pre>
              <div className="absolute top-2 right-2 flex items-center gap-1">
                <button
                  onClick={() => setShowSecret(!showSecret)}
                  className="px-2 py-1 bg-dark-800 border border-dark-500 rounded text-xs text-dark-200 hover:text-dark-50 transition-colors"
                  title={showSecret ? "Hide secret" : "Show secret"}
                >
                  {showSecret ? (
                    <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M3.98 8.223A10.477 10.477 0 001.934 12C3.226 16.338 7.244 19.5 12 19.5c.993 0 1.953-.138 2.863-.395M6.228 6.228A10.45 10.45 0 0112 4.5c4.756 0 8.773 3.162 10.065 7.498a10.523 10.523 0 01-4.293 5.774M6.228 6.228L3 3m3.228 3.228l3.65 3.65m7.894 7.894L21 21m-3.228-3.228l-3.65-3.65m0 0a3 3 0 10-4.243-4.243m4.242 4.242L9.88 9.88" />
                    </svg>
                  ) : (
                    <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M2.036 12.322a1.012 1.012 0 010-.639C3.423 7.51 7.36 4.5 12 4.5c4.638 0 8.573 3.007 9.963 7.178.07.207.07.431 0 .639C20.577 16.49 16.64 19.5 12 19.5c-4.638 0-8.573-3.007-9.963-7.178z" />
                      <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                    </svg>
                  )}
                </button>
                <button
                  onClick={() => copyText(`${window.location.origin}/api/webhooks/deploy/${config.site_id}/${config.webhook_secret}`)}
                  className="px-2 py-1 bg-dark-800 border border-dark-500 rounded text-xs text-dark-200 hover:text-dark-50 transition-colors"
                >
                  Copy
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Status */}
      {config && config.last_deploy && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-4 flex items-center justify-between">
            <div>
              <span className="text-sm font-medium text-dark-100">Last deploy: </span>
              <span className="text-sm text-dark-50">{formatDate(config.last_deploy)}</span>
            </div>
            <span className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium ${
              config.last_status === "success"
                ? "bg-emerald-500/15 text-emerald-400"
                : "bg-red-500/15 text-red-400"
            }`}>
              {config.last_status}
            </span>
          </div>
        </div>
      )}

      {/* Deploy Logs */}
      {logs.length > 0 && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-4 border-b border-dark-600">
            <h2 className="text-lg font-semibold text-dark-50">Deploy History</h2>
          </div>
          <div className="divide-y divide-dark-600">
            {logs.map((log) => (
              <div key={log.id}>
                <button
                  onClick={() => setExpandedLog(expandedLog === log.id ? null : log.id)}
                  className="w-full px-5 py-3 flex items-center justify-between hover:bg-dark-800 transition-colors text-left"
                >
                  <div className="flex items-center gap-3">
                    <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${
                      log.status === "success"
                        ? "bg-emerald-500/15 text-emerald-400"
                        : "bg-red-500/15 text-red-400"
                    }`}>
                      {log.status}
                    </span>
                    <span className="text-sm text-dark-50">{formatDate(log.created_at)}</span>
                    {log.commit_hash && (
                      <code className="text-xs text-dark-200 bg-dark-700 px-1.5 py-0.5 rounded">
                        {log.commit_hash.substring(0, 8)}
                      </code>
                    )}
                  </div>
                  <div className="flex items-center gap-3">
                    {log.duration_ms != null && (
                      <span className="text-xs text-dark-200 font-mono">{(log.duration_ms / 1000).toFixed(1)}s</span>
                    )}
                    <span className="text-xs text-dark-300 bg-dark-700 px-1.5 py-0.5 rounded">{log.triggered_by}</span>
                    <svg className={`w-4 h-4 text-dark-300 transition-transform ${expandedLog === log.id ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="m19.5 8.25-7.5 7.5-7.5-7.5" />
                    </svg>
                  </div>
                </button>
                {expandedLog === log.id && log.output && (
                  <div className="px-5 pb-4">
                    <pre className="bg-gray-900 text-gray-100 rounded-lg p-4 text-xs font-mono overflow-x-auto max-h-64 overflow-y-auto whitespace-pre-wrap">
                      {log.output}
                    </pre>
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
