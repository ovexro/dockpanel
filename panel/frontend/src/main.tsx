import { StrictMode, Suspense, lazy } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Routes, Route, Navigate } from "react-router-dom";
import { AuthProvider } from "./context/AuthContext";
import Layout from "./components/Layout";
import Login from "./pages/Login";
import Setup from "./pages/Setup";
import Dashboard from "./pages/Dashboard";
import "./index.css";

const Register = lazy(() => import("./pages/Register"));
const ForgotPassword = lazy(() => import("./pages/ForgotPassword"));
const ResetPassword = lazy(() => import("./pages/ResetPassword"));
const VerifyEmail = lazy(() => import("./pages/VerifyEmail"));
const Servers = lazy(() => import("./pages/Servers"));

const Monitors = lazy(() => import("./pages/Monitors"));
const Teams = lazy(() => import("./pages/Teams"));

// Lazy-loaded pages (split into separate chunks)
const Sites = lazy(() => import("./pages/Sites"));
const SiteDetail = lazy(() => import("./pages/SiteDetail"));
const Databases = lazy(() => import("./pages/Databases"));
const Files = lazy(() => import("./pages/Files"));
const Terminal = lazy(() => import("./pages/Terminal"));
const Backups = lazy(() => import("./pages/Backups"));
const Crons = lazy(() => import("./pages/Crons"));
const Deploy = lazy(() => import("./pages/Deploy"));
const Dns = lazy(() => import("./pages/Dns"));
const WordPress = lazy(() => import("./pages/WordPress"));
const Logs = lazy(() => import("./pages/Logs"));
const Users = lazy(() => import("./pages/Users"));
const Apps = lazy(() => import("./pages/Apps"));
const Security = lazy(() => import("./pages/Security"));
const Diagnostics = lazy(() => import("./pages/Diagnostics"));
const Settings = lazy(() => import("./pages/Settings"));
const Updates = lazy(() => import("./pages/Updates"));
const Alerts = lazy(() => import("./pages/Alerts"));
const Activity = lazy(() => import("./pages/Activity"));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AuthProvider>
      <BrowserRouter>
        <Suspense fallback={<div className="flex items-center justify-center h-screen text-dark-300">Loading...</div>}>
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
              <Route path="/users" element={<Users />} />
              <Route path="/servers" element={<Servers />} />

              <Route path="/alerts" element={<Alerts />} />
              <Route path="/monitors" element={<Monitors />} />
              <Route path="/teams" element={<Teams />} />
              <Route path="/teams/accept" element={<Teams />} />
            </Route>
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </Suspense>
      </BrowserRouter>
    </AuthProvider>
  </StrictMode>
);
