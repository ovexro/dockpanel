import { useState } from "react";
import MonitorsContent from "./Monitors";
import AlertsContent from "./Alerts";
import CertificatesContent from "./Certificates";
import MaintenanceContent from "./Maintenance";

export default function Monitoring() {
  const [tab, setTab] = useState<"monitors" | "alerts" | "certificates" | "maintenance">("monitors");

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Monitoring</h1>
          <p className="text-sm text-dark-200 mt-1">Uptime monitors, certificates, and alert management</p>
        </div>
      </div>
      <div className="flex gap-6 mb-6 text-sm font-mono">
        <button onClick={() => setTab("monitors")} className={tab === "monitors" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Monitors</button>
        <button onClick={() => setTab("alerts")} className={tab === "alerts" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Alerts</button>
        <button onClick={() => setTab("certificates")} className={tab === "certificates" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Certificates</button>
        <button onClick={() => setTab("maintenance")} className={tab === "maintenance" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Maintenance</button>
      </div>
      {tab === "monitors" && <MonitorsContent />}
      {tab === "alerts" && <AlertsContent />}
      {tab === "certificates" && <CertificatesContent />}
      {tab === "maintenance" && <MaintenanceContent />}
    </div>
  );
}
