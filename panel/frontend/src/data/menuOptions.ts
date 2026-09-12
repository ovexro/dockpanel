import { useEffect, useState } from "react";
import { api } from "../api";
import { navGroups, type NavGroup } from "./navItems";

/**
 * Which menu entries a role is shown.
 *
 * Set by an administrator, per role, and stored panel-side in the `settings`
 * table under `menu_hidden_{role}` — so it applies to everyone with that role on
 * every browser they use, not to whoever happened to tick a box.
 *
 * This is a PREFERENCE, not a permission. Hiding a row removes the door, not
 * what is behind it: the route still resolves, the URL still works, and the
 * handler still applies whatever role check it always did. `isNavVisible` in
 * `navItems.ts` answers the other question — may this account be OFFERED this
 * page — and the two are deliberately kept apart. Folding decluttering into the
 * permission predicate would make a checkbox in Settings look like a security
 * control, and would invite someone to hide a row INSTEAD of restricting it.
 */

/**
 * Fired after an administrator saves, so every mounted menu re-reads.
 *
 * Same bus pattern as `THEME_CHANGE_EVENT`. It only reaches the tab that saved:
 * other sessions pick the change up on their next load, which is the right
 * cadence for a setting an operator changes once.
 */
export const MENU_CHANGE_EVENT = "dp-menu-change";

/** The roles an administrator may configure. */
export const CONFIGURABLE_ROLES = ["admin", "reseller", "user", "client"] as const;
export type ConfigurableRole = (typeof CONFIGURABLE_ROLES)[number];

/**
 * One administrator's own menu, overriding the stored `admin` default.
 *
 * The admin row is the only one that is a DEFAULT rather than a decision. An
 * operator debugging a box has to be able to get every door back without
 * another account's help — there is no account above an admin to restore a row
 * they can no longer find — so an admin may set their own view here and a
 * non-admin may not.
 *
 * Per browser, beside `dp-theme` and `dp-layout`, because it is the same kind of
 * choice: what THIS person wants to look at, not what the panel is configured to
 * be. Absent means "follow the stored default", which is why it is removed
 * rather than emptied when an admin goes back to it — an empty array is a real
 * answer meaning "hide nothing".
 */
export const ADMIN_OVERRIDE_KEY = "dp-menu-admin-override";

/** The calling admin's own menu, or null when they follow the default. */
export function readAdminOverride(): Set<string> | null {
  try {
    const raw = localStorage.getItem(ADMIN_OVERRIDE_KEY);
    if (raw === null) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return null;
    return new Set(parsed.filter((v): v is string => typeof v === "string" && !isAlwaysVisible(v)));
  } catch {
    return null;
  }
}

/** Set this admin's own menu, or pass null to go back to the stored default. */
export function writeAdminOverride(hidden: Set<string> | null): void {
  try {
    if (hidden === null) localStorage.removeItem(ADMIN_OVERRIDE_KEY);
    else localStorage.setItem(ADMIN_OVERRIDE_KEY, JSON.stringify([...hidden].filter(k => !isAlwaysVisible(k))));
  } catch {
    // Storage full or blocked. The event below still fires, so the change
    // applies for this session even when it cannot be remembered.
  }
  window.dispatchEvent(new Event(MENU_CHANGE_EVENT));
}

/**
 * The rows no checkbox may ever turn off.
 *
 * Dashboard is where every layout's logo links and where a refused route lands,
 * so losing it strands the account. Settings is the only way BACK to these
 * checkboxes. My Account is the sole entry every role can see (see the comment
 * on that row in `navItems.ts`) and the 2FA banner in all three layouts points
 * at it — hiding it from a `client` would leave the panel nagging them to enrol
 * with no screen on which to do it.
 */
export const ALWAYS_VISIBLE_PATHS: readonly string[] = ["/", "/settings", "/account"];

export function isAlwaysVisible(path: string): boolean {
  return ALWAYS_VISIBLE_PATHS.includes(path);
}

/**
 * The top-bar controls, which have no registry of their own.
 *
 * `navGroups` covers the side menu because every row there is a route. These are
 * not routes — they are the chrome each layout draws around the outlet — so they
 * are enumerated here and keyed by name. The keys are prefixed `chrome:` so one
 * flat stored array can hold both kinds without a control name ever colliding
 * with a path.
 *
 * The signed-in username and its Logout button are deliberately absent: an
 * account must always be able to see who it is signed in as, and sign out.
 */
export interface ChromeItem {
  key: string;
  label: string;
  hint: string;
}

export const CHROME_ITEMS: readonly ChromeItem[] = [
  { key: "chrome:search", label: "Search box", hint: "The Ctrl-K search shortcut. The keyboard shortcut itself keeps working." },
  { key: "chrome:notifications", label: "Notifications bell", hint: "The bell and its unread badge." },
  { key: "chrome:serverSelector", label: "Server selector", hint: "The fleet server picker above the navigation." },
  { key: "chrome:layoutSwitcher", label: "Layout switcher", hint: "The control that changes between the Command, Glass and Atlas layouts." },
  { key: "chrome:themeToggle", label: "Theme toggle", hint: "The button that cycles the colour theme." },
  { key: "chrome:health", label: "Connection status", hint: "The host-health dot and its Connected / Disconnected label." },
];

