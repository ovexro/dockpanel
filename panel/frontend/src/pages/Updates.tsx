import { useState, useEffect, useRef } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";

interface PackageUpdate {
  name: string;
  current_version: string;
  new_version: string;
  repo: string;
  security: boolean;
}

interface ApplyResult {
  success: boolean;
  updated: number;
  output: string;
  reboot_required: boolean;
}

function colorLine(line: string): string {
  if (/^> .*completed successfully/.test(line)) return "text-rust-400 font-semibold";
  if (/^> .*completed with errors/.test(line)) return "text-danger-400 font-semibold";
  if (/^> /.test(line)) return "text-rust-400";
  if (/^\$ /.test(line)) return "text-dark-400";
  if (/^Get:\d+/.test(line)) return "text-accent-400";
  if (/^Fetched /.test(line)) return "text-accent-400 font-medium";
  if (/(Unpacking|Setting up|Processing triggers) /.test(line)) return "text-rust-400";
  if (/Restarting services/.test(line)) return "text-rust-400";
  if (/WARNING|W:/.test(line)) return "text-warn-400";
  if (/ERROR|E:/.test(line)) return "text-danger-400";
  return "text-dark-300";
}

export default function Updates() {
  const { user } = useAuth();
  const [packages, setPackages] = useState<PackageUpdate[]>([]);
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [checked, setChecked] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [message, setMessage] = useState({ text: "", type: "" });
  const [rebootRequired, setRebootRequired] = useState(false);
  const [aptOutput, setAptOutput] = useState<string[] | null>(null);
  const [updateDone, setUpdateDone] = useState<"success" | "error" | null>(null);
  const [lastChecked, setLastChecked] = useState<Date | null>(null);
  const [confirmReboot, setConfirmReboot] = useState(false);
  const termRef = useRef<HTMLDivElement>(null);

  if (user?.role !== "admin") return <Navigate to="/" replace />;

  // Auto-load updates on mount
  useEffect(() => {
    setLoading(true);
    Promise.all([
      api.get<{ count: number; security: number; reboot_required: boolean }>("/system/updates/count"),
      api.get<PackageUpdate[]>("/system/updates"),
    ])
      .then(([countData, listData]) => {
        setRebootRequired(countData.reboot_required);
        setPackages(Array.isArray(listData) ? listData : []);
        setChecked(true);
        setLastChecked(new Date());
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, []);

  // Auto-scroll terminal to bottom
  useEffect(() => {
    if (termRef.current) {
      termRef.current.scrollTop = termRef.current.scrollHeight;
    }
  }, [aptOutput]);

  const checkUpdates = async () => {
    setLoading(true);
    setMessage({ text: "", type: "" });
    try {
      const data = await api.get<PackageUpdate[]>("/system/updates");
      setPackages(Array.isArray(data) ? data : []);
      setSelected(new Set());
      setChecked(true);
      setLastChecked(new Date());
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to check for updates",
        type: "error",
      });
    } finally {
      setLoading(false);
    }
  };

  const applyUpdates = async () => {
    setApplying(true);
    setMessage({ text: "", type: "" });
    setUpdateDone(null);
    const cmdLabel = selected.size > 0
      ? `$ apt-get install -y ${Array.from(selected).join(" ")}`
      : "$ apt-get upgrade -y";
    setAptOutput([cmdLabel, ""]);
    try {
      const body = selected.size > 0 ? { packages: Array.from(selected) } : {};
      const result = await api.post<{ install_id?: string } & ApplyResult>("/system/updates/apply", body);

      if (result.install_id) {
        // SSE mode — stream progress
        const es = new EventSource(`/api/services/install/${result.install_id}/log`);
        es.onmessage = (event) => {
          try {
            const step = JSON.parse(event.data);
            if (step.step === "line") {
              // Live streaming: append each apt output line as it arrives
              setAptOutput(prev => [...(prev || []), step.label]);
              // Auto-scroll the output container
              requestAnimationFrame(() => {
                const el = document.getElementById("apt-output");
                if (el) el.scrollTop = el.scrollHeight;
              });
            }
            if (step.step === "update") {
              if (step.status === "in_progress") {
                setAptOutput(prev => [...(prev || []), "> Starting package update..."]);
              }
            }
            if (step.step === "complete") {
              es.close();
              const success = step.status === "done";
              setUpdateDone(success ? "success" : "error");
              setAptOutput(prev => [
                ...(prev || []),
                "",
                success
                  ? "> Update completed successfully"
                  : "> Update completed with errors — check the output above",
              ]);
              setMessage({
                text: success
                  ? selected.size > 0
                    ? `Successfully updated ${selected.size} package(s)`
                    : "All packages updated successfully"
                  : "Update completed with errors — check the output below",
                type: success ? "success" : "error",
              });
              // Refresh package list
              api.get<PackageUpdate[]>("/system/updates")
                .then(data => { setPackages(Array.isArray(data) ? data : []); setSelected(new Set()); })
                .catch(() => {});
              api.get<{ reboot_required: boolean }>("/system/updates/count")
                .then(d => setRebootRequired(d.reboot_required))
                .catch(() => {});
              setApplying(false);
            }
          } catch { /* ignore */ }
        };
        es.onerror = () => {
          es.close();
          setApplying(false);
        };
      } else {
        // Fallback: synchronous response (old behavior)
        if (result.output) setAptOutput(result.output.split("\n"));
        setRebootRequired(result.reboot_required);
        setUpdateDone(result.success ? "success" : "error");
        setMessage({
          text: result.success ? "All packages updated" : "Update completed with errors",
          type: result.success ? "success" : "error",
        });
        const data = await api.get<PackageUpdate[]>("/system/updates");
        setPackages(Array.isArray(data) ? data : []);
        setSelected(new Set());
        setApplying(false);
      }
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to apply updates",
        type: "error",
      });
      setApplying(false);
    }
  };

  const handleReboot = async () => {
    setConfirmReboot(false);
    try {
      const result = await api.post<{ success: boolean; message: string }>("/system/reboot");
      setMessage({
        text: result.message,
        type: result.success ? "success" : "error",
      });
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to initiate reboot",
        type: "error",
      });
    }
  };

  const toggleSelect = (name: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };

  const toggleAll = () => {
    if (selected.size === packages.length) {
      setSelected(new Set());
    } else {
      setSelected(new Set(packages.map((p) => p.name)));
    }
  };

  const securityCount = packages.filter((p) => p.security).length;

  return (
    <div>
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6">
        <div>
          <p className="text-xs text-dark-300 font-mono">
            Manage system package updates
          </p>
          {lastChecked && !loading && (
            <p className="text-[10px] text-dark-400 font-mono mt-1">
              Last checked: {lastChecked.toLocaleTimeString()}
            </p>
          )}
        </div>
        <div className="flex items-center gap-2">
          {packages.length > 0 && (
            <button
              onClick={applyUpdates}
              disabled={applying}
              className="px-4 py-2 bg-rust-600 text-white rounded-lg text-sm font-medium hover:bg-rust-700 disabled:opacity-50 flex items-center gap-2"
            >
              {applying && (
                <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                </svg>
              )}
              {applying
                ? "Applying..."
                : selected.size > 0
                  ? `Update Selected (${selected.size})`
                  : "Update All"}
            </button>
          )}
          <button
            onClick={checkUpdates}
            disabled={loading}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 flex items-center gap-2"
          >
            {loading && (
              <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
              </svg>
            )}
            {loading ? "Checking..." : "Check for Updates"}
          </button>
        </div>
      </div>

      {/* Reboot Required Banner */}
      {rebootRequired && (
        <div className="border border-warn-500/50 bg-warn-500/5 p-4 flex items-start gap-3 mb-6">
          <svg className="w-5 h-5 text-warn-400 shrink-0 mt-0.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
          </svg>
          <div className="flex-1">
            <p className="text-sm text-warn-400 font-bold">Reboot Required</p>
            <p className="text-xs text-dark-300 mt-1">
              Recent package updates (such as a new kernel version) require a reboot to be fully applied.
            </p>
          </div>
          {!confirmReboot ? (
            <button
              onClick={() => setConfirmReboot(true)}
              className="px-4 py-2 bg-warn-500 text-dark-900 text-xs font-bold uppercase tracking-wider hover:bg-warn-400 transition-colors shrink-0"
            >
              Reboot Now
            </button>
          ) : (
            <div className="flex items-center gap-2 shrink-0">
              <span className="text-xs text-warn-400 font-mono">Are you sure?</span>
              <button
                onClick={handleReboot}
                className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors"
              >
                Confirm
              </button>
              <button
                onClick={() => setConfirmReboot(false)}
                className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors"
              >
                Cancel
              </button>
            </div>
          )}
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
        </div>
      )}

      {/* Terminal output */}
      {(applying || aptOutput) && (
        <div className="mb-6">
          {/* Terminal header */}
          <div className="flex items-center justify-between border border-dark-500 border-b-0 bg-dark-800 px-4 py-2">
            <div className="flex items-center gap-3">
              <span className="text-xs text-dark-300 uppercase tracking-widest font-mono">apt output</span>
              {applying && (
                <span className="text-[10px] text-rust-400 font-mono animate-pulse">running...</span>
              )}
              {!applying && updateDone === "success" && (
                <span className="text-[10px] text-rust-400 font-mono">done</span>
              )}
              {!applying && updateDone === "error" && (
                <span className="text-[10px] text-danger-400 font-mono">failed</span>
              )}
            </div>
            <div className="flex items-center gap-3">
              {!applying && aptOutput && (
                <button
                  onClick={() => { setAptOutput(null); setUpdateDone(null); }}
                  className="text-[10px] text-dark-400 hover:text-dark-200 font-mono uppercase tracking-wider transition-colors"
                >
                  Clear
                </button>
              )}
              <div className="flex gap-1.5">
                <div className={`w-2.5 h-2.5 rounded-full transition-colors ${
                  applying ? "bg-warn-400 animate-pulse"
                  : updateDone === "success" ? "bg-rust-400"
                  : updateDone === "error" ? "bg-danger-400"
                  : "bg-dark-500"
                }`} />
                <div className="w-2.5 h-2.5 rounded-full bg-dark-500" />
                <div className="w-2.5 h-2.5 rounded-full bg-dark-500" />
              </div>
            </div>
          </div>
          {/* Terminal body */}
          <div
            id="apt-output"
            ref={termRef}
            className="border border-dark-500 bg-dark-950 p-4 h-80 overflow-y-auto font-mono text-[11px] leading-relaxed"
          >
            {applying && !aptOutput && (
              <div className="text-rust-400">
                <span>&gt; Running apt upgrade...</span>
                <span className="inline-block w-2 h-3.5 bg-rust-400 ml-1 animate-pulse" />
              </div>
            )}
            {aptOutput &&
              aptOutput.map((line, i) => (
                <div key={i} className={colorLine(line)}>
                  {line || "\u00A0"}
                </div>
              ))}
          </div>
        </div>
      )}

      {/* Summary cards */}
      {checked && (
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-6">
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
            <div className="flex items-center gap-2.5">
              <div className="w-9 h-9 rounded-lg bg-accent-500/10 flex items-center justify-center">
                <svg className="w-5 h-5 text-accent-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.993 0 3.181 3.183a8.25 8.25 0 0 0 13.803-3.7M4.031 9.865a8.25 8.25 0 0 1 13.803-3.7l3.181 3.182m0-4.991v4.99" />
                </svg>
              </div>
              <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-wider">Available</p>
            </div>
            <p className="text-3xl font-bold text-dark-50 mt-2">{packages.length}</p>
            <p className="text-xs text-dark-300 mt-1">packages to update</p>
          </div>

          <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
            <div className="flex items-center gap-2.5">
              <div className="w-9 h-9 rounded-lg bg-danger-500/10 flex items-center justify-center">
                <svg className="w-5 h-5 text-danger-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
                </svg>
              </div>
              <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-wider">Security</p>
            </div>
            <p className={`text-3xl font-bold mt-2 ${securityCount > 0 ? "text-danger-400" : "text-rust-400"}`}>
              {securityCount}
            </p>
            <p className="text-xs text-dark-300 mt-1">security patches</p>
          </div>

          <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
            <div className="flex items-center gap-2.5">
              <div className="w-9 h-9 rounded-lg bg-rust-500/10 flex items-center justify-center">
                <svg className="w-5 h-5 text-rust-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75 11.25 15 15 9.75M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
                </svg>
              </div>
              <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-wider">Selected</p>
            </div>
            <p className="text-3xl font-bold text-dark-50 mt-2">
              {selected.size || "All"}
            </p>
            <p className="text-xs text-dark-300 mt-1">
              {selected.size > 0 ? "packages selected" : "packages will update"}
            </p>
          </div>
        </div>
      )}

      {/* Package table */}
      {checked && packages.length === 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <svg className="w-12 h-12 text-rust-400 mx-auto mb-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75 11.25 15 15 9.75M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
          </svg>
          <p className="text-dark-50 text-sm font-medium">System is up to date</p>
          <p className="text-dark-300 text-xs mt-1 font-mono">All packages are at their latest versions</p>
        </div>
      ) : checked && packages.length > 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
          <table className="w-full">
            <thead>
              <tr className="bg-dark-900">
                <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2 w-10">
                  <input
                    type="checkbox"
                    checked={selected.size === packages.length && packages.length > 0}
                    onChange={toggleAll}
                    className="w-3.5 h-3.5 accent-rust-500"
                  />
                </th>
                <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Package</th>
                <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Current</th>
                <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Available</th>
                <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Repo</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-600">
              {packages.map((pkg) => (
                <tr
                  key={pkg.name}
                  className={`hover:bg-dark-700/30 transition-colors ${
                    selected.has(pkg.name) ? "bg-dark-700/20" : ""
                  }`}
                >
                  <td className="px-5 py-2.5">
                    <input
                      type="checkbox"
                      checked={selected.has(pkg.name)}
                      onChange={() => toggleSelect(pkg.name)}
                      className="w-3.5 h-3.5 accent-rust-500"
                    />
                  </td>
                  <td className="px-5 py-2.5 text-sm text-dark-50 font-mono">
                    <div className="flex items-center gap-2">
                      {pkg.name}
                      {pkg.security && (
                        <span className="px-1.5 py-0.5 bg-danger-500/15 text-danger-400 rounded text-[10px] font-semibold uppercase tracking-wider border border-danger-500/20">
                          Security
                        </span>
                      )}
                    </div>
                  </td>
                  <td className="px-5 py-2.5 text-sm text-dark-300 font-mono">{pkg.current_version}</td>
                  <td className="px-5 py-2.5 text-sm text-rust-400 font-mono">{pkg.new_version}</td>
                  <td className="px-5 py-2.5 text-sm text-dark-200 font-mono">{pkg.repo}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : !checked ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <svg className="w-12 h-12 text-dark-300 mx-auto mb-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.993 0 3.181 3.183a8.25 8.25 0 0 0 13.803-3.7M4.031 9.865a8.25 8.25 0 0 1 13.803-3.7l3.181 3.182m0-4.991v4.99" />
          </svg>
          <p className="text-dark-300 text-sm">Click "Check for Updates" to scan for available package updates</p>
        </div>
      ) : null}
    </div>
  );
}
