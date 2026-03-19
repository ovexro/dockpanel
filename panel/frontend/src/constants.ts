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
