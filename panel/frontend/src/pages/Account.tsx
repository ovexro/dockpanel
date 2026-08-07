import { useAuth } from "../context/AuthContext";
import {
  TwoFactorCard,
  PasskeysCard,
  ChangePasswordCard,
  SessionsCard,
  ApiKeysCard,
  ExportMyDataCard,
} from "../components/AccountSecurity";

/**
 * A person's own account, reachable by every role.
 *
 * Until this page existed, all of it lived on `/settings`, whose nav row is
 * `adminOnly`, so a `client`, `user` or `reseller` had no way to change their own
 * password or enrol in 2FA — while the layout's own banner told them 2FA was
 * required and linked them to that same admin page. The endpoints were never the
 * obstacle: every one is `AuthUser` and scoped to the caller's own id.
 *
 * Deliberately NOT `adminOnly` in `data/navItems.ts`. If a future change makes it
 * admin-only again, the 2FA banner in all four layouts becomes a dead end for the
 * exact population it is aimed at.
 */
export default function Account() {
  const { user } = useAuth();

  return (
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-header-title">My Account</h1>
          <p className="page-header-subtitle">
            {user?.email || "Your sign-in, security and API access"}
            {user?.role && (
              <span className="ml-2 px-1.5 py-0.5 bg-dark-700 border border-dark-600 rounded text-[10px] font-medium uppercase tracking-wider text-dark-300">
                {user.role}
              </span>
            )}
          </p>
        </div>
      </div>

      <div className="p-6 lg:p-8">
        <div className="space-y-6">
          <TwoFactorCard />
          <PasskeysCard />
          <ChangePasswordCard />
          <SessionsCard />
          <ApiKeysCard />
          <ExportMyDataCard />
        </div>
      </div>
    </div>
  );
}