/** What the panel stores for this account's role, before any admin override. */
export interface MenuDefault {
  role: string;
  hidden: Set<string>;
}

// One fetch per session, shared by the three layouts, the command palette and
// the dashboard. Without the cache each of those mounts its own request for the
// same tiny answer, and the sidebar would re-request it on every layout switch.
let cache: MenuDefault | null = null;
let inflight: Promise<MenuDefault> | null = null;

/**
 * The stored menu for the CURRENT account's role, from `GET /api/menu-options`.
 *
 * Never rejects. A failure — offline, a 500, a logged-out 401 — resolves to an
 * empty set, because every caller is a menu about to render and a cluttered
 * menu is a better failure than an empty one.
 */
export function loadMenuDefault(force = false): Promise<MenuDefault> {
  if (force) {
    cache = null;
    inflight = null;
  }
  if (cache) return Promise.resolve(cache);
  if (inflight) return inflight;

  inflight = api
    .get<{ role?: string; hidden?: string[] }>("/menu-options")
    .then(r => {
      const hidden = Array.isArray(r?.hidden) ? r.hidden : [];
      // An always-visible path that somehow reached the store is dropped on READ
      // as well as on write: a value written by an older build, or edited into
      // the table by hand, must not be able to strand an account.
      cache = {
        role: typeof r?.role === "string" ? r.role : "",
        hidden: new Set(hidden.filter(k => typeof k === "string" && !isAlwaysVisible(k))),
      };
      return cache;
    })
    .catch(() => {
      cache = { role: "", hidden: new Set<string>() };
      return cache;
    })
    .finally(() => {
      inflight = null;
    });

  return inflight;
}

/** Drop the cached answer. Called after an admin saves, and on logout, so the
 *  next account does not inherit the previous one's menu. */
export function invalidateHiddenMenu(): void {
  cache = null;
  inflight = null;
  window.dispatchEvent(new Event(MENU_CHANGE_EVENT));
}

/**
 * Resolve what this account actually sees: the stored default for its role,
 * replaced wholesale by this admin's own choice when they have made one.
 *
 * The override is honoured ONLY for an admin, and the role comes from the server
 * rather than from localStorage, so a stale or hand-edited `dp-menu-admin-
 * override` cannot reshape a `user`'s menu. That matters less than it sounds —
 * none of this is a permission — but a setting an administrator made for a role
 * should not be quietly undone by something in that person's own browser.
 */
export function resolveHiddenMenu(base: MenuDefault): Set<string> {
  if (base.role !== "admin") return base.hidden;
  return readAdminOverride() ?? base.hidden;
}

/**
 * Subscribe to the hidden set for this account.
 *
 * Starts empty and fills in when the fetch lands, so a menu renders complete and
 * then settles rather than blocking on a request. Only rows this account is
 * permitted to see are ever in that first paint — the role filter in
 * `isNavVisible` is synchronous and does not wait for this.
 */
export function useHiddenMenu(): Set<string> {
  const [hidden, setHidden] = useState<Set<string>>(() => (cache ? resolveHiddenMenu(cache) : new Set()));

  useEffect(() => {
    let alive = true;
    const load = () => {
      loadMenuDefault().then(next => {
        if (alive) setHidden(resolveHiddenMenu(next));
      });
    };
    load();
    window.addEventListener(MENU_CHANGE_EVENT, load);
    // An admin editing their own view in another tab.
    window.addEventListener("storage", load);
    return () => {
      alive = false;
      window.removeEventListener(MENU_CHANGE_EVENT, load);
      window.removeEventListener("storage", load);
    };
  }, []);

  return hidden;
}

/**
 * Is this menu entry hidden? `key` is a nav path or a `chrome:` control name.
 *
 * The always-visible check lives here as well as at the read, so a row cannot be
 * hidden by any route into this module.
 */
export function isMenuHidden(key: string, hidden: Set<string>): boolean {
  if (isAlwaysVisible(key)) return false;
  return hidden.has(key);
}

/**
 * The nav rows a checkbox may turn off, in the order the sidebar draws them.
 *
 * Derived from `navGroups` rather than restated, so a row added to the registry
 * gets a checkbox automatically. A hardcoded second list is precisely how the
 * command palette came to offer pages the sidebar did not.
 *
 * Not filtered by role. An administrator configuring the `client` menu is
 * choosing from every row that exists, and `isNavVisible` still removes the ones
 * that role could never reach — so a box ticked here for a page a client cannot
 * open changes nothing, rather than being a trap.
 */
export function toggleableNavGroups(): NavGroup[] {
  return navGroups
    .map(g => ({ ...g, items: g.items.filter(item => !isAlwaysVisible(item.to)) }))
    .filter(g => g.items.length > 0);
}
