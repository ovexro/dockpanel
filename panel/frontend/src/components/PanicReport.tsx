// The panic button's fleet result — specifically, the hosts it did NOT reach.
//
// `POST /api/security/panic` fans out across every registered server, so its
// legacy trio (`agent_reached`, `terminals_killed`, `server_terminals_killed`)
// are now fleet-wide: the counts are sums, and `agent_reached` is false whenever
// ANY member was left unswept. That degrades safely — an old client still prints
// "terminals may still be running" — but a warning that cannot name a machine
// cannot be acted on. Un-swept means one thing and it is the worst thing here:
// NOBODY KILLED ANYTHING ON THAT BOX, so a root shell an intruder is holding
// there is still holding.
//
// Rendered as a block rather than a toast because this is an emergency control:
// SecurityHardening's banner auto-clears after 5s, both banners are one line
// among many, and the panic revokes the pressing admin's own session — the
// browser gets one render of the most detailed account of this event that will
// ever exist outside the audit log.

/** One member that was not swept, and why. */
export interface UnreachableServer {
  server_id: string;
  server_name: string;
  reason: string;
}

/** Per-host outcome. Present for reached and un-reached members alike. */
export interface PanicServerResult {
  server_id: string;
  server_name: string;
  is_local: boolean;
  reached: boolean;
  terminals_killed: number | null;
  server_terminals_killed: number | null;
  registered?: number | null;
  error?: string;
}

export interface PanicResult {
  // Legacy trio — same names and types as before the fan-out, fleet-wide in
  // meaning. `terminals_killed` is null only when not a single host answered.
  agent_reached: boolean;
  terminals_killed: number | null;
  server_terminals_killed: number | null;
  sessions_revoked: boolean;
  shares_revoked: number;
  // Fleet detail. Optional so a panel whose backend predates the fan-out still
  // type-checks and still renders its legacy warning instead of crashing.
  servers_total?: number;
  servers_reached?: number;
  servers_unreachable?: number;
  server_results?: PanicServerResult[];
  unreachable_servers?: UnreachableServer[];
}

/** "web-2", "web-2 and db-1", "web-2, db-1 and edge-3". */
export function serverList(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** The un-swept host names, in the order the backend reported them. */
export function unreachableNames(result: PanicResult): string[] {
  return (result.unreachable_servers ?? []).map((s) => s.server_name);
}

/**
 * The un-swept machines, named, with the reason each one was missed.
 *
 * Renders nothing when the whole fleet was swept — the caller's success banner
 * already says so, and an empty "all clear" panel would train an operator to
 * skip the place where the names appear.
 */
export default function PanicReport({
  result,
  onDismiss,
}: {
  result: PanicResult;
  /** Leave the page deliberately. The panic revoked this admin's session. */
  onDismiss?: () => void;
}) {
  const unreachable = result.unreachable_servers ?? [];
  if (unreachable.length === 0) return null;

  const total = result.servers_total ?? unreachable.length;
  const reached = result.servers_reached ?? Math.max(total - unreachable.length, 0);
  const names = serverList(unreachable.map((s) => s.server_name));

  return (
    <div
      role="alert"
      aria-live="assertive"
      className="mb-4 rounded-lg border-2 border-danger-500/60 bg-danger-500/10 overflow-hidden"
    >
      <div className="px-5 py-4 border-b border-danger-500/30 flex items-start gap-3">
        <svg
          className="w-6 h-6 text-danger-400 shrink-0 mt-0.5"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
          aria-hidden="true"
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            d="M12 9v4m0 4h.01M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0Z"
          />
        </svg>
        <div className="min-w-0">
          <h2 className="text-sm font-bold uppercase font-mono tracking-widest text-danger-400">
            Root shells may still be live on {names}
          </h2>
          <p className="text-xs text-dark-200 font-mono mt-1">
            {reached} of {total} server{total === 1 ? "" : "s"} swept ·{" "}
            {unreachable.length} NOT swept — nothing was killed on{" "}
            {unreachable.length === 1 ? "it" : "them"}.
          </p>
        </div>
      </div>

      <ul className="divide-y divide-danger-500/20">
        {unreachable.map((s) => (
          <li
            key={s.server_id}
            className="px-5 py-3 flex flex-col sm:flex-row sm:items-baseline gap-1 sm:gap-4"
          >
            <span className="text-sm font-bold font-mono text-danger-400 shrink-0 sm:w-48">
              {s.server_name}
            </span>
            <span className="text-xs text-dark-200 font-mono break-all">{s.reason}</span>
          </li>
        ))}
      </ul>

      <div className="px-5 py-3 border-t border-danger-500/30 flex flex-col sm:flex-row sm:items-center justify-between gap-3">
        <p className="text-xs text-dark-200 font-mono">
          Lockdown and session revocation are panel-wide and did apply. Terminals were not:
          reach {names} by hand and kill any live session there.
        </p>
        {onDismiss && (
          <button
            onClick={onDismiss}
            className="px-3 py-1.5 bg-dark-600 text-dark-100 text-xs font-bold uppercase tracking-wider rounded-lg hover:bg-dark-500 transition-colors shrink-0"
          >
            Sign in again
          </button>
        )}
      </div>
    </div>
  );
}
