import { useState, useEffect } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api } from "../api";

export default function VerifyEmail() {
  const [params] = useSearchParams();
  const token = params.get("token") || "";
  const [status, setStatus] = useState<"loading" | "success" | "error">("loading");
  const [message, setMessage] = useState("");

  useEffect(() => {
    if (!token) {
      setStatus("error");
      setMessage("Invalid verification link.");
      return;
    }

    api
      .post<{ message: string }>("/auth/verify-email", { token })
      .then((res) => {
        setStatus("success");
        setMessage(res.message || "Email verified!");
      })
      .catch((err) => {
        setStatus("error");
        setMessage(err instanceof Error ? err.message : "Verification failed");
      });
  }, [token]);

  return (
    <div className="min-h-screen flex items-center justify-center bg-dark-950 px-4">
      <div className="w-full max-w-sm">
        <div className="text-center mb-8">
          <div className="inline-flex items-center justify-center w-14 h-14 bg-rust-500 rounded-xl mb-4">
            <svg className="w-8 h-8 text-white" viewBox="0 0 32 32" fill="currentColor">
              <rect x="4" y="4" width="10" height="10" rx="2" opacity="0.9" />
              <rect x="18" y="4" width="10" height="10" rx="2" opacity="0.7" />
              <rect x="4" y="18" width="10" height="10" rx="2" opacity="0.7" />
              <rect x="18" y="18" width="10" height="10" rx="2" opacity="0.5" />
            </svg>
          </div>
          <h1 className="text-base font-bold text-rust-500 uppercase font-mono tracking-widest">DockPanel</h1>
        </div>

        <div className="bg-dark-800 rounded-lg border border-dark-600 p-6 space-y-4">
          {status === "loading" && (
            <div className="text-center text-dark-200 py-4">Verifying your email...</div>
          )}
          {status === "success" && (
            <div className="bg-rust-50 text-rust-400 text-sm px-4 py-3 rounded-lg border border-rust-200">
              {message}
            </div>
          )}
          {status === "error" && (
            <div className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20">
              {message}
            </div>
          )}
          <Link
            to="/login"
            className="block w-full py-2.5 bg-rust-500 text-white rounded-lg font-medium hover:bg-rust-600 text-center text-sm"
          >
            Go to Login
          </Link>
        </div>
      </div>
    </div>
  );
}
