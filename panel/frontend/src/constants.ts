/**
 * The largest file the file manager accepts, in bytes.
 *
 * ⛔ Must equal `UPLOAD_MAX_FILE_BYTES` in `panel/backend/src/routes/files.rs`,
 * the agent's copy in `panel/agent/src/routes/server_utils.rs`, and the figure
 * published in `docs/guides/file-uploads.md`. Four surfaces, four crates and
 * languages apart; `upload-refusal-pin-e2e.sh` is what holds them together.
 *
 * It is small because an upload is base64-encoded into a JSON body, and that
 * body is capped at 2 MiB by the server framework — twice, once at the panel
 * and again on the hop to the agent. Checking it here is what turns a long
 * upload ending in "1 failed" into an immediate sentence naming the limit.
 */
export const UPLOAD_MAX_FILE_BYTES = 1_500_000;

/** The refusal, worded the way the server words it. */
export function uploadTooLargeMessage(fileName: string, size: number): string {
  return (
    `${fileName} is ${(size / 1_000_000).toFixed(1)} MB — too large for the file ` +
    `manager (limit ${(UPLOAD_MAX_FILE_BYTES / 1_000_000).toFixed(1)} MB). Copy it ` +
    `with rsync or scp over the server's own SSH, or use the Migration wizard ` +
    `to move a whole site.`
  );
}

export const statusColors: Record<string, string> = {
  active: "bg-rust-500/15 text-rust-400",
  creating: "bg-warn-500/15 text-warn-400",
  error: "bg-danger-500/15 text-danger-400",
  stopped: "bg-dark-700 text-dark-200",
};

export const runtimeLabels: Record<string, string> = {
  static: "Static",
  php: "PHP",
  proxy: "Reverse Proxy",
};

export const runtimeLabelsDetailed: Record<string, string> = {
  static: "Static (HTML/CSS/JS)",
  php: "PHP",
  proxy: "Reverse Proxy",
};
