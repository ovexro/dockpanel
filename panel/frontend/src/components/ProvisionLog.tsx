import { useState, useEffect } from "react";

interface ProvisionStep {
  step: string;
  label: string;
  status: "pending" | "in_progress" | "done" | "error";
  message?: string;
}

interface Props {
  siteId?: string;
  sseUrl?: string;
  onComplete?: () => void;
}

export default function ProvisionLog({ siteId, sseUrl, onComplete }: Props) {
  const [steps, setSteps] = useState<ProvisionStep[]>([]);
  const [done, setDone] = useState(false);

  const url = sseUrl || (siteId ? `/api/sites/${siteId}/provision-log` : "");

  useEffect(() => {
    if (!url) return;
    const es = new EventSource(url);

    es.onmessage = (event) => {
      try {
        const step: ProvisionStep = JSON.parse(event.data);
        setSteps((prev) => {
          const idx = prev.findIndex((s) => s.step === step.step);
          if (idx >= 0) {
            const next = [...prev];
            next[idx] = step;
            return next;
          }
          return [...prev, step];
        });
        if (step.step === "complete") {
          es.close();
          setDone(true);
          setTimeout(() => onComplete?.(), 2000);
        }
      } catch {
        // ignore parse errors
      }
    };

    es.onerror = () => {
      es.close();
      setSteps((prev) => {
        if (prev.length === 0) { setDone(true); onComplete?.(); }
        return prev;
      });
    };

    return () => es.close();
  }, [url]);

  // Don't render the "complete" pseudo-step
  const visibleSteps = steps.filter((s) => s.step !== "complete");

  const hasError = steps.some((s) => s.step === "complete" && s.status === "error");
  const isComplete = done && !hasError;

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6 animate-fade-up">
      <div className="flex items-center gap-2 mb-4">
        {!done ? (
          <svg className="w-4 h-4 text-rust-400 animate-spin" fill="none" viewBox="0 0 24 24">
            <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
            <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
          </svg>
        ) : isComplete ? (
          <svg className="w-4 h-4 text-rust-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2.5">
            <path strokeLinecap="round" strokeLinejoin="round" d="m4.5 12.75 6 6 9-13.5" />
          </svg>
        ) : (
          <svg className="w-4 h-4 text-danger-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="2.5">
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
          </svg>
        )}
        <span className="text-sm font-medium text-dark-50 font-mono tracking-wide">
          {!done ? "Provisioning..." : isComplete ? "Provisioning complete" : "Provisioning failed"}
        </span>
      </div>

      <div className="space-y-1">
        {visibleSteps.map((step) => (
          <div key={step.step} className="flex items-start gap-3 py-1.5">
            {/* Status icon */}
            <div className="mt-0.5 flex-shrink-0">
              {step.status === "in_progress" ? (
                <svg className="w-3.5 h-3.5 text-warn-400 animate-spin" fill="none" viewBox="0 0 24 24">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                </svg>
              ) : step.status === "done" ? (
                <svg className="w-3.5 h-3.5 text-rust-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="3">
                  <path strokeLinecap="round" strokeLinejoin="round" d="m4.5 12.75 6 6 9-13.5" />
                </svg>
              ) : step.status === "error" ? (
                <svg className="w-3.5 h-3.5 text-danger-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="3">
                  <path strokeLinecap="round" strokeLinejoin="round" d="M6 18 18 6M6 6l12 12" />
                </svg>
              ) : (
                <div className="w-3.5 h-3.5 rounded-full border-2 border-dark-500" />
              )}
            </div>

            {/* Label + message */}
            <div className="min-w-0">
              <span className={`text-sm font-mono ${
                step.status === "done" ? "text-dark-200" :
                step.status === "in_progress" ? "text-dark-50" :
                step.status === "error" ? "text-danger-400" :
                "text-dark-300"
              }`}>
                {step.label}
              </span>
              {step.message && (
                <p className="text-xs text-dark-300 mt-0.5 truncate">{step.message}</p>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
