import { StrictMode, Suspense, lazy, Component, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Routes, Route, Navigate } from "react-router-dom";
import { AuthProvider } from "./context/AuthContext";
import Layout from "./components/Layout";
import Login from "./pages/Login";
import Setup from "./pages/Setup";
import Dashboard from "./pages/Dashboard";
import "./index.css";

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
const Monitors = lazyRetry(() => import("./pages/Monitors"));

// Lazy-loaded pages (split into separate chunks)
const Sites = lazyRetry(() => import("./pages/Sites"));
const SiteDetail = lazyRetry(() => import("./pages/SiteDetail"));
const Databases = lazyRetry(() => import("./pages/Databases"));
const Files = lazyRetry(() => import("./pages/Files"));
const Terminal = lazyRetry(() => import("./pages/Terminal"));
const Backups = lazyRetry(() => import("./pages/Backups"));
const Crons = lazyRetry(() => import("./pages/Crons"));
const Deploy = lazyRetry(() => import("./pages/Deploy"));
const Dns = lazyRetry(() => import("./pages/Dns"));
const WordPress = lazyRetry(() => import("./pages/WordPress"));
const Logs = lazyRetry(() => import("./pages/Logs"));
const Users = lazyRetry(() => import("./pages/Users"));
const Apps = lazyRetry(() => import("./pages/Apps"));
const Security = lazyRetry(() => import("./pages/Security"));
const Diagnostics = lazyRetry(() => import("./pages/Diagnostics"));
const Settings = lazyRetry(() => import("./pages/Settings"));
const Updates = lazyRetry(() => import("./pages/Updates"));
const Alerts = lazyRetry(() => import("./pages/Alerts"));
const Activity = lazyRetry(() => import("./pages/Activity"));
const Mail = lazyRetry(() => import("./pages/Mail"));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AuthProvider>
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
            <Route element={<Layout />}>
              <Route path="/" element={<Dashboard />} />
              <Route path="/sites" element={<Sites />} />
              <Route path="/sites/:id" element={<SiteDetail />} />
              <Route path="/sites/:id/files" element={<Files />} />
              <Route path="/sites/:id/backups" element={<Backups />} />
              <Route path="/sites/:id/crons" element={<Crons />} />
              <Route path="/sites/:id/deploy" element={<Deploy />} />
              <Route path="/sites/:id/wordpress" element={<WordPress />} />
              <Route path="/dns" element={<Dns />} />
              <Route path="/databases" element={<Databases />} />
              <Route path="/terminal" element={<Terminal />} />
              <Route path="/logs" element={<Logs />} />
              <Route path="/apps" element={<Apps />} />
              <Route path="/security" element={<Security />} />
              <Route path="/diagnostics" element={<Diagnostics />} />
              <Route path="/settings" element={<Settings />} />
              <Route path="/updates" element={<Updates />} />
              <Route path="/activity" element={<Activity />} />
              <Route path="/mail" element={<Mail />} />
              <Route path="/users" element={<Users />} />
              <Route path="/alerts" element={<Alerts />} />
              <Route path="/monitors" element={<Monitors />} />
            </Route>
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </Suspense>
        </ChunkErrorBoundary>
      </BrowserRouter>
    </AuthProvider>
  </StrictMode>
);
