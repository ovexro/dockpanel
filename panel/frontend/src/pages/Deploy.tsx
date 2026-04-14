import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { api } from "../api";
import { formatDate } from "../utils/format";
import ProvisionLog from "../components/ProvisionLog";

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
  atomic_deploy: boolean;
  keep_releases: number;
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

interface ReleaseInfo {
  id: string;
  active: boolean;
  commit_hash: string | null;
  created_at: string;
}

export default function Deploy() {
  const { id } = useParams<{ id: string }>();
  const [config, setConfig] = useState<DeployConfig | null>(null);
  const [logs, setLogs] = useState<DeployLog[]>([]);
  const [releases, setReleases] = useState<ReleaseInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [deploying, setDeploying] = useState(false);
  const [deployId, setDeployId] = useState<string | null>(null);
  const [generatingKey, setGeneratingKey] = useState(false);
  const [rollingBack, setRollingBack] = useState<string | null>(null);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [expandedLog, setExpandedLog] = useState<string | null>(null);
  const [showSecret, setShowSecret] = useState(false);
  const [pendingConfirm, setPendingConfirm] = useState<{ type: string; id: string; label: string } | null>(null);

  // Form fields
  const [repoUrl, setRepoUrl] = useState("");
  const [branch, setBranch] = useState("main");
  const [deployScript, setDeployScript] = useState("");
  const [autoDeploy, setAutoDeploy] = useState(false);
  const [atomicDeploy, setAtomicDeploy] = useState(false);
  const [keepReleases, setKeepReleases] = useState(5);

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
        setAtomicDeploy(cfg.atomic_deploy);
        setKeepReleases(cfg.keep_releases);
        // Load releases if atomic deploy is enabled
        if (cfg.atomic_deploy) {
          try {
            const rel = await api.get<ReleaseInfo[]>(`/sites/${id}/deploy/releases`);
            setReleases(rel);
          } catch { /* no releases yet */ }
        }
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
    setMessage({ text: "", type: "" });
    try {
      const cfg = await api.put<DeployConfig>(`/sites/${id}/deploy`, {
        repo_url: repoUrl,
        branch: branch || "main",
        deploy_script: deployScript || undefined,
        auto_deploy: autoDeploy,
        atomic_deploy: atomicDeploy,
        keep_releases: keepReleases,
      });
      setConfig(cfg);
      setMessage({ text: "Deploy configuration saved.", type: "success" });
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Save failed", type: "error" });
    } finally {
      setSaving(false);
    }
  };

  const handleDeploy = async () => {
    setDeploying(true);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.post<{ deploy_id?: string }>(`/sites/${id}/deploy/trigger`);
      if (result.deploy_id) {
        setDeployId(result.deploy_id);
      } else {
        setMessage({ text: "Deployment completed.", type: "success" });
        setDeploying(false);
        await load();
      }
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Deploy failed", type: "error" });
      setDeploying(false);
    }
  };

  const handleRollback = (releaseId: string) => {
    setPendingConfirm({ type: "rollback", id: releaseId, label: `Rollback to release ${releaseId}? This will instantly activate that release.` });
  };

  const executeRollback = async (releaseId: string) => {
    setRollingBack(releaseId);
    setMessage({ text: "", type: "" });
    try {
      await api.post(`/sites/${id}/deploy/rollback/${releaseId}`);
      setMessage({ text: `Rolled back to release ${releaseId}.`, type: "success" });
      await load();
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Rollback failed", type: "error" });
    } finally {
      setRollingBack(null);
    }
  };

  const handleKeygen = async () => {
    setGeneratingKey(true);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.post<{ public_key: string }>(`/sites/${id}/deploy/keygen`);
      setMessage({ text: "Deploy key generated. Add it to your repository's deploy keys.", type: "success" });
      setConfig((prev) => prev ? { ...prev, deploy_key_public: result.public_key } : prev);
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Key generation failed", type: "error" });
    } finally {
      setGeneratingKey(false);
    }
  };

  const handleRemove = () => {
    setPendingConfirm({ type: "remove", id: "", label: "Remove deploy configuration? This won't delete your site files." });
  };

  const executeRemove = async () => {
    try {
      await api.delete(`/sites/${id}/deploy`);
      setConfig(null);
      setRepoUrl("");
      setBranch("main");
      setDeployScript("");
      setAutoDeploy(false);
      setAtomicDeploy(false);
      setKeepReleases(5);
      setReleases([]);
      setMessage({ text: "Deploy configuration removed.", type: "success" });
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Remove failed", type: "error" });
    }
  };

  const executeConfirm = async () => {
    if (!pendingConfirm) return;
    const { type, id: confirmId } = pendingConfirm;
    setPendingConfirm(null);
    if (type === "rollback") await executeRollback(confirmId);
    else if (type === "remove") await executeRemove();
  };

  const copyText = (text: string) => {
    navigator.clipboard.writeText(text);
    setMessage({ text: "Copied to clipboard.", type: "success" });
    setTimeout(() => setMessage({ text: "", type: "" }), 2000);
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
        <Link to="/sites" className="text-sm text-dark-200 hover:text-dark-100">Sites</Link>
        <span className="text-sm text-dark-300 mx-2">/</span>
        <Link to={`/sites/${id}`} className="text-sm text-dark-200 hover:text-dark-100">Site</Link>
        <span className="text-sm text-dark-300 mx-2">/</span>
        <span className="text-sm text-dark-50 font-medium">Git Deploy</span>
      </div>

      <div className="flex items-center justify-between">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Git Deploy</h1>
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
              className="px-4 py-2 bg-danger-500/10 text-danger-400 rounded-lg text-sm font-medium hover:bg-danger-500/20 transition-colors"
            >
              Remove
            </button>
          </div>
        )}
      </div>

      {/* Message */}
      {message.text && (
        <div className={`px-4 py-3 rounded-lg text-sm border ${
          message.type === "success"
            ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
            : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`}>
          {message.text}
        </div>
      )}

      {/* Inline confirmation */}
      {pendingConfirm && (
        <div className="border border-danger-500/30 bg-danger-500/5 rounded-lg px-4 py-3 flex items-center justify-between">
          <span className="text-xs text-danger-400 font-mono">{pendingConfirm.label}</span>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            <button onClick={executeConfirm} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">Confirm</button>
            <button onClick={() => setPendingConfirm(null)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      {/* Deploy provisioning log */}
      {deployId && (
        <ProvisionLog
          sseUrl={`/api/services/install/${deployId}/log`}
          onComplete={() => {
            setDeployId(null);
            setDeploying(false);
            load();
          }}
        />
      )}

      {/* Configuration */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <div className="px-5 py-4 border-b border-dark-600">
          <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Repository</h2>
        </div>
        <div className="p-5 space-y-4">
          <div>
            <label className="block text-sm font-medium text-dark-100 mb-1">Repository URL</label>
            <input
              type="text"
              value={repoUrl}
              onChange={(e) => setRepoUrl(e.target.value)}
              placeholder="https://github.com/user/repo.git or git@github.com:user/repo.git"
              className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
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
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
              />
            </div>
            <div className="flex items-end">
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={autoDeploy}
                  onChange={(e) => setAutoDeploy(e.target.checked)}
                  className="w-4 h-4 text-rust-500 border-dark-500 rounded focus:ring-accent-500"
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
              className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
            />
          </div>

          {/* Zero-Downtime Deploy */}
          <div className="border border-dark-500 rounded-lg p-4 space-y-3">
            <div className="flex items-center justify-between">
              <div>
                <h3 className="text-sm font-medium text-dark-50">Zero-Downtime Deploy</h3>
                <p className="text-xs text-dark-300 mt-0.5">
                  Capistrano-style atomic symlink deploys. Each deploy creates an immutable release. Rollback is instant.
                </p>
              </div>
              <label className="relative inline-flex items-center cursor-pointer">
                <input
                  type="checkbox"
                  checked={atomicDeploy}
                  onChange={(e) => setAtomicDeploy(e.target.checked)}
                  className="sr-only peer"
                />
                <div className="w-11 h-6 bg-dark-600 peer-focus:ring-2 peer-focus:ring-accent-500 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-rust-500" />
              </label>
            </div>
            {atomicDeploy && (
              <div className="flex items-center gap-4">
                <div>
                  <label className="block text-xs font-medium text-dark-200 mb-1">Keep Releases</label>
                  <input
                    type="number"
                    value={keepReleases}
                    onChange={(e) => setKeepReleases(Math.max(2, Math.min(20, parseInt(e.target.value) || 5)))}
                    min={2}
                    max={20}
                    className="w-20 px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  />
                </div>
                <p className="text-xs text-dark-300 mt-4">
                  Old releases are automatically cleaned up. Shared dirs (uploads, .env) persist across releases.
                </p>
              </div>
            )}
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

      {/* Releases (atomic deploy only) */}
      {config?.atomic_deploy && releases.length > 0 && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-4 border-b border-dark-600">
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Releases</h2>
            <p className="text-xs text-dark-200 mt-1">{releases.length} release{releases.length !== 1 ? "s" : ""} on disk. Active release serves traffic.</p>
          </div>
          <div className="divide-y divide-dark-600">
            {releases.map((rel) => (
              <div key={rel.id} className="px-5 py-3 flex items-center justify-between">
                <div className="flex items-center gap-3">
                  {rel.active ? (
                    <span className="inline-flex px-2 py-0.5 rounded-full text-xs font-medium bg-rust-500/15 text-rust-400">
                      active
                    </span>
                  ) : (
                    <span className="inline-flex px-2 py-0.5 rounded-full text-xs font-medium bg-dark-700 text-dark-300">
                      inactive
                    </span>
                  )}
                  <code className="text-sm text-dark-50 font-mono">{rel.id}</code>
                  {rel.commit_hash && (
                    <code className="text-xs text-dark-200 bg-dark-700 px-1.5 py-0.5 rounded">
                      {rel.commit_hash}
                    </code>
                  )}
                </div>
                <div className="flex items-center gap-3">
                  <span className="text-xs text-dark-300">{rel.created_at ? formatDate(rel.created_at) : ""}</span>
                  {!rel.active && (
                    <button
                      onClick={() => handleRollback(rel.id)}
                      disabled={rollingBack === rel.id}
                      className="px-3 py-1 bg-dark-700 text-dark-100 rounded text-xs font-medium hover:bg-dark-600 disabled:opacity-50 transition-colors"
                    >
                      {rollingBack === rel.id ? "Rolling back..." : "Rollback"}
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Deploy Key */}
      {config && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-4 border-b border-dark-600">
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Deploy Key</h2>
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
                  className="text-sm text-dark-200 hover:text-dark-50"
                >
                  {generatingKey ? "Regenerating..." : "Regenerate key"}
                </button>
              </div>
            ) : (
              <button
                onClick={handleKeygen}
                disabled={generatingKey}
                className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600 disabled:opacity-50 transition-colors"
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
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Webhook URL</h2>
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
                ? "bg-rust-500/15 text-rust-400"
                : "bg-danger-500/15 text-danger-400"
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
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Deploy History</h2>
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
                        ? "bg-rust-500/15 text-rust-400"
                        : "bg-danger-500/15 text-danger-400"
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
                    <pre className="bg-dark-900 text-dark-100 rounded-lg p-4 text-xs font-mono overflow-x-auto max-h-64 overflow-y-auto whitespace-pre-wrap">
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
