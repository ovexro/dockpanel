import { useState, useEffect, type ReactNode } from "react";
import { api } from "../api";

interface EnvVar {
  name: string;
  label: string;
  default: string;
  required: boolean;
  secret: boolean;
}

interface AppTemplate {
  id: string;
  name: string;
  description: string;
  category: string;
  image: string;
  default_port: number;
  container_port: string;
  env_vars: EnvVar[];
}

interface DeployedApp {
  container_id: string;
  name: string;
  template: string;
  status: string;
  port: number | null;
}

interface ComposeService {
  name: string;
  image: string;
  ports: { host: number; container: number; protocol: string }[];
  environment: Record<string, string>;
  volumes: string[];
  restart: string;
}

interface ComposeDeployResult {
  services: { name: string; container_id: string; status: string; error: string | null }[];
}

const categoryColors: Record<string, string> = {
  CMS: "bg-blue-500/15",
  Database: "bg-emerald-500/15",
  Monitoring: "bg-purple-500/15",
  DevOps: "bg-amber-500/15",
  Automation: "bg-pink-500/15",
  Tools: "bg-slate-500/15",
  Development: "bg-cyan-500/15",
};

// SVG icons for each app template
const appIcons: Record<string, ReactNode> = {
  wordpress: (
    <svg className="w-6 h-6" viewBox="0 0 24 24" fill="#3b82f6">
      <path d="M12 2C6.486 2 2 6.486 2 12s4.486 10 10 10 10-4.486 10-10S17.514 2 12 2ZM3.009 12c0-1.36.306-2.65.856-3.805l4.71 12.907A8.99 8.99 0 0 1 3.01 12Zm8.991 9a8.95 8.95 0 0 1-2.737-.426l2.907-8.447 2.98 8.164c.02.049.043.095.068.14A8.95 8.95 0 0 1 12 21Zm1.237-13.23c.584-.031 1.11-.093 1.11-.093.523-.062.461-.83-.063-.8 0 0-1.574.124-2.59.124-.953 0-2.558-.124-2.558-.124-.524-.03-.586.769-.063.8 0 0 .494.063 1.016.093l1.51 4.135-2.122 6.362L6.18 8.77c.584-.031 1.11-.093 1.11-.093.523-.062.461-.83-.063-.8 0 0-1.573.124-2.59.124-.182 0-.398-.005-.623-.013A8.993 8.993 0 0 1 12 3.009c2.236 0 4.274.815 5.846 2.164-.037-.002-.073-.007-.111-.007-.953 0-1.628.83-1.628 1.722 0 .8.461 1.476.952 2.276.37.645.8 1.476.8 2.673 0 .83-.318 1.792-.738 3.133l-.968 3.233Zm4.906 1.908a8.967 8.967 0 0 1-1.16 4.39l-3.19-9.238c.627-1.138.83-2.05.83-2.86 0-.293-.02-.566-.054-.82A8.96 8.96 0 0 1 20.991 12c0 1.199-.235 2.343-.66 3.391l-.197.287Z" />
    </svg>
  ),
  ghost: (
    <svg className="w-6 h-6" viewBox="0 0 24 24" fill="#8b5cf6">
      <path d="M12 2C7.58 2 4 5.58 4 10v8.5c0 .83.67 1.5 1.5 1.5s1.5-.67 1.5-1.5-.67-1.5-1.5-1.5v-1c1.38 0 2.5 1.12 2.5 2.5S6.88 21 5.5 21 3 19.88 3 18.5V10c0-4.97 4.03-9 9-9s9 4.03 9 9v8.5c0 1.38-1.12 2.5-2.5 2.5S16 19.88 16 18.5s1.12-2.5 2.5-2.5v1c-.83 0-1.5.67-1.5 1.5s.67 1.5 1.5 1.5 1.5-.67 1.5-1.5V10c0-4.42-3.58-8-8-8Zm-2.5 8a1.5 1.5 0 1 1 0-3 1.5 1.5 0 0 1 0 3Zm5 0a1.5 1.5 0 1 1 0-3 1.5 1.5 0 0 1 0 3Zm-5.5 3.5c0 1.38 1.12 2.5 2.5 2.5h1c1.38 0 2.5-1.12 2.5-2.5V13H9v.5Z" />
    </svg>
  ),
  redis: (
    <svg className="w-6 h-6" viewBox="0 0 24 24" fill="#ef4444">
      <path d="M21.7 9.3c-.1-.1-.3-.2-.5-.2l-8.5-3.4c-.5-.2-1-.2-1.4 0L2.8 9.1c-.2.1-.4.2-.5.3-.3.3-.3.7 0 1l.5.3 8.5 3.4c.2.1.5.1.7.1s.5 0 .7-.1l8.5-3.4.5-.3c.2-.3.2-.8 0-1.1Zm-9.7 5.5-7.2-2.9-.5-.2c-.3-.3-.3-.7 0-1l.5-.3L12 7.5l7.2 2.9.5.2c.3.3.3.7 0 1l-.5.3L12 14.8Zm9.7 1.5-.5-.2-8.5-3.4c-.4-.2-1-.2-1.4 0l-8.5 3.4-.5.2c-.3.3-.3.7 0 1l.5.3 8.5 3.4c.2.1.5.1.7.1s.5 0 .7-.1l8.5-3.4.5-.3c.2-.3.2-.7 0-1Z" />
    </svg>
  ),
  adminer: (
    <svg className="w-6 h-6" viewBox="0 0 24 24" fill="#10b981">
      <path d="M4 6h16v2H4V6Zm0 5h16v2H4v-2Zm0 5h16v2H4v-2Z" />
      <circle cx="7" cy="7" r="1.5" />
      <circle cx="7" cy="12" r="1.5" />
      <circle cx="7" cy="17" r="1.5" />
    </svg>
  ),
  "uptime-kuma": (
    <svg className="w-6 h-6" viewBox="0 0 24 24" fill="#a855f7">
      <path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35Z" />
    </svg>
  ),
  portainer: (
    <svg className="w-6 h-6" viewBox="0 0 24 24" fill="#06b6d4">
      <path d="M12 2L2 7v10l10 5 10-5V7L12 2Zm0 2.18L19.18 8 12 11.82 4.82 8 12 4.18ZM4 9.32l7 3.5v7.86l-7-3.5V9.32Zm9 11.36v-7.86l7-3.5v7.86l-7 3.5Z" />
    </svg>
  ),
  n8n: (
    <svg className="w-6 h-6" viewBox="0 0 24 24" fill="#f97316">
      <path d="M12 2a2 2 0 0 1 2 2c0 .74-.4 1.39-1 1.73V7h3a3 3 0 0 1 3 3v1.27c.6.34 1 .99 1 1.73a2 2 0 0 1-2 2 2 2 0 0 1-2-2c0-.74.4-1.39 1-1.73V10a1 1 0 0 0-1-1h-3v2.27c.6.34 1 .99 1 1.73a2 2 0 0 1-4 0c0-.74.4-1.39 1-1.73V9H8a1 1 0 0 0-1 1v1.27c.6.34 1 .99 1 1.73a2 2 0 0 1-4 0c0-.74.4-1.39 1-1.73V10a3 3 0 0 1 3-3h3V5.73c-.6-.34-1-.99-1-1.73a2 2 0 0 1 2-2Zm0 15a2 2 0 0 1 2 2 2 2 0 0 1-2 2 2 2 0 0 1-2-2 2 2 0 0 1 2-2Z" />
    </svg>
  ),
  gitea: (
    <svg className="w-6 h-6" viewBox="0 0 24 24" fill="#22c55e">
      <path d="M4.209 4.603c-.247 0-.525.02-.84.088-.333.07-1.28.283-2.054 1.027C.56 6.474.181 7.461.018 7.983a.327.327 0 0 0 .208.405c.08.025.164.016.239-.024l2.196-1.19a.391.391 0 0 1 .163-.037h.312c.12 0 .239.04.335.112l1.318 1.006a.28.28 0 0 0 .34.002l.95-.706a.37.37 0 0 1 .224-.075h.428c.097 0 .19.037.263.104l.96.926a.26.26 0 0 0 .363 0l.962-.926a.37.37 0 0 1 .263-.104h.428c.084 0 .166.027.233.075l.95.706a.28.28 0 0 0 .34-.002l1.319-1.006a.47.47 0 0 1 .335-.112h.312a.391.391 0 0 1 .163.037l2.196 1.19a.327.327 0 0 0 .447-.38c-.164-.523-.543-1.51-1.297-2.265-.774-.744-1.722-.957-2.054-1.027a4.146 4.146 0 0 0-.84-.088c-.217 0-.444.016-.654.056a2.849 2.849 0 0 0-.444.12l-.207.088-.176-.055a3.335 3.335 0 0 0-.853-.124 3.39 3.39 0 0 0-.855.124l-.176.055-.207-.088a2.849 2.849 0 0 0-.444-.12 4.146 4.146 0 0 0-.654-.056ZM12 9.5a7.5 7.5 0 1 0 0 15 7.5 7.5 0 0 0 0-15Zm0 2a5.5 5.5 0 1 1 0 11 5.5 5.5 0 0 1 0-11Zm-1.5 2.5v2h-2v1.5h2v2h1.5v-2h2V16h-2v-2h-1.5Z" />
    </svg>
  ),
};

