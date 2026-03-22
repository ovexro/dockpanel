import { useState, useRef, useEffect, useCallback } from "react";
import { createPortal } from "react-dom";

const layouts = [
  { id: "command", label: "Sidebar", desc: "Full sidebar, grouped nav" },
  { id: "glass", label: "Compact", desc: "Collapsible icon sidebar" },
  { id: "atlas", label: "Topbar", desc: "Horizontal navbar, breadcrumbs" },
];

interface Props {
  variant?: "dark" | "light";
  compact?: boolean;
}

export default function LayoutSwitcher({ variant = "dark", compact = false }: Props) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ top: 0, left: 0, openDown: false });
  const current = localStorage.getItem("dp-layout") || "command";

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      const target = e.target as Node;
      // Ignore clicks inside the button or the portalled dropdown
      if (ref.current?.contains(target)) return;
      if (dropdownRef.current?.contains(target)) return;
      setOpen(false);
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, []);

  const toggle = useCallback(() => {
    if (!open && btnRef.current) {
      const rect = btnRef.current.getBoundingClientRect();
      const dropdownHeight = layouts.length * 52 + 8; // ~52px per item + padding
      const spaceAbove = rect.top;
      const spaceBelow = window.innerHeight - rect.bottom;

      // Open downward if near top of screen (e.g. in a header/navbar)
      const openDown = spaceAbove < dropdownHeight && spaceBelow > dropdownHeight;

      if (openDown) {
        setPos({ top: rect.bottom + 4, left: rect.left, openDown: true });
      } else {
        setPos({ top: rect.top - 4, left: rect.left, openDown: false });
      }
    }
    setOpen(!open);
  }, [open]);

  const switchLayout = (id: string) => {
    localStorage.setItem("dp-layout", id);
    document.documentElement.setAttribute("data-layout", id);
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
        className={`flex items-center gap-1.5 rounded-lg text-xs font-medium transition-colors text-dark-400 hover:text-dark-200 hover:bg-dark-600/15 ${
          compact ? "p-1.5" : "px-2 py-1.5"
        }`}
        title="Switch layout"
      >
        <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
          <rect x="3" y="3" width="18" height="18" rx="2" />
          <line x1="9" y1="3" x2="9" y2="21" />
          <line x1="9" y1="9" x2="21" y2="9" />
        </svg>
        {!compact && <span>{currentLabel}</span>}
        <svg className={`w-3 h-3 transition-transform ${open ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
        </svg>
      </button>

      {open && createPortal(
        <div
          ref={dropdownRef}
          className={`fixed w-52 rounded-lg shadow-2xl z-[9999] p-1 ${
            isDark ? "bg-dark-900 border border-dark-600" : "bg-dark-950 border border-dark-700 shadow-lg"
          }`}
          style={{
            top: pos.top,
            left: Math.min(pos.left, window.innerWidth - 220),
            transform: pos.openDown ? "none" : "translateY(-100%)",
          }}
        >
          {layouts.map(l => (
            <button
              key={l.id}
              onClick={() => switchLayout(l.id)}
              className={`w-full text-left px-3 py-2.5 rounded-md transition-colors ${
                l.id === current
                  ? isDark ? "bg-dark-800 text-dark-50" : "bg-rust-500/10 text-rust-600"
                  : isDark ? "text-dark-300 hover:bg-dark-800 hover:text-dark-100" : "text-dark-400 hover:bg-dark-800 hover:text-dark-50"
              }`}
            >
              <div className="text-sm font-medium">{l.label}</div>
              <div className={`text-xs mt-0.5 ${
                l.id === current
                  ? isDark ? "text-dark-400" : "text-rust-500/70"
                  : isDark ? "text-dark-500" : "text-dark-500"
              }`}>{l.desc}</div>
            </button>
          ))}
        </div>,
        document.body
      )}
    </div>
  );
}
