export const statusColors: Record<string, string> = {
  active: "bg-emerald-500/15 text-emerald-400",
  creating: "bg-amber-500/15 text-amber-400",
  error: "bg-red-500/15 text-red-400",
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
