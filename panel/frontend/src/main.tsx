import { StrictMode, Suspense, lazy, Component, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Routes, Route, Navigate } from "react-router-dom";
import { AuthProvider } from "./context/AuthContext";
import { ServerProvider } from "./context/ServerContext";
import { BrandingProvider } from "./context/BrandingContext";
import LayoutShell from "./components/LayoutShell";
import Login from "./pages/Login";
import Setup from "./pages/Setup";
import Dashboard from "./pages/Dashboard";
import "./index.css";

// Theme initialization — runs before first paint, migrates old values
(() => {
  const stored = localStorage.getItem("dp-theme");
  let theme = stored || "midnight";
  if (theme === "dark") theme = "midnight";
  if (theme === "light") theme = "arctic";
  if (theme === "nexus") theme = "clean";
  if (theme === "nexus-dark") theme = "clean-dark";
  // Layout initialization
  const layout = localStorage.getItem("dp-layout") || "command";
  localStorage.setItem("dp-theme", theme);
  document.documentElement.setAttribute("data-theme", theme);
  document.documentElement.setAttribute("data-layout", layout);
  document.documentElement.setAttribute("data-color-scheme", (theme === "clean" || theme === "arctic") ? "light" : "dark");
})();

// Retry lazy import — handles stale chunks after deploy
function lazyRetry(importFn: () => Promise<{ default: React.ComponentType }>) {
  return lazy(() =>
    importFn().catch(() => {
      // Chunk failed to load (likely stale hash after deploy) — reload once
      const key = "chunk_reload";
      if (!sessionStorage.getItem(key)) {
        sessionStorage.setItem(key, "1");
        window.location.reload();
      }
      return importFn(); // return promise anyway to satisfy types
    })
  );
}

