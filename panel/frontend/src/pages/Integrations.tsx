import { useState } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import WebhookGatewayContent from "./WebhookGateway";
import ExtensionsContent from "./Extensions";

export default function Integrations() {
  const { user } = useAuth();
  const [tab, setTab] = useState<"webhooks" | "extensions">("webhooks");

  if (!user || user.role !== "admin") return <Navigate to="/" replace />;

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Integrations</h1>
          <p className="text-sm text-dark-200 mt-1">Webhooks, extensions, and external service connections</p>
        </div>
      </div>
      <div className="flex gap-6 mb-6 text-sm font-mono">
        <button onClick={() => setTab("webhooks")} className={tab === "webhooks" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Webhooks</button>
        <button onClick={() => setTab("extensions")} className={tab === "extensions" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Extensions</button>
      </div>
      {tab === "webhooks" && <WebhookGatewayContent />}
      {tab === "extensions" && <ExtensionsContent />}
    </div>
  );
}