const statusColors: Record<string, string> = {
  running: "bg-emerald-500/15 text-emerald-400",
  exited: "bg-red-500/15 text-red-400",
  created: "bg-amber-500/15 text-amber-400",
  paused: "bg-dark-700 text-dark-200",
};

export default function Apps() {
  const [templates, setTemplates] = useState<AppTemplate[]>([]);
  const [apps, setApps] = useState<DeployedApp[]>([]);
  const [loading, setLoading] = useState(true);
  const [deploying, setDeploying] = useState(false);
  const [selected, setSelected] = useState<AppTemplate | null>(null);
  const [appName, setAppName] = useState("");
  const [appPort, setAppPort] = useState(0);
  const [envValues, setEnvValues] = useState<Record<string, string>>({});
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [logsTarget, setLogsTarget] = useState<string | null>(null);
  const [logLines, setLogLines] = useState<string[]>([]);
  // Compose import state
  const [showCompose, setShowCompose] = useState(false);
  const [composeYaml, setComposeYaml] = useState("");
  const [composeParsed, setComposeParsed] = useState<ComposeService[] | null>(null);
  const [composeDeploying, setComposeDeploying] = useState(false);
  const [composeError, setComposeError] = useState("");

  useEffect(() => {
    Promise.all([
      api.get<AppTemplate[]>("/apps/templates").catch(() => []),
      api.get<DeployedApp[]>("/apps").catch(() => []),
    ]).then(([tmpl, deployed]) => {
      setTemplates(tmpl);
      setApps(deployed);
      setLoading(false);
    });
  }, []);

  const loadApps = () => {
    api.get<DeployedApp[]>("/apps").then(setApps).catch((e) => console.error("Failed to load apps:", e));
  };

  const openDeploy = (tmpl: AppTemplate) => {
    setSelected(tmpl);
    setAppName(tmpl.id);
    setAppPort(tmpl.default_port);
    const defaults: Record<string, string> = {};
    tmpl.env_vars.forEach((v) => {
      defaults[v.name] = v.default;
    });
    setEnvValues(defaults);
  };

  const handleDeploy = async () => {
    if (!selected) return;
    setDeploying(true);
    setMessage({ text: "", type: "" });
    try {
      await api.post("/apps/deploy", {
        template_id: selected.id,
        name: appName,
        port: appPort,
        env: envValues,
      });
      setSelected(null);
      setMessage({ text: `${selected.name} deployed successfully`, type: "success" });
      loadApps();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Deploy failed",
        type: "error",
      });
    } finally {
      setDeploying(false);
    }
  };

  const handleAction = async (containerId: string, action: "stop" | "start" | "restart") => {
    setActionLoading(`${containerId}-${action}`);
    setMessage({ text: "", type: "" });
    try {
      await api.post(`/apps/${containerId}/${action}`);
      setMessage({ text: `App ${action}ed successfully`, type: "success" });
      loadApps();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : `${action} failed`,
        type: "error",
      });
    } finally {
      setActionLoading(null);
    }
  };

  const handleLogs = async (containerId: string) => {
    if (logsTarget === containerId) {
      setLogsTarget(null);
      setLogLines([]);
      return;
    }
    setLogsTarget(containerId);
    setLogLines([]);
    try {
      const data = await api.get<{ logs: string }>(`/apps/${containerId}/logs`);
      setLogLines((data.logs || "").split("\n").filter(Boolean));
    } catch (e) {
      setLogLines([`Error: ${e instanceof Error ? e.message : "Failed to fetch logs"}`]);
    }
  };

  const handleRemove = async (containerId: string) => {
    try {
      await api.delete(`/apps/${containerId}`);
      setDeleteTarget(null);
      if (logsTarget === containerId) {
        setLogsTarget(null);
        setLogLines([]);
      }
      setMessage({ text: "App removed", type: "success" });
      loadApps();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Remove failed",
        type: "error",
      });
    }
  };

  // Compose import handlers
  const handleComposeParse = async () => {
    if (!composeYaml.trim()) return;
    setComposeError("");
    setComposeParsed(null);
    try {
      const services = await api.post<ComposeService[]>("/apps/compose/parse", { yaml: composeYaml });
      setComposeParsed(services);
    } catch (e) {
      setComposeError(e instanceof Error ? e.message : "Failed to parse YAML");
    }
  };

  const handleComposeDeploy = async () => {
    if (!composeYaml.trim()) return;
    setComposeDeploying(true);
    setComposeError("");
    try {
      const result = await api.post<ComposeDeployResult>("/apps/compose/deploy", { yaml: composeYaml });
      const failed = result.services.filter(s => s.status === "failed");
      if (failed.length > 0) {
        setComposeError(`${failed.length} service(s) failed: ${failed.map(s => `${s.name}: ${s.error}`).join(", ")}`);
      } else {
        setShowCompose(false);
        setComposeYaml("");
        setComposeParsed(null);
        setMessage({
          text: `Compose import: ${result.services.length} service(s) deployed`,
          type: "success",
        });
        loadApps();
      }
    } catch (e) {
      setComposeError(e instanceof Error ? e.message : "Deploy failed");
    } finally {
      setComposeDeploying(false);
    }
  };

  return (
    <div className="p-6 lg:p-8">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-bold text-dark-50">Docker Apps</h1>
          <p className="text-sm text-dark-200 mt-1">
            One-click deploy popular applications
          </p>
        </div>
        <button
          onClick={() => setShowCompose(true)}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 flex items-center gap-2"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 14.25v-2.625a3.375 3.375 0 00-3.375-3.375h-1.5A1.125 1.125 0 0113.5 7.125v-1.5a3.375 3.375 0 00-3.375-3.375H8.25m6.75 12l-3-3m0 0l-3 3m3-3v6m-1.5-15H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 00-9-9z" />
          </svg>
          Import Compose
        </button>
      </div>

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-emerald-50 text-emerald-400 border-emerald-200"
              : "bg-red-500/10 text-red-400 border-red-500/20"
          }`}
        >
          {message.text}
        </div>
      )}

      {/* Deployed Apps */}
      {apps.length > 0 && (
        <div className="mb-8">
          <h2 className="text-sm font-medium text-dark-200 uppercase mb-3">
            Running Apps
          </h2>
          <div className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden">
            <table className="w-full">
              <thead>
                <tr className="bg-dark-900 border-b border-dark-500">
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3">App</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 hidden sm:table-cell">Template</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 hidden sm:table-cell w-20">Port</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 w-24">Status</th>
                  <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase px-5 py-3">Actions</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-600">
                {apps.map((app) => (
                  <tr key={app.container_id} className="hover:bg-dark-800">
                    <td className="px-5 py-4 text-sm text-dark-50 font-medium">{app.name}</td>
                    <td className="px-5 py-4 text-sm text-dark-200 hidden sm:table-cell">{app.template}</td>
                    <td className="px-5 py-4 text-sm text-dark-200 font-mono hidden sm:table-cell">{app.port || "\u2014"}</td>
                    <td className="px-5 py-4">
                      <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${statusColors[app.status] || "bg-dark-700 text-dark-200"}`}>
                        {app.status}
                      </span>
                    </td>
                    <td className="px-5 py-4 text-right">
                      <div className="flex items-center justify-end gap-1 flex-wrap">
                        {app.status === "running" ? (
                          <button
                            onClick={() => handleAction(app.container_id, "stop")}
                            disabled={actionLoading === `${app.container_id}-stop`}
                            className="px-2 py-1 bg-amber-50 text-amber-400 rounded text-xs font-medium hover:bg-amber-100 disabled:opacity-50"
                          >
                            {actionLoading === `${app.container_id}-stop` ? "..." : "Stop"}
                          </button>
                        ) : (
                          <button
                            onClick={() => handleAction(app.container_id, "start")}
                            disabled={actionLoading === `${app.container_id}-start`}
                            className="px-2 py-1 bg-emerald-50 text-emerald-400 rounded text-xs font-medium hover:bg-emerald-100 disabled:opacity-50"
                          >
                            {actionLoading === `${app.container_id}-start` ? "..." : "Start"}
                          </button>
                        )}
                        <button
                          onClick={() => handleAction(app.container_id, "restart")}
                          disabled={!!actionLoading}
                          className="px-2 py-1 bg-blue-50 text-blue-400 rounded text-xs font-medium hover:bg-blue-100 disabled:opacity-50"
                        >
                          {actionLoading === `${app.container_id}-restart` ? "..." : "Restart"}
                        </button>
                        <button
                          onClick={() => handleLogs(app.container_id)}
                          className={`px-2 py-1 rounded text-xs font-medium ${
                            logsTarget === app.container_id
                              ? "bg-slate-200 text-slate-700"
                              : "bg-slate-50 text-slate-600 hover:bg-slate-100"
                          }`}
                        >
                          Logs
                        </button>
                        {deleteTarget === app.container_id ? (
                          <>
                            <button onClick={() => handleRemove(app.container_id)} className="px-2 py-1 bg-red-600 text-white rounded text-xs">Confirm</button>
                            <button onClick={() => setDeleteTarget(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs">Cancel</button>
                          </>
                        ) : (
                          <button onClick={() => setDeleteTarget(app.container_id)} className="px-2 py-1 bg-red-500/10 text-red-400 rounded text-xs font-medium hover:bg-red-100">Remove</button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* Logs panel */}
          {logsTarget && (
            <div className="mt-3 bg-[#1e1e2e] rounded-xl border border-gray-700 overflow-hidden">
              <div className="flex items-center justify-between px-4 py-2 border-b border-gray-700">
                <span className="text-xs text-dark-300 font-mono">
                  Logs: {apps.find(a => a.container_id === logsTarget)?.name}
                </span>
                <button
                  onClick={() => { setLogsTarget(null); setLogLines([]); }}
                  className="text-dark-200 hover:text-gray-300 text-xs"
                >
                  Close
                </button>
              </div>
              <div className="p-4 max-h-64 overflow-y-auto font-mono text-xs">
                {logLines.length === 0 ? (
                  <div className="text-dark-200">Loading...</div>
                ) : (
                  logLines.map((line, i) => (
                    <div key={i} className="py-0.5 text-gray-300 whitespace-pre-wrap break-all">
                      {line}
                    </div>
                  ))
                )}
              </div>
            </div>
          )}
        </div>
      )}

      {/* Template Gallery */}
      <h2 className="text-sm font-medium text-dark-200 uppercase mb-3">
        App Templates
      </h2>
      {loading ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
          {[...Array(8)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-xl border border-dark-500 p-5 animate-pulse">
              <div className="h-5 bg-dark-600 rounded w-24 mb-2" />
              <div className="h-4 bg-dark-600 rounded w-full" />
            </div>
          ))}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
          {templates.map((tmpl) => (
            <button
              key={tmpl.id}
              onClick={() => openDeploy(tmpl)}
              className="bg-dark-800 rounded-xl border border-dark-500 p-5 text-left hover:border-indigo-300 hover:shadow-sm transition-all group"
            >
              <div className="flex items-center gap-3 mb-3">
                <div className={`w-10 h-10 rounded-lg flex items-center justify-center ${categoryColors[tmpl.category] || "bg-dark-700"}`}>
                  {appIcons[tmpl.id] || <span className="text-sm font-bold text-dark-200">{tmpl.name[0]}</span>}
                </div>
                <div>
                  <h3 className="text-sm font-semibold text-dark-50 group-hover:text-indigo-600">
                    {tmpl.name}
                  </h3>
                  <span className="text-[10px] text-dark-300 uppercase">{tmpl.category}</span>
                </div>
              </div>
              <p className="text-xs text-dark-200 line-clamp-2">{tmpl.description}</p>
              <div className="flex items-center justify-between mt-3">
                <span className="text-[10px] text-dark-300 font-mono">{tmpl.image}</span>
                <span className="text-[10px] text-dark-300">:{tmpl.default_port}</span>
              </div>
            </button>
          ))}
        </div>
      )}

      {/* Deploy dialog */}
      {selected && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 p-4"
          role="dialog"
          aria-labelledby="deploy-dialog-title"
          onKeyDown={(e) => { if (e.key === "Escape") setSelected(null); }}
        >
          <div className="bg-dark-800 rounded-xl shadow-xl p-6 w-full max-w-[480px] max-h-[80vh] overflow-y-auto">
            <h3 id="deploy-dialog-title" className="text-lg font-semibold text-dark-50 mb-1">
              Deploy {selected.name}
            </h3>
            <p className="text-sm text-dark-200 mb-4">{selected.description}</p>

            <div className="space-y-4">
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Container Name</label>
                <input
                  type="text"
                  value={appName}
                  onChange={(e) => setAppName(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Host Port</label>
                <input
                  type="number"
                  value={appPort}
                  onChange={(e) => setAppPort(Number(e.target.value))}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
                />
              </div>

              {selected.env_vars.length > 0 && (
                <div>
                  <p className="text-sm font-medium text-dark-100 mb-2">Environment Variables</p>
                  <div className="space-y-2">
                    {selected.env_vars.map((v) => (
                      <div key={v.name}>
                        <label className="block text-xs text-dark-200 mb-0.5">
                          {v.label} {v.required && <span className="text-red-500">*</span>}
                        </label>
                        <input
                          type={v.secret ? "password" : "text"}
                          value={envValues[v.name] || ""}
                          onChange={(e) =>
                            setEnvValues({ ...envValues, [v.name]: e.target.value })
                          }
                          placeholder={v.default || v.name}
                          className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
                        />
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>

            <div className="flex justify-end gap-2 mt-6">
              <button
                onClick={() => setSelected(null)}
                className="px-4 py-2 text-sm text-dark-200 hover:text-gray-800"
              >
                Cancel
              </button>
              <button
                onClick={handleDeploy}
                disabled={deploying || !appName}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {deploying ? "Deploying..." : "Deploy"}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Compose Import Dialog */}
      {showCompose && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 p-4"
          role="dialog"
          aria-labelledby="compose-dialog-title"
          onKeyDown={(e) => { if (e.key === "Escape") { setShowCompose(false); setComposeParsed(null); setComposeError(""); }}}
        >
          <div className="bg-dark-800 rounded-xl shadow-xl p-6 w-full max-w-2xl max-h-[85vh] overflow-y-auto">
            <h3 id="compose-dialog-title" className="text-lg font-semibold text-dark-50 mb-1">
              Import Docker Compose
            </h3>
            <p className="text-sm text-dark-200 mb-4">
              Paste your docker-compose.yml to parse and deploy services. Only services with an <code className="text-xs bg-dark-700 px-1 rounded">image:</code> field are supported (no build context).
            </p>

            {composeError && (
              <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-red-500/10 text-red-400 border-red-500/20">
                {composeError}
              </div>
            )}

            <div className="mb-4">
              <label htmlFor="compose-yaml" className="block text-sm font-medium text-dark-100 mb-1">docker-compose.yml</label>
              <textarea
                id="compose-yaml"
                value={composeYaml}
                onChange={(e) => { setComposeYaml(e.target.value); setComposeParsed(null); }}
                rows={12}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
                placeholder={`version: "3.8"\nservices:\n  web:\n    image: nginx:latest\n    ports:\n      - "8080:80"\n  redis:\n    image: redis:7-alpine\n    ports:\n      - "6379:6379"`}
                spellCheck={false}
              />
            </div>

            {!composeParsed && (
              <div className="flex justify-end gap-2">
                <button
                  onClick={() => { setShowCompose(false); setComposeYaml(""); setComposeParsed(null); setComposeError(""); }}
                  className="px-4 py-2 text-sm text-dark-200"
                >
                  Cancel
                </button>
                <button
                  onClick={handleComposeParse}
                  disabled={!composeYaml.trim()}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                >
                  Parse & Preview
                </button>
              </div>
            )}

            {/* Parsed services preview */}
            {composeParsed && (
              <>
                <div className="mb-4">
                  <h4 className="text-sm font-medium text-dark-50 mb-2">
                    {composeParsed.length} service{composeParsed.length !== 1 ? "s" : ""} found:
                  </h4>
                  <div className="space-y-2">
                    {composeParsed.map((svc) => (
                      <div key={svc.name} className="bg-dark-900 rounded-lg p-3 border border-dark-500">
                        <div className="flex items-center justify-between">
                          <div className="flex items-center gap-2">
                            <span className="text-sm font-semibold text-dark-50">{svc.name}</span>
                            <span className="text-xs text-dark-300 font-mono">{svc.image}</span>
                          </div>
                          {svc.restart !== "no" && (
                            <span className="text-[10px] text-dark-300">restart: {svc.restart}</span>
                          )}
                        </div>
                        {svc.ports.length > 0 && (
                          <div className="mt-1.5 flex gap-2 flex-wrap">
                            {svc.ports.map((p, i) => (
                              <span key={i} className="px-1.5 py-0.5 bg-blue-50 text-blue-600 rounded text-[10px] font-mono">
                                {p.host}:{p.container}/{p.protocol}
                              </span>
                            ))}
                          </div>
                        )}
                        {Object.keys(svc.environment).length > 0 && (
                          <div className="mt-1.5 flex gap-2 flex-wrap">
                            {Object.entries(svc.environment).slice(0, 5).map(([k, v]) => (
                              <span key={k} className="px-1.5 py-0.5 bg-purple-50 text-purple-600 rounded text-[10px] font-mono">
                                {k}={v.length > 20 ? v.slice(0, 20) + "..." : v}
                              </span>
                            ))}
                            {Object.keys(svc.environment).length > 5 && (
                              <span className="text-[10px] text-dark-300">+{Object.keys(svc.environment).length - 5} more</span>
                            )}
                          </div>
                        )}
                        {svc.volumes.length > 0 && (
                          <div className="mt-1.5 flex gap-2 flex-wrap">
                            {svc.volumes.map((v, i) => (
                              <span key={i} className="px-1.5 py-0.5 bg-amber-50 text-amber-600 rounded text-[10px] font-mono">
                                {v}
                              </span>
                            ))}
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                </div>

                <div className="flex justify-end gap-2">
                  <button
                    onClick={() => { setComposeParsed(null); setComposeError(""); }}
                    className="px-4 py-2 text-sm text-dark-200"
                  >
                    Edit YAML
                  </button>
                  <button
                    onClick={handleComposeDeploy}
                    disabled={composeDeploying}
                    className="px-4 py-2 bg-emerald-600 text-white rounded-lg text-sm font-medium hover:bg-emerald-700 disabled:opacity-50 flex items-center gap-2"
                  >
                    {composeDeploying && (
                      <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
                        <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                        <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                      </svg>
                    )}
                    {composeDeploying ? "Deploying..." : `Deploy ${composeParsed.length} Service${composeParsed.length !== 1 ? "s" : ""}`}
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
