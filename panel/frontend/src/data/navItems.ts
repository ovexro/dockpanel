export interface NavItem {
  to: string;
  label: string;
  iconName: string;
  adminOnly?: boolean;
  /** Visible to reseller role (and admin, which sees everything) */
  resellerVisible?: boolean;
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
      { to: "/wordpress-toolkit", label: "WP Toolkit", iconName: "wordpress", adminOnly: true },
      { to: "/apps", label: "Docker Apps", iconName: "apps", adminOnly: true },
      { to: "/git-deploys", label: "Git Deploy", iconName: "gitDeploys", adminOnly: true },
      { to: "/migration", label: "Migration", iconName: "migration", adminOnly: true },
    ],
  },
  {
    key: "reseller",
    label: "Reseller",
    items: [
      { to: "/reseller", label: "Reseller Panel", iconName: "reseller", resellerVisible: true },
      { to: "/reseller/users", label: "My Users", iconName: "users", resellerVisible: true },
    ],
  },
  {
    key: "operations",
    label: "Operations",
    items: [
      { to: "/dns", label: "DNS", iconName: "dns" },
      { to: "/cdn", label: "CDN", iconName: "dns", adminOnly: true },
      { to: "/mail", label: "Mail", iconName: "mail", adminOnly: true },
      { to: "/backup-orchestrator", label: "Backup Manager", iconName: "backups", adminOnly: true },
      { to: "/monitoring", label: "Monitoring", iconName: "monitoring" },
      { to: "/notifications", label: "Notifications", iconName: "notifications" },
      { to: "/logs", label: "Logs", iconName: "logs", adminOnly: true },
      { to: "/terminal", label: "Terminal", iconName: "terminal" },
    ],
  },
  {
    key: "admin",
    label: "Admin",
    items: [
      { to: "/servers", label: "Servers", iconName: "servers", adminOnly: true },
      { to: "/users", label: "Users", iconName: "users", adminOnly: true },
      { to: "/container-policies", label: "Container Policies", iconName: "servers", adminOnly: true },
      { to: "/integrations", label: "Integrations", iconName: "extensions", adminOnly: true },
      { to: "/secrets", label: "Secrets", iconName: "secrets", adminOnly: true },
      { to: "/security", label: "Security", iconName: "security", adminOnly: true },
      { to: "/system", label: "System", iconName: "servers", adminOnly: true },
      { to: "/telemetry", label: "Telemetry", iconName: "monitoring", adminOnly: true },
      { to: "/settings", label: "Settings", iconName: "settings", adminOnly: true },
    ],
  },
];
