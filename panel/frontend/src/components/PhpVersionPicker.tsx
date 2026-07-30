import { useState, useEffect, useCallback } from "react";
import { api, ApiError } from "../api";
import ProvisionLog from "./ProvisionLog";

interface PhpVersion {
  version: string;
  installed: boolean;
  fpm_running: boolean;
  socket: string;
}

interface Props {
  value: string;
  /**
   * Called with the chosen version and whether this server can actually serve
   * it. `ready` is false for a version that is not installed, or is installed
   * with its FPM socket absent — the two states the agent refuses a PHP vhost
   * for. Callers that act immediately on a change should act only when it is
   * true; the picker offers the install itself and calls back with `true` once
   * the version really is usable.
   */
  onChange: (version: string, ready: boolean) => void;
  disabled?: boolean;
  /** Installing is admin-only server-side; a non-admin sees the state, not the button. */
  canInstall?: boolean;
  id?: string;
  className?: string;
}

/**
 * The PHP version control, with the one thing it was missing: knowledge of what
 * this server has.
 *
 * Both pickers used to be a hardcoded list of five options and no idea which of
 * them existed, so choosing an uninstalled version failed at the agent with
 * "PHP 8.3 is not installed…" and a pointer to a Settings page whose PHP tile
 * has no version on it and — on any box that already has some PHP — no install
 * button at all. The install has existed end to end since v2.8: agent route,
 * repo configuration for Debian and Ubuntu, RHEL module streams, backend proxy.
 * Nothing in the browser had ever called it.
 */
