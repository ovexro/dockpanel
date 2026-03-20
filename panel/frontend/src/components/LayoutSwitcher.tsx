import { useState, useRef, useEffect, useCallback } from "react";

const layouts = [
  { id: "command", label: "Terminal", desc: "Dark, CLI aesthetic" },
  { id: "glass", label: "Glass", desc: "Collapsible sidebar" },
  { id: "atlas", label: "Atlas", desc: "Top navbar, breadcrumbs" },
  { id: "nexus", label: "Nexus", desc: "Light, clean SaaS" },
];

interface Props {
  variant?: "dark" | "light";
}

export default function LayoutSwitcher({ variant = "dark" }: Props) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const [pos, setPos] = useState({ top: 0, left: 0 });
  const current = localStorage.getItem("dp-layout") || "command";

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, []);

  const toggle = useCallback(() => {
    if (!open && btnRef.current) {
      const rect = btnRef.current.getBoundingClientRect();
      // Open above the button
      setPos({ top: rect.top - 4, left: rect.left });
    }
    setOpen(!open);
  }, [open]);

  const switchLayout = (id: string) => {
    localStorage.setItem("dp-layout", id);
    window.dispatchEvent(new Event("dp-layout-change"));
    setOpen(false);
  };

  const isDark = variant === "dark";
  const currentLabel = layouts.find(l => l.id === current)?.label || "Layout";

  return (
    <div ref={ref}>
      <button
        ref={btnRef}
        onClick={toggle}
        className={`flex items-center gap-1.5 px-2 py-1.5 rounded text-xs font-medium transition-colors ${
          isDark
            ? "bg-dark-800 border border-dark-600/40 text-dark-300 hover:text-dark-50"
            : "bg-white border border-zinc-200 text-zinc-600 hover:text-zinc-900"
        }`}
        title="Switch layout"
      >
        <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
          <rect x="3" y="3" width="7" height="7" rx="1" />
          <rect x="14" y="3" width="7" height="7" rx="1" />
          <rect x="3" y="14" width="7" height="7" rx="1" />
          <rect x="14" y="14" width="7" height="7" rx="1" />
        </svg>
        <span>{currentLabel}</span>
        <svg className={`w-3 h-3 transition-transform ${open ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
        </svg>
      </button>

      {open && (
        <div
          className={`fixed w-52 rounded-lg shadow-2xl overflow-hidden z-[9999] ${
            isDark ? "bg-dark-900 border border-dark-600" : "bg-white border border-zinc-200"
          }`}
          style={{ top: pos.top, left: pos.left, transform: "translateY(-100%)" }}
        >
          {layouts.map(l => (
            <button
              key={l.id}
              onClick={() => switchLayout(l.id)}
              className={`w-full text-left px-3 py-2.5 transition-colors ${
                l.id === current
                  ? isDark ? "bg-dark-800 text-dark-50" : "bg-indigo-50 text-indigo-700"
                  : isDark ? "text-dark-300 hover:bg-dark-800 hover:text-dark-100" : "text-zinc-600 hover:bg-zinc-50 hover:text-zinc-900"
              }`}
            >
              <div className="text-sm font-medium">{l.label}</div>
              <div className={`text-xs mt-0.5 ${isDark ? "text-dark-500" : "text-zinc-400"}`}>{l.desc}</div>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
