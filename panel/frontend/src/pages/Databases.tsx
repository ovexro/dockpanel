import { useState, useEffect, useRef, useCallback, FormEvent } from "react";
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

interface QueryResult {
  columns: string[];
  rows: string[][];
  row_count: number;
  execution_time_ms: number;
  truncated: boolean;
}

const engineLabels: Record<string, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL",
  mariadb: "MariaDB",
};

/* ─── SQL Browser ───────────────────────────────────────────────── */

interface SchemaOverview {
  tables?: QueryResult;
  foreign_keys?: QueryResult;
}

function SchemaBrowser({ database, onClose }: { database: Database; onClose: () => void }) {
  const [overview, setOverview] = useState<SchemaOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [selectedTable, setSelectedTable] = useState<string | null>(null);
  const [tableSchema, setTableSchema] = useState<QueryResult | null>(null);
  const [tableIndexes, setTableIndexes] = useState<QueryResult | null>(null);

  useEffect(() => {
    (async () => {
      try {
        const data = await api.get<SchemaOverview>(`/databases/${database.id}/schema-overview`);
        setOverview(data);
      } catch (e) {
        setError(e instanceof Error ? e.message : "Failed to load schema");
      } finally {
        setLoading(false);
      }
    })();
  }, [database.id]);

  const loadTableDetails = async (table: string) => {
    setSelectedTable(table);
    try {
      const [schema, indexes] = await Promise.all([
        api.get<QueryResult>(`/databases/${database.id}/tables/${encodeURIComponent(table)}`),
        api.get<QueryResult>(`/databases/${database.id}/indexes/${encodeURIComponent(table)}`),
      ]);
      setTableSchema(schema);
      setTableIndexes(indexes);
    } catch { /* ignore */ }
  };

  const tables = overview?.tables?.rows || [];
  const fks = overview?.foreign_keys?.rows || [];
  const fkCols = overview?.foreign_keys?.columns || [];

  // Map FK columns to named fields
  const srcTableIdx = fkCols.indexOf("source_table");
  const srcColIdx = fkCols.indexOf("source_column");
  const tgtTableIdx = fkCols.indexOf("target_table");
  const tgtColIdx = fkCols.indexOf("target_column");

  return (
    <div className="animate-fade-up">
      <div className="flex items-center gap-3 mb-5 pb-4 border-b border-dark-600">
        <button onClick={onClose} className="px-3 py-1.5 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors">
          &larr; Back
        </button>
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Schema Browser</h1>
        <span className="text-sm font-mono text-dark-50">{database.name}</span>
        <span className="px-2 py-0.5 bg-dark-700 text-dark-200 rounded text-xs font-medium">{database.engine}</span>
      </div>

      {loading && <p className="text-dark-400 text-sm">Loading schema...</p>}
      {error && <div className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20">{error}</div>}

      {!loading && !error && (
        <div className="flex gap-4" style={{ minHeight: "calc(100vh - 220px)" }}>
          {/* Left: Table list */}
          <div className="w-56 shrink-0 bg-dark-800 rounded-lg border border-dark-500 overflow-hidden flex flex-col">
            <div className="px-3 py-2.5 border-b border-dark-600">
              <span className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                Tables ({tables.length})
              </span>
            </div>
            <div className="overflow-y-auto flex-1 py-1">
              {tables.map((row: string[]) => {
                const name = row[0];
                const rowCount = row[2] || "0";
                const sizeKb = row[3] || "0";
                const hasFk = fks.some((fk: string[]) => fk[srcTableIdx] === name || fk[tgtTableIdx] === name);
                return (
                  <button
                    key={name}
                    onClick={() => loadTableDetails(name)}
                    className={`w-full text-left px-3 py-2 text-sm hover:bg-dark-700 transition-colors flex items-center justify-between ${
                      selectedTable === name ? "bg-dark-700 text-dark-50" : "text-dark-200"
                    }`}
                  >
                    <span className="flex items-center gap-1.5 truncate">
                      {hasFk && <span className="w-1.5 h-1.5 rounded-full bg-rust-400 shrink-0" title="Has relationships" />}
                      {name}
                    </span>
                    <span className="text-xs text-dark-400 shrink-0 ml-2">{rowCount}r</span>
                  </button>
                );
              })}
            </div>
          </div>

          {/* Right: Table detail + relationships */}
          <div className="flex-1 space-y-4">
            {selectedTable && tableSchema ? (
              <>
                <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
                  <div className="px-4 py-3 border-b border-dark-600 flex items-center justify-between">
                    <h3 className="text-sm font-medium text-dark-100 font-mono">{selectedTable}</h3>
                    <span className="text-xs text-dark-400">{tableSchema.rows?.length || 0} columns</span>
                  </div>
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-dark-600 bg-dark-700/50">
                        <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Column</th>
                        <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Type</th>
                        <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Nullable</th>
                        <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Default</th>
                        {database.engine !== "postgres" && <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Key</th>}
                      </tr>
                    </thead>
                    <tbody>
                      {(tableSchema.rows || []).map((col: string[], i: number) => (
                        <tr key={i} className="border-b border-dark-700 hover:bg-dark-700/30">
                          <td className="px-4 py-2 font-mono text-dark-100">{col[0]}</td>
                          <td className="px-4 py-2 font-mono text-accent-400">{col[1]}</td>
                          <td className="px-4 py-2 text-dark-300">{col[database.engine === "postgres" ? 3 : 2] === "YES" ? "Yes" : "No"}</td>
                          <td className="px-4 py-2 text-dark-400 font-mono text-xs">{col[database.engine === "postgres" ? 4 : 3] || "-"}</td>
                          {database.engine !== "postgres" && <td className="px-4 py-2 text-rust-400">{col[4] || ""}</td>}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>

                {/* Indexes */}
                {tableIndexes && tableIndexes.rows?.length > 0 && (
                  <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
                    <div className="px-4 py-3 border-b border-dark-600">
                      <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Indexes</h3>
                    </div>
                    <table className="w-full text-sm">
                      <thead>
                        <tr className="border-b border-dark-600 bg-dark-700/50">
                          {(tableIndexes.columns || []).map((c: string, i: number) => (
                            <th key={i} className="text-left px-4 py-2 text-xs text-dark-300 uppercase">{c}</th>
                          ))}
                        </tr>
                      </thead>
                      <tbody>
                        {(tableIndexes.rows || []).map((row: string[], i: number) => (
                          <tr key={i} className="border-b border-dark-700">
                            {row.map((cell: string, j: number) => (
                              <td key={j} className="px-4 py-2 font-mono text-dark-200 text-xs">{cell}</td>
                            ))}
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}

                {/* Foreign Keys involving this table */}
                {(() => {
                  const relatedFks = fks.filter((fk: string[]) => fk[srcTableIdx] === selectedTable || fk[tgtTableIdx] === selectedTable);
                  if (relatedFks.length === 0) return null;
                  return (
                    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
                      <div className="px-4 py-3 border-b border-dark-600">
                        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Relationships</h3>
                      </div>
                      <div className="p-4 space-y-2">
                        {relatedFks.map((fk: string[], i: number) => (
                          <div key={i} className="flex items-center gap-2 text-sm">
                            <span className="font-mono text-dark-100">{fk[srcTableIdx]}</span>
                            <span className="text-dark-400">.</span>
                            <span className="font-mono text-accent-400">{fk[srcColIdx]}</span>
                            <svg className="w-4 h-4 text-rust-400" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M14 5l7 7m0 0l-7 7m7-7H3" /></svg>
                            <span className="font-mono text-dark-100">{fk[tgtTableIdx]}</span>
                            <span className="text-dark-400">.</span>
                            <span className="font-mono text-accent-400">{fk[tgtColIdx]}</span>
                            {fk[4] && <span className="text-xs text-dark-400 ml-2">({fk[4]})</span>}
                          </div>
                        ))}
                      </div>
                    </div>
                  );
                })()}
              </>
            ) : (
              <div className="bg-dark-800 rounded-lg border border-dark-500 p-8 text-center">
                <p className="text-dark-400 text-sm">Select a table to view its schema, indexes, and relationships.</p>
                {fks.length > 0 && (
                  <div className="mt-6 text-left">
                    <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-3">All Relationships ({fks.length})</h3>
                    <div className="space-y-1.5">
                      {fks.map((fk: string[], i: number) => (
                        <div key={i} className="flex items-center gap-2 text-sm">
                          <button onClick={() => loadTableDetails(fk[srcTableIdx])} className="font-mono text-dark-100 hover:text-rust-400">{fk[srcTableIdx]}</button>
                          <span className="text-dark-400">.</span>
                          <span className="font-mono text-accent-400">{fk[srcColIdx]}</span>
                          <svg className="w-3 h-3 text-dark-400" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M14 5l7 7m0 0l-7 7m7-7H3" /></svg>
                          <button onClick={() => loadTableDetails(fk[tgtTableIdx])} className="font-mono text-dark-100 hover:text-rust-400">{fk[tgtTableIdx]}</button>
                          <span className="text-dark-400">.</span>
                          <span className="font-mono text-accent-400">{fk[tgtColIdx]}</span>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function SqlBrowser({
  database,
  onClose,
}: {
  database: Database;
  onClose: () => void;
}) {
  const [tables, setTables] = useState<string[]>([]);
  const [tablesLoading, setTablesLoading] = useState(true);
  const [selectedTable, setSelectedTable] = useState<string | null>(null);
  const [sql, setSql] = useState("");
  const [result, setResult] = useState<QueryResult | null>(null);
  const [queryLoading, setQueryLoading] = useState(false);
  const [queryError, setQueryError] = useState("");
  const [history, setHistory] = useState<string[]>([]);
  const [historyIdx, setHistoryIdx] = useState(-1);
  const editorRef = useRef<HTMLTextAreaElement>(null);

  const engine = database.engine;
  const q = engine === "postgres" ? '"' : "`";

  const loadTables = useCallback(async () => {
    setTablesLoading(true);
    try {
      const res = await api.get<QueryResult>(
        `/databases/${database.id}/tables`
      );
      setTables(res.rows.map((r) => r[0]));
    } catch (e) {
      setQueryError(e instanceof Error ? e.message : "Failed to load tables");
    } finally {
      setTablesLoading(false);
    }
  }, [database.id]);

  useEffect(() => {
    loadTables();
  }, [loadTables]);

  const executeQuery = useCallback(
    async (queryStr?: string) => {
      const toRun = queryStr || sql;
      if (!toRun.trim()) return;

      setQueryLoading(true);
      setQueryError("");
      setResult(null);
      try {
        const res = await api.post<QueryResult>(
          `/databases/${database.id}/query`,
          { sql: toRun }
        );
        setResult(res);
        // Add to history (deduplicate)
        setHistory((prev) => {
          const deduped = prev.filter((h) => h !== toRun);
          return [toRun, ...deduped].slice(0, 50);
        });
        setHistoryIdx(-1);
      } catch (e) {
        setQueryError(e instanceof Error ? e.message : "Query failed");
      } finally {
        setQueryLoading(false);
      }
    },
    [database.id, sql]
  );

  const selectTable = (table: string) => {
    setSelectedTable(table);
    const query = `SELECT * FROM ${q}${table}${q} LIMIT 100`;
    setSql(query);
    executeQuery(query);
  };

  const showSchema = async (table: string, e: React.MouseEvent) => {
    e.stopPropagation();
    setSelectedTable(table);
    setQueryLoading(true);
    setQueryError("");
    try {
      const res = await api.get<QueryResult>(
        `/databases/${database.id}/tables/${encodeURIComponent(table)}`
      );
      setResult(res);
      setSql("");
    } catch (err) {
      setQueryError(
        err instanceof Error ? err.message : "Failed to load schema"
      );
    } finally {
      setQueryLoading(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      executeQuery();
    }
    // History navigation with Ctrl+Up/Down
    if (e.ctrlKey && e.key === "ArrowUp" && history.length > 0) {
      e.preventDefault();
      const next = Math.min(historyIdx + 1, history.length - 1);
      setHistoryIdx(next);
      setSql(history[next]);
    }
    if (e.ctrlKey && e.key === "ArrowDown" && historyIdx > 0) {
      e.preventDefault();
      const next = historyIdx - 1;
      setHistoryIdx(next);
      setSql(history[next]);
    }
  };

  return (
    <div className="animate-fade-up">
      {/* Header */}
      <div className="flex items-center gap-3 mb-5 pb-4 border-b border-dark-600">
        <button
          onClick={onClose}
          className="px-3 py-1.5 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors"
        >
          &larr; Back
        </button>
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">
          SQL Browser
        </h1>
        <span className="text-sm font-mono text-dark-50">{database.name}</span>
        <span className="px-2 py-0.5 bg-dark-700 text-dark-200 rounded text-xs font-medium">
          {engineLabels[engine] || engine}
        </span>
      </div>

      {/* Two-column layout */}
      <div className="flex gap-4" style={{ minHeight: "calc(100vh - 220px)" }}>
        {/* Left: Tables sidebar */}
        <div className="w-52 shrink-0 bg-dark-800 rounded-lg border border-dark-500 overflow-hidden flex flex-col">
          <div className="px-3 py-2.5 border-b border-dark-600 flex items-center justify-between">
            <span className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
              Tables
            </span>
            <button
              onClick={loadTables}
              className="text-dark-400 hover:text-dark-200 transition-colors"
              title="Refresh tables"
            >
              <svg
                className="w-3.5 h-3.5"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.993 0 3.181 3.183a8.25 8.25 0 0 0 13.803-3.7M4.031 9.865a8.25 8.25 0 0 1 13.803-3.7l3.181 3.182"
                />
              </svg>
            </button>
          </div>
          <div className="flex-1 overflow-y-auto p-1.5">
            {tablesLoading ? (
              <div className="space-y-1.5 p-2">
                {[...Array(5)].map((_, i) => (
                  <div
                    key={i}
                    className="h-5 bg-dark-700 rounded animate-pulse"
                  />
                ))}
              </div>
            ) : tables.length === 0 ? (
              <div className="text-center text-dark-400 text-xs py-6">
                No tables found
              </div>
            ) : (
              tables.map((t) => (
                <div
                  key={t}
                  onClick={() => selectTable(t)}
                  className={`group flex items-center justify-between px-2.5 py-1.5 rounded cursor-pointer text-sm font-mono transition-colors ${
                    selectedTable === t
                      ? "bg-dark-600 text-dark-50"
                      : "text-dark-200 hover:bg-dark-700"
                  }`}
                >
                  <span className="truncate">{t}</span>
                  <button
                    onClick={(e) => showSchema(t, e)}
                    title="View schema"
                    className="opacity-0 group-hover:opacity-100 text-dark-400 hover:text-dark-200 transition-opacity shrink-0 ml-1"
                  >
                    <svg
                      className="w-3.5 h-3.5"
                      fill="none"
                      viewBox="0 0 24 24"
                      stroke="currentColor"
                      strokeWidth={2}
                    >
                      <path
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        d="m11.25 11.25.041-.02a.75.75 0 0 1 1.063.852l-.708 2.836a.75.75 0 0 0 1.063.853l.041-.021M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Zm-9-3.75h.008v.008H12V8.25Z"
                      />
                    </svg>
                  </button>
                </div>
              ))
            )}
          </div>
        </div>

        {/* Right: Editor + Results */}
        <div className="flex-1 min-w-0 flex flex-col gap-4">
          {/* SQL Editor */}
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-3">
            <textarea
              ref={editorRef}
              value={sql}
              onChange={(e) => setSql(e.target.value)}
              onKeyDown={handleKeyDown}
              className="w-full h-28 bg-dark-900 border border-dark-500 rounded-lg p-3 text-sm font-mono text-dark-100 resize-y focus:outline-none focus:border-dark-400 placeholder-dark-500"
              placeholder="SELECT * FROM ..."
              spellCheck={false}
            />
            <div className="flex items-center justify-between mt-2">
              <div className="flex items-center gap-3">
                <button
                  onClick={() => executeQuery()}
                  disabled={queryLoading || !sql.trim()}
                  className="flex items-center gap-2 px-4 py-1.5 bg-rust-500 text-white rounded text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
                >
                  {queryLoading && (
                    <span className="w-3.5 h-3.5 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  )}
                  Execute
                </button>
                <span className="text-xs text-dark-500">Ctrl+Enter</span>
              </div>
              {result && (
                <span className="text-xs text-dark-300">
                  {result.execution_time_ms}ms &middot; {result.row_count} row
                  {result.row_count !== 1 ? "s" : ""}
                  {result.truncated && " (truncated to 1000)"}
                </span>
              )}
            </div>
          </div>

          {/* Error */}
          {queryError && (
            <div
              role="alert"
              className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20"
            >
              <button
                onClick={() => setQueryError("")}
                className="float-right font-bold ml-2"
                aria-label="Close error"
              >
                &times;
              </button>
              <pre className="whitespace-pre-wrap font-mono text-xs">
                {queryError}
              </pre>
            </div>
          )}

          {/* Results table */}
          {result && result.columns.length > 0 && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto flex-1">
              <table className="w-full">
                <thead className="sticky top-0">
                  <tr className="border-b border-dark-500 bg-dark-900">
                    <th className="text-left text-[10px] font-medium text-dark-400 uppercase font-mono px-3 py-2 w-10">
                      #
                    </th>
                    {result.columns.map((col, i) => (
                      <th
                        key={i}
                        className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-3 py-2 whitespace-nowrap"
                      >
                        {col}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody className="divide-y divide-dark-700">
                  {result.rows.map((row, i) => (
                    <tr
                      key={i}
                      className="hover:bg-dark-700/30 transition-colors"
                    >
                      <td className="px-3 py-1.5 text-[10px] text-dark-500 font-mono">
                        {i + 1}
                      </td>
                      {row.map((val, j) => (
                        <td
                          key={j}
                          className="px-3 py-1.5 text-sm text-dark-100 font-mono whitespace-nowrap max-w-xs truncate"
                          title={val}
                        >
                          {val === "" || val === "\\N" ? (
                            <span className="text-dark-500 italic text-xs">
                              NULL
                            </span>
                          ) : (
                            val
                          )}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {/* Empty result (DML/DDL) */}
          {result &&
            result.columns.length === 0 &&
            result.rows.length === 0 &&
            !queryError && (
              <div className="bg-dark-800 rounded-lg border border-dark-500 p-8 text-center text-dark-300 text-sm">
                <svg
                  className="w-8 h-8 mx-auto text-rust-400 mb-2"
                  fill="none"
                  viewBox="0 0 24 24"
                  stroke="currentColor"
                  strokeWidth={2}
                >
                  <path
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    d="M4.5 12.75l6 6 9-13.5"
                  />
                </svg>
                Query executed successfully ({result.execution_time_ms}ms)
              </div>
            )}

          {/* Initial state */}
          {!result && !queryError && !queryLoading && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center flex-1 flex flex-col items-center justify-center">
              <svg
                className="w-10 h-10 text-dark-500 mb-3"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={1}
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M20.25 6.375c0 2.278-3.694 4.125-8.25 4.125S3.75 8.653 3.75 6.375m16.5 0c0-2.278-3.694-4.125-8.25-4.125S3.75 4.097 3.75 6.375m16.5 0v11.25c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125V6.375m16.5 0v3.75m-16.5-3.75v3.75m16.5 0v3.75C20.25 16.153 16.556 18 12 18s-8.25-1.847-8.25-4.125v-3.75m16.5 0c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125"
                />
              </svg>
              <p className="text-dark-300 text-sm">
                Select a table or write a query
              </p>
              <p className="text-dark-500 text-xs mt-1">
                Ctrl+Up/Down to navigate query history
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/* ─── Main Databases Page ───────────────────────────────────────── */

export default function Databases() {
  const [databases, setDatabases] = useState<Database[]>([]);
  const [sites, setSites] = useState<Site[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [error, setError] = useState("");
  const [successMsg, setSuccessMsg] = useState("");

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
  const [search, setSearch] = useState("");
  const [displayCount, setDisplayCount] = useState(25);

  // SQL Browser state
  const [browseDb, setBrowseDb] = useState<Database | null>(null);
  const [schemaDb, setSchemaDb] = useState<Database | null>(null);

  // Password reset state
  const [resettingPw, setResettingPw] = useState<string | null>(null);
  const [pwResetSuccess, setPwResetSuccess] = useState<string | null>(null);

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
    setSuccessMsg("");
    setSubmitting(true);
    try {
      await api.post("/databases", {
        site_id: siteId,
        name: dbName,
        engine,
      });
      setShowForm(false);
      setSuccessMsg(`Database "${dbName}" created`);
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
    setSuccessMsg("");
    try {
      await api.delete(`/databases/${id}`);
      setConfirmDeleteId(null);
      setSuccessMsg("Database deleted");
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

  const handleResetPassword = async (id: string) => {
    setResettingPw(id);
    setPwResetSuccess(null);
    setError("");
    try {
      const res = await api.post<{ ok: boolean; password: string }>(
        `/databases/${id}/reset-password`
      );
      setPwResetSuccess(res.password);
      // Show the new credentials panel with refreshed data
      setCredentialsId(id);
      setCredentials(null);
      setCredentialsLoading(true);
      try {
        const creds = await api.get<Credentials>(`/databases/${id}/credentials`);
        setCredentials(creds);
      } catch {
        // Credentials will be stale but password is shown in success message
      } finally {
        setCredentialsLoading(false);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Password reset failed");
    } finally {
      setResettingPw(null);
    }
  };

  const copyToClipboard = (text: string, label: string) => {
    navigator.clipboard.writeText(text);
    setCopied(label);
    setTimeout(() => setCopied(""), 2000);
  };

  const getSiteDomain = (siteId: string) =>
    sites.find((s) => s.id === siteId)?.domain || "Unknown";

  // Schema Browser mode
  if (schemaDb) {
    return (
      <div className="p-6 lg:p-8">
        <SchemaBrowser database={schemaDb} onClose={() => setSchemaDb(null)} />
      </div>
    );
  }

  // SQL Browser mode
  if (browseDb) {
    return (
      <div className="p-6 lg:p-8">
        <SqlBrowser database={browseDb} onClose={() => setBrowseDb(null)} />
      </div>
    );
  }

  return (
    <div className="animate-fade-up">
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Databases</h1>
          <p className="page-header-subtitle">Create and manage your databases</p>
        </div>
        <div className="flex items-center gap-2">
          {databases.length >= 2 && (
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search databases..."
              className="px-3 py-1.5 bg-dark-800 border border-dark-600 rounded-lg text-sm text-dark-100 placeholder-dark-400 focus:outline-none focus:border-dark-400"
            />
          )}
          <button
            onClick={() => setShowForm(!showForm)}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
          >
            {showForm ? "Cancel" : "Create Database"}
          </button>
        </div>
      </div>

      <div className="p-6 lg:p-8">

      {error && (
        <div role="alert" className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20 mb-4">
          {error}
          <button onClick={() => setError("")} className="float-right font-bold" aria-label="Close error">
            &times;
          </button>
        </div>
      )}

      {successMsg && (
        <div className="bg-rust-500/10 text-rust-400 text-sm px-4 py-3 rounded-lg border border-rust-500/20 mb-4">
          {successMsg}
          <button onClick={() => setSuccessMsg("")} className="float-right font-bold" aria-label="Dismiss">
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
              <p className="text-xs text-dark-300 mt-1">Name for your database instance</p>
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
              <p className="text-xs text-dark-300 mt-1">MySQL, MariaDB, or PostgreSQL</p>
            </div>
          </div>
          <div className="flex gap-3">
            <button
              type="submit"
              disabled={submitting}
              className="flex items-center gap-2 px-6 py-2.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
            >
              {submitting && <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />}
              {submitting ? "Creating..." : "Create Database"}
            </button>
            <button
              type="button"
              onClick={() => setShowForm(false)}
              className="px-4 py-2 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors"
            >
              Cancel
            </button>
          </div>
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
      ) : !showForm && databases.length === 0 ? (
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
          <p className="text-dark-300 text-sm mt-2 max-w-md mx-auto">Create MySQL or PostgreSQL databases with automatic Docker containers, user management, and a built-in SQL browser.</p>
          <button onClick={() => setShowForm(true)} className="mt-3 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors">
            Create your first database
          </button>
        </div>
      ) : databases.length > 0 ? (
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
                {(() => {
                  const filtered = databases.filter((db) => db.name.toLowerCase().includes(search.toLowerCase()));
                  const displayed = filtered.slice(0, displayCount);
                  return displayed.map((db) => (
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
                          onClick={() => setBrowseDb(db)}
                          className="px-2 py-1 rounded text-xs font-medium bg-dark-700 text-dark-200 hover:bg-dark-600 transition-colors"
                        >
                          Browse
                        </button>
                        <button
                          onClick={() => setSchemaDb(db)}
                          className="px-2 py-1 rounded text-xs font-medium bg-accent-500/10 text-accent-400 hover:bg-accent-500/15 transition-colors"
                        >
                          Schema
                        </button>
                        <button
                          onClick={async () => {
                            try {
                              const cfg = await api.get<{ pitr_enabled: boolean; retention_hours: number }>(`/databases/${db.id}/pitr`);
                              const newEnabled = !cfg.pitr_enabled;
                              await api.put(`/databases/${db.id}/pitr`, {
                                pitr_enabled: newEnabled,
                                retention_hours: cfg.retention_hours || 24,
                              });
                              setError("");
                              setSuccessMsg(`Point-in-time recovery ${newEnabled ? "enabled" : "disabled"} for ${db.name}`);
                            } catch (e) { setError(e instanceof Error ? e.message : "Failed to toggle PITR"); }
                          }}
                          className="px-2 py-1 rounded text-xs font-medium bg-dark-700 text-dark-300 hover:bg-dark-600 transition-colors"
                          title="Toggle point-in-time recovery (WAL/binlog)"
                        >
                          PITR
                        </button>
                        <button
                          onClick={() => handleResetPassword(db.id)}
                          disabled={resettingPw === db.id}
                          className="px-2 py-1 rounded text-xs font-medium bg-dark-700 text-dark-300 hover:bg-dark-600 disabled:opacity-50 transition-colors"
                          title="Generate a new database password"
                        >
                          {resettingPw === db.id ? "..." : "Reset PW"}
                        </button>
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
                              className="px-2 py-1 bg-danger-600 text-white rounded text-xs hover:bg-danger-700 disabled:opacity-50"
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
                ));
                })()}
              </tbody>
            </table>
            {(() => {
              const filtered = databases.filter((db) => db.name.toLowerCase().includes(search.toLowerCase()));
              const remaining = filtered.length - displayCount;
              return remaining > 0 ? (
                <button
                  onClick={() => setDisplayCount((c) => c + 25)}
                  className="w-full py-2 text-sm text-dark-300 hover:text-dark-100 border-t border-dark-600 hover:bg-dark-700/30 transition-colors"
                >
                  Show more ({remaining} remaining)
                </button>
              ) : null;
            })()}
          </div>

          {/* Password reset success */}
          {pwResetSuccess && (
            <div className="mt-4 bg-dark-800 rounded-lg border border-accent-500/30 p-4">
              <div className="flex items-center justify-between mb-2">
                <h3 className="text-xs font-medium text-accent-400 uppercase font-mono tracking-widest">
                  Password Reset Successful
                </h3>
                <button
                  onClick={() => setPwResetSuccess(null)}
                  className="text-dark-400 hover:text-dark-200 text-sm"
                  aria-label="Dismiss"
                >
                  &times;
                </button>
              </div>
              <p className="text-xs text-dark-300 mb-2">Save this password now — it will not be shown again after you leave this page.</p>
              <div className="flex items-center gap-2">
                <code className="flex-1 bg-dark-900 border border-dark-500 rounded-lg px-3 py-2 text-sm font-mono text-dark-50">
                  {pwResetSuccess}
                </code>
                <button
                  onClick={() => copyToClipboard(pwResetSuccess, "new_password")}
                  className="shrink-0 px-3 py-2 bg-dark-700 text-dark-200 rounded-lg text-xs hover:bg-dark-600 transition-colors"
                >
                  {copied === "new_password" ? "Copied!" : "Copy"}
                </button>
              </div>
            </div>
          )}

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
      ) : null}
      </div>
    </div>
  );
}