// Error boundary — catches chunk load failures and shows reload button
class ChunkErrorBoundary extends Component<{ children: ReactNode }, { hasError: boolean }> {
  constructor(props: { children: ReactNode }) {
    super(props);
    this.state = { hasError: false };
  }
  static getDerivedStateFromError() {
    return { hasError: true };
  }
  render() {
    if (this.state.hasError) {
      return (
        <div className="flex flex-col items-center justify-center h-screen gap-4 bg-dark-900">
          <p className="text-dark-200 text-sm font-mono">Page failed to load — a new version may have been deployed.</p>
          <button
            onClick={() => window.location.reload()}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600"
          >
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

const Register = lazyRetry(() => import("./pages/Register"));
const ForgotPassword = lazyRetry(() => import("./pages/ForgotPassword"));
const ResetPassword = lazyRetry(() => import("./pages/ResetPassword"));
const VerifyEmail = lazyRetry(() => import("./pages/VerifyEmail"));
const Monitoring = lazyRetry(() => import("./pages/Monitoring"));

// Lazy-loaded pages (split into separate chunks)
const Sites = lazyRetry(() => import("./pages/Sites"));
const SiteDetail = lazyRetry(() => import("./pages/SiteDetail"));
const Databases = lazyRetry(() => import("./pages/Databases"));
const Files = lazyRetry(() => import("./pages/Files"));
const Terminal = lazyRetry(() => import("./pages/Terminal"));
const Backups = lazyRetry(() => import("./pages/Backups"));
const Crons = lazyRetry(() => import("./pages/Crons"));
const Deploy = lazyRetry(() => import("./pages/Deploy"));
const GitDeploys = lazyRetry(() => import("./pages/GitDeploys"));
const Cdn = lazyRetry(() => import("./pages/Cdn"));
const Dns = lazyRetry(() => import("./pages/Dns"));
const WordPress = lazyRetry(() => import("./pages/WordPress"));
const WordPressToolkit = lazyRetry(() => import("./pages/WordPressToolkit"));
const Logs = lazyRetry(() => import("./pages/Logs"));
const Apps = lazyRetry(() => import("./pages/Apps"));
const Extensions = lazyRetry(() => import("./pages/Extensions"));
const Security = lazyRetry(() => import("./pages/Security"));
const Settings = lazyRetry(() => import("./pages/Settings"));
const Mail = lazyRetry(() => import("./pages/Mail"));
const Servers = lazyRetry(() => import("./pages/Servers"));
const ResellerDashboard = lazyRetry(() => import("./pages/ResellerDashboard"));
const MigrationWizard = lazyRetry(() => import("./pages/Migration"));
const ResellerUsers = lazyRetry(() => import("./pages/ResellerUsers"));
const BackupOrchestrator = lazyRetry(() => import("./pages/BackupOrchestrator"));
const PublicStatusPage = lazyRetry(() => import("./pages/PublicStatusPage"));
const SecretsManager = lazyRetry(() => import("./pages/SecretsManager"));
const Notifications = lazyRetry(() => import("./pages/Notifications"));
const Integrations = lazyRetry(() => import("./pages/Integrations"));
const Users = lazyRetry(() => import("./pages/Users"));
const ContainerPolicies = lazyRetry(() => import("./pages/ContainerPolicies"));
const System = lazyRetry(() => import("./pages/System"));
const Telemetry = lazyRetry(() => import("./pages/Telemetry"));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrandingProvider>
    <AuthProvider>
      <ServerProvider>
      <BrowserRouter>
        <ChunkErrorBoundary>
        <Suspense fallback={<div className="flex items-center justify-center h-screen bg-dark-900"><div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" /></div>}>
          <Routes>
            <Route path="/login" element={<Login />} />
            <Route path="/setup" element={<Setup />} />
            <Route path="/register" element={<Register />} />
            <Route path="/forgot-password" element={<ForgotPassword />} />
            <Route path="/reset-password" element={<ResetPassword />} />
            <Route path="/verify-email" element={<VerifyEmail />} />
            <Route element={<LayoutShell />}>
              <Route path="/" element={<Dashboard />} />
              <Route path="/sites" element={<Sites />} />
              <Route path="/sites/:id" element={<SiteDetail />} />
              <Route path="/sites/:id/files" element={<Files />} />
              <Route path="/sites/:id/backups" element={<Backups />} />
              <Route path="/sites/:id/crons" element={<Crons />} />
              <Route path="/sites/:id/deploy" element={<Deploy />} />
              <Route path="/sites/:id/wordpress" element={<WordPress />} />
              <Route path="/wordpress-toolkit" element={<WordPressToolkit />} />
              <Route path="/cdn" element={<Cdn />} />
              <Route path="/dns" element={<Dns />} />
              <Route path="/databases" element={<Databases />} />
              <Route path="/terminal" element={<Terminal />} />
              <Route path="/logs" element={<Logs />} />
              <Route path="/apps" element={<Apps />} />
              <Route path="/git-deploys" element={<GitDeploys />} />
              <Route path="/extensions" element={<Navigate to="/integrations" replace />} />
              <Route path="/security" element={<Security />} />
              <Route path="/diagnostics" element={<Navigate to="/security" replace />} />
              <Route path="/settings" element={<Settings />} />
              <Route path="/updates" element={<Navigate to="/system" replace />} />
              <Route path="/activity" element={<Navigate to="/logs" replace />} />
              <Route path="/system-logs" element={<Navigate to="/logs" replace />} />
              <Route path="/mail" element={<Mail />} />
              <Route path="/servers" element={<Servers />} />
              <Route path="/migration" element={<MigrationWizard />} />
              <Route path="/reseller" element={<ResellerDashboard />} />
              <Route path="/reseller/users" element={<ResellerUsers />} />
              <Route path="/users" element={<Users />} />
              <Route path="/container-policies" element={<ContainerPolicies />} />
              <Route path="/backup-orchestrator" element={<BackupOrchestrator />} />
              <Route path="/incidents" element={<Navigate to="/monitoring" replace />} />
              <Route path="/secrets" element={<SecretsManager />} />
              <Route path="/integrations" element={<Integrations />} />
              <Route path="/webhooks" element={<Navigate to="/integrations" replace />} />
              <Route path="/notifications" element={<Notifications />} />
              <Route path="/monitoring" element={<Monitoring />} />
              <Route path="/system" element={<System />} />
              <Route path="/telemetry" element={<Telemetry />} />
              <Route path="/security-hardening" element={<Navigate to="/security" replace />} />
              <Route path="/monitors" element={<Navigate to="/monitoring" replace />} />
              <Route path="/alerts" element={<Navigate to="/monitoring" replace />} />
            </Route>
            <Route path="/status" element={<PublicStatusPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </Suspense>
        </ChunkErrorBoundary>
      </BrowserRouter>
      </ServerProvider>
    </AuthProvider>
    </BrandingProvider>
  </StrictMode>
);
