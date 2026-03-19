import { lazy, Suspense } from "react";
import CommandLayout from "./CommandLayout";

const GlassLayout = lazy(() => import("./GlassLayout"));
const AtlasLayout = lazy(() => import("./AtlasLayout"));

const fallback = (
  <div className="flex items-center justify-center h-screen bg-dark-900">
    <div className="w-8 h-8 border-4 border-rust-500 border-t-transparent rounded-full animate-spin" />
  </div>
);

export default function LayoutShell() {
  const layout = localStorage.getItem("dp-layout") || "command";

  if (layout === "glass") return <Suspense fallback={fallback}><GlassLayout /></Suspense>;
  if (layout === "atlas") return <Suspense fallback={fallback}><AtlasLayout /></Suspense>;
  return <CommandLayout />;
}
