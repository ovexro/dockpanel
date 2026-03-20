import { useState, useEffect, lazy, Suspense } from "react";
import CommandLayout from "./CommandLayout";

const GlassLayout = lazy(() => import("./GlassLayout"));
const AtlasLayout = lazy(() => import("./AtlasLayout"));
const NexusLayout = lazy(() => import("./NexusLayout"));

const fallback = (
  <div className="flex items-center justify-center h-screen bg-dark-900">
    <div className="w-8 h-8 border-4 border-rust-500 border-t-transparent rounded-full animate-spin" />
  </div>
);

export default function LayoutShell() {
  const [layout, setLayout] = useState(() => localStorage.getItem("dp-layout") || "command");

  useEffect(() => {
    const handler = () => setLayout(localStorage.getItem("dp-layout") || "command");
    window.addEventListener("dp-layout-change", handler);
    return () => window.removeEventListener("dp-layout-change", handler);
  }, []);

  if (layout === "glass") return <Suspense fallback={fallback}><GlassLayout /></Suspense>;
  if (layout === "atlas") return <Suspense fallback={fallback}><AtlasLayout /></Suspense>;
  if (layout === "nexus") return <Suspense fallback={fallback}><NexusLayout /></Suspense>;
  return <CommandLayout />;
}
