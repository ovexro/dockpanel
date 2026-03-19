export interface NavItem {
  to: string;
  label: string;
  iconName: string;
  adminOnly?: boolean;
}

export interface NavGroup {
  key: string;
  label: string;
  items: NavItem[];
}

export const navGroups: NavGroup[] = [
  {
    key: "hosting",
    label: "Hosting",
    items: [
      { to: "/", label: "Dashboard", iconName: "dashboard" },
      { to: "/sites", label: "Sites", iconName: "sites" },
      { to: "/databases", label: "Databases", iconName: "databases" },
      { to: "/apps", label: "Docker Apps", iconName: "apps", adminOnly: true },
      { to: "/git-deploys", label: "Git Deploy", iconName: "gitDeploys", adminOnly: true },
    ],
  },
  {
    key: "operations",
    label: "Operations",
    items: [
      { to: "/dns", label: "DNS", iconName: "dns" },
      { to: "/mail", label: "Mail", iconName: "mail", adminOnly: true },
      { to: "/monitoring", label: "Monitoring", iconName: "monitoring" },
      { to: "/logs", label: "Logs", iconName: "logs", adminOnly: true },
      { to: "/terminal", label: "Terminal", iconName: "terminal" },
    ],
  },
  {
    key: "admin",
    label: "Admin",
    items: [
      { to: "/security", label: "Security", iconName: "security", adminOnly: true },
      { to: "/settings", label: "Settings", iconName: "settings", adminOnly: true },
    ],
  },
];

// Sub-navigation for Atlas context sidebar
export const subNavMap: Record<string, { label: string; to: string; iconName: string }[]> = {
  "/sites/:id": [
    { label: "Overview", to: "", iconName: "sites" },
    { label: "Files", to: "/files", iconName: "files" },
    { label: "Backups", to: "/backups", iconName: "backups" },
    { label: "Crons", to: "/crons", iconName: "crons" },
    { label: "Deploy", to: "/deploy", iconName: "deploy" },
    { label: "WordPress", to: "/wordpress", iconName: "wordpress" },
  ],
};
