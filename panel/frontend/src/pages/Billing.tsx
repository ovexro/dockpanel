import { useState, useEffect } from "react";
import { api } from "../api";

interface PlanInfo {
  plan: string;
  plan_status: string;
  server_limit: number;
  server_count: number;
  billing_enabled: boolean;
}

const plans = [
  {
    id: "starter",
    name: "Starter",
    price: "$9/mo",
    servers: 1,
    features: ["1 server", "Unlimited sites", "SSL management", "File manager", "Backups"],
  },
  {
    id: "pro",
    name: "Pro",
    price: "$19/mo",
    servers: 5,
    features: ["5 servers", "Unlimited sites", "SSL management", "File manager", "Backups", "Staging environments", "Priority support"],
  },
  {
    id: "agency",
    name: "Agency",
    price: "$39/mo",
    servers: 20,
    features: ["20 servers", "Unlimited sites", "SSL management", "File manager", "Backups", "Staging environments", "Priority support", "Team access"],
  },
];

const statusColors: Record<string, string> = {
  active: "bg-rust-500/15 text-rust-400",
  grace: "bg-warn-500/15 text-warn-400",
  suspended: "bg-red-500/15 text-red-400",
};

export default function Billing() {
  const [info, setInfo] = useState<PlanInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [checkoutLoading, setCheckoutLoading] = useState("");
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    if (params.get("success") === "1") {
      setSuccess("Subscription activated! Your plan has been updated.");
      window.history.replaceState({}, "", "/billing");
    } else if (params.get("canceled") === "1") {
      setError("Checkout canceled. No changes were made.");
      window.history.replaceState({}, "", "/billing");
    }

    api.get<PlanInfo>("/billing/plan")
      .then(setInfo)
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, []);

  const handleCheckout = async (plan: string) => {
    setError("");
    setCheckoutLoading(plan);
    try {
      const res = await api.post<{ url: string }>("/billing/checkout", { plan });
      window.location.href = res.url;
    } catch (err) {
      setError(err instanceof Error ? err.message : "Checkout failed");
      setCheckoutLoading("");
    }
  };

  const handlePortal = async () => {
    setError("");
    try {
      const res = await api.post<{ url: string }>("/billing/portal", {});
      window.location.href = res.url;
    } catch (err) {
      setError(err instanceof Error ? err.message : "Portal access failed");
    }
  };

  if (loading) {
    return (
      <div className="p-6 lg:p-8">
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8">
      <div className="mb-6">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Billing</h1>
        <p className="text-sm text-dark-200 font-mono mt-1">Manage your subscription and plan</p>
      </div>

      {error && (
        <div className="bg-red-500/10 text-red-400 text-sm px-4 py-3 rounded-lg border border-red-500/20 mb-4">
          {error}
        </div>
      )}
      {success && (
        <div className="bg-rust-50 text-rust-400 text-sm px-4 py-3 rounded-lg border border-rust-200 mb-4">
          {success}
        </div>
      )}

      {/* Current Plan */}
      {info && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6">
          <div className="flex items-center justify-between">
            <div>
              <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-2">Current Plan</h2>
              <div className="flex items-center gap-3">
                <span className="text-lg font-bold text-dark-50 capitalize">{info.plan}</span>
                <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${statusColors[info.plan_status] || "bg-dark-700 text-dark-200"}`}>
                  {info.plan_status}
                </span>
              </div>
              <p className="text-sm text-dark-200 mt-1">
                {info.server_count} / {info.server_limit} server{info.server_limit !== 1 ? "s" : ""} used
              </p>
              {/* Usage bar */}
              <div className="mt-2 w-48 h-2 bg-dark-700 rounded-full overflow-hidden">
                <div
                  className={`h-full rounded-full transition-all ${
                    info.server_count >= info.server_limit ? "bg-red-500" : "bg-rust-400"
                  }`}
                  style={{ width: `${Math.min((info.server_count / info.server_limit) * 100, 100)}%` }}
                />
              </div>
            </div>
            {info.plan !== "free" && info.billing_enabled && (
              <button
                onClick={handlePortal}
                className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-gray-200 transition-colors"
              >
                Manage Subscription
              </button>
            )}
          </div>
          {info.plan_status === "grace" && (
            <div className="mt-3 bg-warn-500/10 text-warn-400 text-sm px-4 py-2 rounded-lg border border-warn-400/20">
              Your last payment failed. Please update your payment method to avoid service interruption.
            </div>
          )}
          {info.plan_status === "suspended" && (
            <div className="mt-3 bg-red-500/10 text-red-400 text-sm px-4 py-2 rounded-lg border border-red-500/20">
              Your subscription is suspended. Please update your payment to restore access.
            </div>
          )}
        </div>
      )}

      {/* Plan Cards */}
      {info?.billing_enabled && (
        <div>
          <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-4">
            {info.plan === "free" ? "Choose a Plan" : "Change Plan"}
          </h2>
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            {plans.map((p) => {
              const isCurrent = info.plan === p.id;
              return (
                <div
                  key={p.id}
                  className={`bg-dark-800 rounded-lg border-2 p-5 ${
                    isCurrent ? "border-rust-500 ring-1 ring-rust-500" : "border-dark-500"
                  }`}
                >
                  <div className="mb-4">
                    <h3 className="text-lg font-bold text-dark-50">{p.name}</h3>
                    <p className="text-2xl font-bold text-rust-500 mt-1">{p.price}</p>
                  </div>
                  <ul className="space-y-2 mb-6">
                    {p.features.map((f) => (
                      <li key={f} className="flex items-center gap-2 text-sm text-dark-200">
                        <svg className="w-4 h-4 text-rust-500 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="m4.5 12.75 6 6 9-13.5" />
                        </svg>
                        {f}
                      </li>
                    ))}
                  </ul>
                  {isCurrent ? (
                    <div className="w-full px-4 py-2 bg-rust-500/10 text-rust-400 rounded-lg text-sm font-medium text-center">
                      Current Plan
                    </div>
                  ) : (
                    <button
                      onClick={() => handleCheckout(p.id)}
                      disabled={checkoutLoading === p.id}
                      className="w-full px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
                    >
                      {checkoutLoading === p.id ? "Redirecting..." : info.plan === "free" ? "Subscribe" : "Switch Plan"}
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {!info?.billing_enabled && (
        <div className="bg-dark-900 rounded-lg border border-dark-500 p-8 text-center">
          <svg className="w-12 h-12 text-gray-300 mx-auto mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M2.25 8.25h19.5M2.25 9h19.5m-16.5 5.25h6m-6 2.25h3m-3.75 3h15a2.25 2.25 0 0 0 2.25-2.25V6.75A2.25 2.25 0 0 0 19.5 4.5h-15a2.25 2.25 0 0 0-2.25 2.25v10.5A2.25 2.25 0 0 0 4.5 19.5Z" />
          </svg>
          <p className="text-dark-200 text-sm">
            Billing is not configured. Set <code className="bg-dark-700 px-1.5 py-0.5 rounded text-xs font-mono">STRIPE_SECRET_KEY</code> to enable subscriptions.
          </p>
        </div>
      )}
    </div>
  );
}
