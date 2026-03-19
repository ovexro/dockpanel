export const layoutThemes = [
  {
    id: "command" as const,
    name: "Command",
    description: "Full sidebar, terminal personality",
  },
  {
    id: "glass" as const,
    name: "Glass",
    description: "Minimal icon sidebar, modern SaaS",
  },
  {
    id: "atlas" as const,
    name: "Atlas",
    description: "Top navbar, enterprise dashboard",
  },
] as const;

export type LayoutTheme = (typeof layoutThemes)[number]["id"];
export const layoutOrder: LayoutTheme[] = ["command", "glass", "atlas"];
