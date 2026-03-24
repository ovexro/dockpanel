import { useState } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import UpdatesContent from "./Updates";

export default function System() {
  const { user } = useAuth();
  const [tab, setTab] = useState<"updates" | "health">("updates");

  if (!user || user.role !== "admin") return <Navigate to="/" replace />;

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">System</h1>
          <p className="text-sm text-dark-200 mt-1">System updates, services, and health monitoring</p>
        </div>
      </div>
      <div className="flex gap-6 mb-6 text-sm font-mono">
        <button onClick={() => setTab("updates")} className={tab === "updates" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Updates</button>
        <button onClick={() => setTab("health")} className={tab === "health" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Health</button>
      </div>
      {tab === "updates" && <UpdatesContent />}
      {tab === "health" && <HealthContent />}
    </div>
  );
}

function HealthContent() {
  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
      <p className="text-sm text-dark-300">System health information is available in Settings &rarr; General tab (health status widget).</p>
    </div>
  );
}
