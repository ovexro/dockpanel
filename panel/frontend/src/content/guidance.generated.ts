// DO NOT EDIT — generated from panel/backend/src/services/prerequisites/copy.rs
//
// The passive tier of the guidance layer: the line under a field, and the
// longer text behind its (i). The reactive and blocking tiers arrive at
// runtime as PrereqResults from the same registry.

export interface FieldGuidance {
  /** Surface this field belongs to, as a user would name it. */
  surface: string;
  label: string;
  /** The line under the field. Always shown. */
  help: string;
  /** The longer explanation behind the (i). Empty means no tooltip. */
  more: string;
  /** Prerequisite key this field's guidance escalates into, if any. */
  escalatesTo: string;
}

export const FIELD_GUIDANCE = {
  "setup.admin_password": {
    surface: "Create your admin account",
    label: "Password",
    help: "At least 8 characters. This signs you in to the panel itself — not to any site you host with it.",
    more: "This is the panel's first account and a full administrator: it can reach every site, database and container on this server. There is no password-reset email configured yet, so store it somewhere you trust before continuing. You can add two-factor authentication straight afterwards from Settings.",
    escalatesTo: "",
  },
  "sites.create.domain": {
    surface: "Create a site",
    label: "Domain",
    help: "Your site's public domain name (e.g. example.com). It needs to point at this server before HTTPS can be issued — DockPanel checks that for you below.",
    more: "You can create the site before the domain resolves; only the certificate needs DNS. If you are moving a live site, create it here first, confirm it serves correctly, and change the DNS record last — that way the switchover has no gap.",
    escalatesTo: "dns.points_here",
  },
  "sites.create.runtime": {
    surface: "Create a site",
    label: "Runtime",
    help: "",
    more: "Static serves files as they are. PHP runs them through PHP-FPM. Node and Python get a managed systemd process with nginx in front. Reverse Proxy is the one to pick when something else already listens on a port — a Docker app, or a service you run yourself — and DockPanel then owns only the front end and the certificate.",
    escalatesTo: "",
  },
  "sites.create.admin_password": {
    surface: "Create a site",
    label: "Admin Password",
    help: "Leave blank and DockPanel generates one, then stores it in this site's Secrets vault — it is not shown again anywhere else.",
    more: "Open the site, then Secrets, to read it back. Earlier versions generated this password, used it and discarded it, which left you unable to log in to the site you had just created.",
    escalatesTo: "",
  },
  "sites.create.proxy_port": {
    surface: "Create a site",
    label: "Proxy Port",
    help: "The local port your application listens on.",
    more: "The port as seen from the server itself, not a public one. nginx connects to it on 127.0.0.1; nothing needs to be opened in the firewall.",
    escalatesTo: "",
  },
  "apps.deploy.name": {
    surface: "Deploy a Docker app",
    label: "Container Name",
    help: "Names the container on this server. Must be unique — DockPanel suggests a free one if it isn't.",
    more: "The container is created as `dockpanel-app-<name>`, so this name is what you will see in `docker ps` minus that prefix.",
    escalatesTo: "apps.name_available",
  },
  "apps.deploy.port": {
    surface: "Deploy a Docker app",
    label: "Host Port",
    help: "The port on this server the app will answer on. DockPanel checks it is free before the image is pulled.",
    more: "Published on 127.0.0.1 by default. To expose the app publicly, put a Reverse Proxy site in front of it — that gets you a certificate and a domain instead of a port number.",
    escalatesTo: "apps.port_available",
  },
  "apps.deploy.memory": {
    surface: "Deploy a Docker app",
    label: "Memory (MB)",
    help: "Hard ceiling for this container. Leave blank for no limit.",
    more: "A limit above what the server has is accepted but unreachable — the box swaps first. DockPanel warns rather than refuses, because over-committing is ordinary and you may know what is about to be freed.",
    escalatesTo: "apps.resource_headroom",
  },
  "backups.policy.destination": {
    surface: "Backup policy",
    label: "Destination",
    help: "Where a copy is uploaded after each backup. Leave empty and backups stay on this server's own disk.",
    more: "A backup on the disk it protects covers a bad deploy or a dropped table, and nothing worse. Destinations are S3-compatible storage or any SFTP server. SFTP destinations that authenticate by PASSWORD need sshpass on the server the backup runs from: fresh installs and servers added since v2.74.0 get it automatically, but a panel upgraded in place does not, because update.sh upgrades binaries and installs no packages — run apt-get install sshpass (or dnf install sshpass) once. Key-authenticated SFTP and every S3 destination are unaffected, and when it is missing Test Connection names it rather than failing opaquely.",
    escalatesTo: "backups.destination_configured",
  },
  "mail.domain.records": {
    surface: "Mail domain → DNS",
    label: "Required DNS Records",
    help: "",
    more: "These are the records DockPanel publishes for you when it manages the zone, and the ones to create by hand when it doesn't. Verify DNS checks that each points at this server rather than merely that something exists — a domain whose MX belongs to another provider is reported as such, not as a pass.",
    escalatesTo: "mail.dns_published",
  },
} as const satisfies Record<string, FieldGuidance>;

export type FieldGuidanceId = keyof typeof FIELD_GUIDANCE;