export default function PhpVersionPicker({
  value,
  onChange,
  disabled,
  canInstall,
  id,
  className,
}: Props) {
  const [versions, setVersions] = useState<PhpVersion[]>([]);
  const [loadFailed, setLoadFailed] = useState(false);
  const [installId, setInstallId] = useState<string | null>(null);
  const [installing, setInstalling] = useState<string | null>(null);
  const [error, setError] = useState("");

  // Returns null for "could not ask", which is not the same answer as "asked and
  // there is nothing". Conflating them made a fetch hiccup immediately after a
  // successful install read as a failed install — and made an agent that answered
  // with an empty list read as every version being present.
  const load = useCallback(async (): Promise<PhpVersion[] | null> => {
    try {
      const res = await api.get<{ versions: PhpVersion[] }>("/php/versions");
      const list = res.versions ?? [];
      setVersions(list);
      setLoadFailed(list.length === 0);
      return list;
    } catch {
      // An agent that cannot be reached, or one a release behind that has no
      // such route, must not take the picker with it — fall back to offering
      // every version unannotated, which is exactly the old behaviour.
      setLoadFailed(true);
      return null;
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // A safety net under ProvisionLog rather than a second progress display.
  //
  // ProvisionLog only calls `onComplete` on a stream error when it has rendered
  // NOTHING, and this install always emits a step before the agent is even
  // called — so any error after that point (the panel restarting, a proxy
  // dropping a long-lived SSE connection, the log channel being reclaimed)
  // leaves the spinner turning with nothing left to end it. The install itself
  // is unaffected: it runs on the server. So while one is in flight, ask the
  // question the log was only ever a proxy for.
  useEffect(() => {
    if (!installing) return;
    const started = Date.now();
    const t = window.setInterval(async () => {
      const fresh = await load();
      const now = fresh?.find((x) => x.version === installing);
      if (now && now.installed && now.fpm_running) {
        window.clearInterval(t);
        const version = installing;
        setInstalling(null);
        setInstallId(null);
        onChange(version, true);
      } else if (Date.now() - started > 20 * 60 * 1000) {
        // Past any plausible install. Stop asserting that something is happening.
        window.clearInterval(t);
        setInstalling(null);
        setError(
          `PHP ${installing} has not appeared after 20 minutes. Check the server's package manager and systemd logs.`,
        );
      }
    }, 10000);
    return () => window.clearInterval(t);
  }, [installing, load, onChange]);

  const known = (v: string) => versions.find((x) => x.version === v);
  const isReady = (v: string) => {
    const found = known(v);
    // Unknown means unknowable, not missing: with no list, every version is
    // offered as before and the server stays the authority. This is why an empty
    // response counts as a failed load rather than as an answer — otherwise a
    // server that listed nothing would be read as a server that has everything.
    return found ? found.installed && found.fpm_running : true;
  };

  const selected = known(value);
  const needsInstall = !loadFailed && !isReady(value);

  const handleInstall = async () => {
    setError("");
    setInstalling(value);
    try {
      const res = await api.post<{ install_id: string }>("/php/install", { version: value });
      setInstallId(res.install_id);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Could not start the install");
      setInstalling(null);
    }
  };

  const handleInstalled = async () => {
    const fresh = await load();
    // Could not ask. Say nothing rather than the wrong thing — the poll above is
    // still running and will settle this as soon as the server answers again.
    if (fresh === null) return;
    const now = fresh.find((x) => x.version === installing);
    // The verdict comes from re-reading the server, not from the log ending.
    // ProvisionLog calls this both when the run reports "complete" — whatever its
    // status — and when the stream simply dies with nothing rendered, so the log
    // finishing says only that it stopped.
    const ok = !!now && now.installed && now.fpm_running;
    const version = installing;
    setInstalling(null);
    if (ok) {
      setInstallId(null);
      if (version) onChange(version, true);
      return;
    }
    // Leave the log up. It is holding the only account of what went wrong —
    // apt's own error, or the agent saying FPM never opened its socket — and
    // clearing it here would take that off the screen two seconds after it
    // arrived. Clearing `installing` is enough to bring the button back for a
    // retry beside it.
  };

  const label = (v: PhpVersion) => {
    if (!v.installed) return `PHP ${v.version} — not installed`;
    if (!v.fpm_running) return `PHP ${v.version} — installed, FPM stopped`;
    return `PHP ${v.version}`;
  };

  // Whatever the server offers, plus the current value if the server does not
  // list it. A site created before a version left the allow-list — or by an API
  // caller, which does not validate `php_version` — would otherwise select
  // nothing at all and the control would render blank over a site that is
  // running something.
  const base: PhpVersion[] = loadFailed
    ? ["8.5", "8.4", "8.3", "8.2", "8.1"].map((version) => ({
        version,
        installed: true,
        fpm_running: true,
        socket: "",
      }))
    : [...versions];
  if (value && !base.some((v) => v.version === value)) {
    base.push({ version: value, installed: false, fpm_running: false, socket: "" });
  }
  const offered = base.sort((a, b) =>
    b.version.localeCompare(a.version, undefined, { numeric: true }),
  );

  return (
    <div className={className}>
      <select
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value, isReady(e.target.value))}
        disabled={disabled || !!installing}
        className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm bg-dark-800 disabled:opacity-50"
      >
        {offered.map((v) => (
          <option key={v.version} value={v.version}>
            {loadFailed ? `PHP ${v.version}` : label(v)}
          </option>
        ))}
      </select>

      {needsInstall && !installing && (
        <div className="mt-2 px-3 py-2.5 bg-warn-500/10 border border-warn-500/30 rounded-lg">
          <p className="text-xs text-warn-400">
            {selected && !selected.installed
              ? `PHP ${value} is not installed on this server.`
              : `PHP ${value} is installed but its PHP-FPM service is not running.`}{" "}
            Sites cannot use it until that is fixed.
          </p>
          {canInstall ? (
            <button
              type="button"
              onClick={handleInstall}
              disabled={!!installing}
              className="mt-2 px-3 py-1.5 bg-rust-500 text-dark-950 rounded-md text-xs font-bold hover:bg-rust-400 transition-colors disabled:opacity-50"
            >
              {installing ? "Starting…" : `Install PHP ${value}`}
            </button>
          ) : (
            <p className="text-xs text-dark-300 mt-1">
              Ask an administrator to install it.
            </p>
          )}
          {error && <p className="text-xs text-danger-400 mt-2">{error}</p>}
        </div>
      )}

      {installId && (
        <div className="mt-2">
          {installing && (
            <p className="text-xs text-dark-300 mb-2">
              Installing PHP {installing}. This adds the packages and, on some distributions, a
              PHP repository first — a few minutes is normal. It runs on the server, so leaving
              this page will not stop it.
            </p>
          )}
          <ProvisionLog
            sseUrl={`/api/services/install/${installId}/log`}
            onComplete={handleInstalled}
          />
        </div>
      )}
    </div>
  );
}
