import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect, useRef, useCallback, type ReactNode } from "react";
import { api } from "../api";
import ProvisionLog from "../components/ProvisionLog";

interface EnvVar {
  name: string;
  label: string;
  default: string;
  required: boolean;
  secret: boolean;
}

interface AppTemplate {
  id: string;
  name: string;
  description: string;
  category: string;
  image: string;
  default_port: number;
  container_port: string;
  env_vars: EnvVar[];
}

interface DeployedApp {
  container_id: string;
  name: string;
  template: string;
  status: string;
  port: number | null;
  domain: string | null;
  health: string | null;
  image: string | null;
  volumes: string[];
  stack_id: string | null;
}

interface StackInfo {
  id: string;
  name: string;
  service_count: number;
  running: number;
  total: number;
  status: string;
  services: DeployedApp[];
  created_at: string;
  updated_at: string;
}

interface ComposeService {
  name: string;
  image: string;
  ports: { host: number; container: number; protocol: string }[];
  environment: Record<string, string>;
  volumes: string[];
  restart: string;
}

interface ComposeDeployResult {
  services: { name: string; container_id: string; status: string; error: string | null }[];
}

const categoryColors: Record<string, string> = {
  CMS: "bg-accent-500/15",
  Database: "bg-rust-500/15",
  Monitoring: "bg-accent-600/15",
  DevOps: "bg-warn-500/15",
  Automation: "bg-accent-500/15",
  Tools: "bg-dark-500/15",
  Development: "bg-accent-400/15",
  Analytics: "bg-accent-600/15",
  Storage: "bg-sky-500/15",
  Security: "bg-danger-500/15",
  Media: "bg-accent-600/15",
  Networking: "bg-rust-500/15",
  Mail: "bg-rust-500/15",
};

// SVG icons for each app template
const appIcons: Record<string, ReactNode> = {
  // ─── CMS ─────────────────────────────────────────────────────
  wordpress: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><circle cx="64" cy="64" r="62" fill="#21759b"/><path fill="#fff" d="M8.4 64c0 21.7 12.6 40.4 30.8 49.3L12.9 42.4C10 48.8 8.4 56.2 8.4 64zm93.1-2.8c0-6.8-2.4-11.5-4.5-15.1-2.8-4.5-5.4-8.3-5.4-12.8 0-5 3.8-9.7 9.2-9.7h.7C91.7 14.5 78.6 8.4 64 8.4 44.6 8.4 27.7 19 18.5 34.7h3.5c5.7 0 14.6-.7 14.6-.7 3-.2 3.3 4.2.3 4.5 0 0-3 .3-6.3.5l20 59.5 12-36-8.5-23.5c-3-.2-5.8-.5-5.8-.5-3-.2-2.6-4.7.3-4.5 0 0 9.1.7 14.4.7 5.7 0 14.6-.7 14.6-.7 3-.2 3.3 4.2.4 4.5 0 0-3 .3-6.3.5l19.8 59 5.5-18.3c2.4-7.6 4.2-13 4.2-17.7zM65.1 69.2l-16.4 47.8c4.9 1.4 10.1 2.2 15.4 2.2 6.3 0 12.4-1.1 18.1-3.1-.1-.2-.3-.5-.4-.7L65.1 69.2zm45.3-31c.5 3.4.7 7.1.7 11.1 0 10.9-2 23.2-8.2 38.6l16.4-47.4c3.2-7.9 4.2-14.2 4.2-19.8 0-2-.1-3.9-.4-5.7-3.9 7.1-9 14.2-12.7 23.2z"/></svg>
  ),
  ghost: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#212121" d="M64 0C38.6 0 18 20.6 18 46v74c0 4.4 5.3 6.6 8.5 3.5 4-4 10.6-4 14.6 0 4 4 10.6 4 14.6 0 4-4 10.6-4 14.6 0 4 4 10.6 4 14.6 0 4-4 10.6-4 14.6 0 3.2 3.2 8.5.9 8.5-3.5V46C110 20.6 89.4 0 64 0z"/><circle fill="#fff" cx="48" cy="46" r="10"/><circle fill="#fff" cx="80" cy="46" r="10"/></svg>
  ),
  strapi: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#4945ff" d="M43 2h83v83H85V44H43V2z"/><path fill="#4945ff" opacity=".4" d="M2 43h41v42h42v41H43c-22.6 0-41-18.4-41-41V43z"/><path fill="#4945ff" opacity=".7" d="M43 2v42h42l-42-42z"/></svg>
  ),
  directus: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#6644ff" d="M64 4c-2.8 0-5.2 1.6-6.4 4L6.4 100c-1.6 3.2.4 7 4 7h24c2.4 0 4.4-1.2 5.6-3.2L64 64l23.6 39.6c1.2 2 3.4 3.4 5.8 3.4h24.2c3.6 0 5.6-3.8 4-7L70.4 8c-1.2-2.4-3.6-4-6.4-4z"/><path fill="#6644ff" opacity=".5" d="M64 44L44.4 78h39.2L64 44z"/></svg>
  ),
  // ─── Databases ───────────────────────────────────────────────
  postgres: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#336791" d="M100.4 78c-2-6-6.4-8.4-6.4-8.4s2.4-5.6 2.8-14.4c.6-12-2.2-20.8-6.6-27.6C84.6 18 74 12 64 12c-10.2 0-19.2 5-25.2 13.2-5.4 7.4-8.6 17.6-7.2 30.4.8 8 3.6 14.8 3.6 14.8s-3.8 2.4-5.6 7.6c-2.4 7 1 14 6 16s11-1 13.4-8c.2-.6.4-1.2.4-1.8 3 1.4 7 2.2 11.2 2.4v.4c-3.2 3.4-7.8 2.2-10.4 4.4-3 2.4-3.4 8.2-.8 11.6 2.8 3.6 10.4 5.6 17.2 1.2 5-3.2 6.8-9.2 6.4-14.8 3.8-.8 7-2.4 9.4-4.6.4 5.8 1 10.4 2.2 12.6 3 5.4 9.4 8.6 16 7.4 4.4-.8 7.6-3.6 8-7.8.6-5.4-3-8.6-8-11.8-3-2-4.8-3.6-5.2-7.2zM48 68.4c-2.2 5.8-5.4 7-7.4 6.2s-3.2-4.4-1.4-9c1.2-3 3.4-4.6 3.4-4.6s1.8 3 3.8 5.2c.8.8 1.6 1.6 2.4 2.2-.2 0-.6 0-.8 0zm38.4-8c-5 7.4-12.2 10.8-19.8 11.4 0 0 .4-1 .4-3 0-3.6-2.4-5.4-2.4-5.4s6 .4 11.2-3.6c3.8-2.8 5.6-7.4 5.6-7.4s.2 2 .6 3.8c.6 2 2.6 3.2 4.4 4.2z"/><circle fill="#336791" cx="52" cy="44" r="5"/><circle fill="#fff" cx="53" cy="43" r="2"/><path fill="#336791" d="M78 36c-2.8-2-6.6-1.6-8.4.8l-.4.6c5.8 2 9.4 7.4 9.4 7.4 2.2-2.6 2.2-6.8-.6-8.8z"/></svg>
  ),
  mysql: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#4479a1" d="M116 100c-4.4-3.2-8.6-5.6-12.4-7.2-2-6.8-5.4-12.6-10.2-17.2 4.8-6.2 7.8-13.4 8.6-21.6 1-12-2.6-22.4-9.8-30.4C84.8 15.2 74.4 10 64 10c-10.6 0-20.8 5.2-28.2 13.6-7.2 8-10.8 18.4-9.8 30.4.8 8.2 3.8 15.4 8.6 21.6-4.8 4.6-8.2 10.4-10.2 17.2-3.8 1.6-8 4-12.4 7.2l4.8 6.4c3-2.2 5.8-3.8 8.4-5 0 .6-.2 1.2-.2 1.8 0 3.4 2 6.6 5 8l3-6c-.8-.4-1.4-1.2-1.4-2 0-2.6 1.6-6.4 4.2-10.6 1.4 9.6 6.2 17.6 13.6 23l4-5.6c-8.4-6.2-12.6-15.6-12.6-27.6 0-9 3-17.4 8.4-23.4 5-5.6 11.6-8.6 18.8-8.6s13.8 3 18.8 8.6c5.4 6 8.4 14.4 8.4 23.4 0 12-4.2 21.4-12.6 27.6l4 5.6c7.4-5.4 12.2-13.4 13.6-23 2.6 4.2 4.2 8 4.2 10.6 0 .8-.6 1.6-1.4 2l3 6c3-1.4 5-4.6 5-8 0-.6-.2-1.2-.2-1.8 2.6 1.2 5.4 2.8 8.4 5l4.8-6.4z"/><path fill="#4479a1" d="M64 34c-13.2 0-22 12-22 26s8.8 26 22 26 22-12 22-26-8.8-26-22-26zm-8 20c-2.2 0-4-2.2-4-5s1.8-5 4-5 4 2.2 4 5-1.8 5-4 5zm16 0c-2.2 0-4-2.2-4-5s1.8-5 4-5 4 2.2 4 5-1.8 5-4 5z"/></svg>
  ),
  mariadb: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#c0765a" d="M112 28c-4 0-8.4 1.6-12.4 4.4-5.6 3.8-10.4 5.6-14.8 5.6-2 0-3.8-.4-5.6-1.2-5.4-2.6-9.6-3.8-14.6-3.8-8.6 0-16 4.2-23.6 13.4C33 56.8 28 68.8 24 84c-1.4 5.2-3.6 8.6-6.8 10.6-2 1.2-4.2 1.8-6.8 1.8H8v8h2.4c4 0 7.8-1 11-3 5.4-3.4 9-9 11.2-16.8 3.6-14 8.2-25 12.8-33 6.4-7.6 12.2-11 19-11 4 0 7.4 1 12.2 3.4 2.6 1.2 5.2 1.8 8 1.8 6 0 12-2.4 18.4-6.8 3-2 5.8-3.4 8.4-3.4h.6v-7.6h-2z"/><circle fill="#c0765a" cx="100" cy="44" r="4"/></svg>
  ),
  mongo: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#47a248" d="M68.2 6.4c-2-3.2-3.6-5.2-4.2-6.4-.6 1.2-2.2 3.2-4.2 6.4C46.6 26 24 50 24 76c0 20 14 38 36.8 42.8l1.2.2v-8.2c0-1-.2-1.8-.4-2.6-8.4-3-14.6-11.6-14.6-21.6 0-10.2 6.2-18.6 14.8-21.6l1.6-.6.6-1.6c.4-1.2.4-2.6.4-4V8.4h.4v50.4c0 1.4 0 2.8.4 4l.6 1.6 1.6.6c8.6 3 14.8 11.4 14.8 21.6 0 10-6.2 18.6-14.6 21.6-.2.8-.4 1.6-.4 2.6V119l1.2-.2C90 115 104 96 104 76 104 50 81.4 26 68.2 6.4z"/><path fill="#2d9c3c" d="M62.4 62.4v-54h3.2v54c0 1.2 0 2 .2 2.8-1.2-.2-2.4-.2-3.6 0 .2-.8.2-1.6.2-2.8z"/></svg>
  ),
  redis: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#d82c20" d="M121.8 69.8c-3.8 2-23.4 10-27.6 12.2-4.2 2.2-6.5 2.1-9.8.4-3.3-1.7-24.8-10.3-28.6-12-3.8-1.8-3.7-3.1 0-4.7 3.8-1.7 25.2-10.4 28.5-11.8 3.3-1.5 5.6-1.5 9.5.3 3.9 1.8 24 9.8 28 11.6 4.2 1.8 3.8 2.1 0 4zM121.8 55.6c-3.8 2-23.4 10-27.6 12.2-4.2 2.2-6.5 2.1-9.8.4-3.3-1.7-24.8-10.3-28.6-12-3.8-1.8-3.7-3.1 0-4.7 3.8-1.7 25.2-10.4 28.5-11.8 3.3-1.5 5.6-1.5 9.5.3 3.9 1.8 24 9.8 28 11.6 4.2 1.8 3.8 2.1 0 4z"/><path fill="#a41209" d="M121.8 83.6c-3.8 2-23.4 10-27.6 12.2-4.2 2.2-6.5 2.1-9.8.4-3.3-1.7-24.8-10.3-28.6-12-3.8-1.8-3.7-3.1 0-4.7 3.8-1.7 25.2-10.4 28.5-11.8 3.3-1.5 5.6-1.5 9.5.3 3.9 1.8 24 9.8 28 11.6 4.2 1.8 3.8 2.1 0 4z"/><path fill="#d82c20" d="M68.2 32.5l24.2 4.8-8.2 3.8-24-5z"/><ellipse fill="#fff" cx="91.6" cy="41.6" rx="8" ry="3.2"/><path fill="#7a0c00" d="M63.2 27.6l8.8 17.8 19-8.4-8.8-17.8z"/><path fill="#ad1c12" d="M63.2 27.6l8.8 17.8 8.4-3.8-8.8-14z"/></svg>
  ),
  // ─── Monitoring ──────────────────────────────────────────────
  grafana: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#f46800" d="M64 6C32 6 6 32 6 64s26 58 58 58 58-26 58-58S96 6 64 6zm0 100c-23.2 0-42-18.8-42-42s18.8-42 42-42 42 18.8 42 42-18.8 42-42 42z"/><path fill="#f46800" d="M64 30c-18.8 0-34 15.2-34 34s15.2 34 34 34 34-15.2 34-34-15.2-34-34-34zm0 58c-13.2 0-24-10.8-24-24s10.8-24 24-24 24 10.8 24 24-10.8 24-24 24z"/><path fill="#f46800" d="M40 62h10l6-12 8 28 6-20 4 8h14v4H72l-6-10-8 28-6-22-4 8H40z"/></svg>
  ),
  prometheus: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#e6522c" d="M64 4C30.9 4 4 30.9 4 64s26.9 60 60 60 60-26.9 60-60S97.1 4 64 4zm0 108.6c-7.8 0-14.2-4.8-14.2-10.8h28.4c0 6-6.4 10.8-14.2 10.8zm23.4-15.4H40.6v-8.6h46.8v8.6zm-.2-13H40.8c-.4-.4-.8-1-1.2-1.4-5.8-7.2-7.2-10.8-8.8-14.2-.2 0 6.2 2.8 10.6 4.6 0 0 2.2 1 5.4 2.2-3.6-4.2-5.8-9.8-5.8-11.4 0 0 4.4 3.6 10.2 5.8-2.6-4.2-4.6-11.2-3.6-15.2 3.2 4.8 10 9.2 12.2 10.2-2.4-5.4-2-13.8 1.2-18.6 1.8 5.4 7.6 11.6 11.4 13.6-1.2-3-2-9-1-12.8 2.4 4 7.2 9 8 15.2 2.2-1.8 4-4.2 5.6-6.8l1.6 8c.4 2.4.8 4.2.2 7.4-.2 1.4-.6 2.8-1.2 4.2-1 2.4-2.6 4.4-5.2 7.6-.4.4-.6.8-1 1.2z"/></svg>
  ),
  "uptime-kuma": (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="20" fill="#5cdd8b" width="128" height="128"/><path fill="#fff" d="M34 70V52c0-16.6 13.4-30 30-30s30 13.4 30 30v18" strokeWidth="0"/><circle fill="#fff" cx="50" cy="56" r="5"/><circle fill="#5cdd8b" cx="50" cy="56" r="2.5"/><circle fill="#fff" cx="78" cy="56" r="5"/><circle fill="#5cdd8b" cx="78" cy="56" r="2.5"/><path fill="#fff" d="M34 70c0 16.6 13.4 30 30 30s30-13.4 30-30H34z" opacity=".5"/><path fill="none" stroke="#fff" strokeWidth="5" strokeLinecap="round" d="M20 78l14-14 10 10 14-20 14 16 10-8 16 16" opacity=".9"/></svg>
  ),
  loki: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#f46800" width="128" height="128"/><path fill="#fff" d="M32 32h8v56H32z" opacity=".9"/><path fill="#fff" d="M32 96h48v8H32z" opacity=".9"/><path fill="#fff" d="M56 44h36v6H56zm0 14h28v6H56zm0 14h32v6H56z" opacity=".6"/><circle fill="#fff" cx="96" cy="36" r="6"/></svg>
  ),
  // ─── Analytics ───────────────────────────────────────────────
  plausible: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#5850ec" width="128" height="128"/><path fill="#fff" d="M32 96V64l20-28h4l20 28v32h-12V72l-10-14-10 14v24H32zm44 0V48l20-28h4v76H88V36l-8 12v48h-4z" opacity=".95"/></svg>
  ),
  umami: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#000" width="128" height="128"/><path fill="#fff" d="M20 88c0-4.4 3.6-8 8-8h72c4.4 0 8 3.6 8 8s-3.6 8-8 8H28c-4.4 0-8-3.6-8-8z"/><path fill="#fff" d="M32 68c0-4.4 3.6-8 8-8h56c4.4 0 8 3.6 8 8s-3.6 8-8 8H40c-4.4 0-8-3.6-8-8z" opacity=".7"/><path fill="#fff" d="M48 48c0-4.4 3.6-8 8-8h36c4.4 0 8 3.6 8 8s-3.6 8-8 8H56c-4.4 0-8-3.6-8-8z" opacity=".4"/></svg>
  ),
  matomo: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#3152a0" d="M64 8C33 8 8 33 8 64s25 56 56 56 56-25 56-56S95 8 64 8z"/><path fill="#fff" d="M44 88l-8-32 16-20 16 20-8 32H44zm32 0l-6-24 14-28 14 28-6 24H76z" opacity=".9"/><circle fill="#95c748" cx="52" cy="36" r="6"/><circle fill="#95c748" cx="84" cy="36" r="6"/></svg>
  ),
  metabase: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#509ee3" width="128" height="128"/><rect fill="#fff" x="20" y="56" width="16" height="48" rx="4"/><rect fill="#fff" x="44" y="36" width="16" height="68" rx="4"/><rect fill="#fff" x="68" y="48" width="16" height="56" rx="4"/><rect fill="#fff" x="92" y="24" width="16" height="80" rx="4"/></svg>
  ),
  // ─── Tools ───────────────────────────────────────────────────
  adminer: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#22c55e" width="128" height="128"/><path fill="#fff" d="M64 20c-22 0-40 7.2-40 16v56c0 8.8 18 16 40 16s40-7.2 40-16V36c0-8.8-18-16-40-16z" opacity=".15"/><ellipse fill="#fff" cx="64" cy="36" rx="36" ry="12" opacity=".9"/><path d="M28 48c0 6.6 16.1 12 36 12s36-5.4 36-12" opacity=".6" stroke="#fff" strokeWidth="2" fill="none"/><path d="M28 68c0 6.6 16.1 12 36 12s36-5.4 36-12" opacity=".4" stroke="#fff" strokeWidth="2" fill="none"/><path fill="none" stroke="#fff" strokeWidth="2" opacity=".3" d="M28 36v56M100 36v56"/></svg>
  ),
  portainer: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#13bef9" width="128" height="128"/><path fill="#fff" d="M40 36h20v20H40zm24 0h20v20H64zm-24 24h20v20H40zm24 0h20v20H64zm-24 24h20v20H40z" opacity=".9"/><path fill="#fff" d="M64 84h20v20H64z" opacity=".5"/><rect fill="#fff" x="88" y="60" width="20" height="20" opacity=".3"/></svg>
  ),
  pgadmin: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#336791" width="128" height="128"/><path fill="#fff" d="M64 18c-18.8 0-34 13.6-34 30.4 0 10.8 6.4 20.2 16 25.6V96c0 4.4 3.6 8 8 8h20c4.4 0 8-3.6 8-8V74c9.6-5.4 16-14.8 16-25.6C98 31.6 82.8 18 64 18z" opacity=".2"/><path fill="#fff" d="M64 24c-15.2 0-27.6 11-27.6 24.6 0 9.4 5.8 17.6 14.4 21.6l2.2 1V88h22V71.2l2.2-1c8.6-4 14.4-12.2 14.4-21.6C91.6 35 79.2 24 64 24zM56 54a5 5 0 110-10 5 5 0 010 10zm16 0a5 5 0 110-10 5 5 0 010 10z" opacity=".9"/><path fill="#fff" d="M53 88h22v6H53z" opacity=".6"/><circle fill="#fff" cx="92" cy="28" r="6" opacity=".7"/><path fill="#fff" d="M92 34v16" stroke="#fff" strokeWidth="3" opacity=".5"/><path fill="#fff" d="M84 42h16" stroke="#fff" strokeWidth="3" opacity=".5"/></svg>
  ),
  minio: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#c72c48" width="128" height="128"/><path fill="#fff" d="M28 92V52l20-24v64h-8V44l-4 4v44h-8zm24 0V36l20 28V92h-8V72l-4-5.6V92h-8zm24 0V28l20 24v40h-8V58l-4-4v38h-8zm24 0V52l12-14v54H100z" opacity=".9"/></svg>
  ),
  meilisearch: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#ff5caa" d="M20 100l24-72h20l-24 72H20zm22 0l24-72h20l-24 72H42zm22 0l24-72h20l-24 72H64z" opacity=".9"/></svg>
  ),
  nocodb: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#7f66ff" width="128" height="128"/><path fill="#fff" d="M24 32h80c2.2 0 4 1.8 4 4v56c0 2.2-1.8 4-4 4H24c-2.2 0-4-1.8-4-4V36c0-2.2 1.8-4 4-4z" opacity=".15"/><path fill="#fff" d="M20 44h88v3H20zm0 20h88v3H20zm0 20h88v3H20z" opacity=".7"/><path fill="#fff" d="M52 32v64" stroke="#fff" strokeWidth="3" opacity=".5"/><path fill="#fff" d="M20 32h88v12H20z" opacity=".3"/><circle fill="#fff" cx="96" cy="76" r="6" opacity=".8"/></svg>
  ),
  searxng: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#3050ff" width="128" height="128"/><circle fill="none" stroke="#fff" strokeWidth="10" cx="52" cy="52" r="22"/><path fill="#fff" d="M68 72l28 28c2.8 2.8 2.8 7.2 0 10s-7.2 2.8-10 0L58 82l10-10z"/></svg>
  ),
  vaultwarden: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#175ddc" d="M64 4L12 28v36c0 28 22 52 52 60 30-8 52-32 52-60V28L64 4z"/><path fill="#fff" d="M64 20L24 38v26c0 22 16 42 40 48 24-6 40-26 40-48V38L64 20z" opacity=".15"/><rect fill="#fff" x="48" y="48" width="32" height="36" rx="4"/><path fill="#fff" d="M56 48V38c0-4.4 3.6-8 8-8s8 3.6 8 8v10h-4V38c0-2.2-1.8-4-4-4s-4 1.8-4 4v10h-4z"/><circle fill="#175ddc" cx="64" cy="64" r="4"/><rect fill="#175ddc" x="62" y="64" width="4" height="10" rx="2"/></svg>
  ),
  // ─── Storage ─────────────────────────────────────────────────
  nextcloud: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#0082c9" d="M64 28c-16 0-29 10.4-33.2 24.8C22.4 49.2 13.4 53.2 8 61.4c-7.8 12-4.6 28 7.2 36.2 5 3.4 10.6 4.8 16.2 4.4C37 110.8 46.4 116 57.4 116c8.6 0 16.2-3.8 21.4-9.8 4.4 2.6 9.6 3.8 15 3.2 14.2-1.6 24.6-14.2 23.4-28.4-.8-9.6-6.4-17.6-14.2-21.8C99 43.2 82.8 28 64 28zm-6.6 16c11.6 0 21 9.4 21 21s-9.4 21-21 21-21-9.4-21-21 9.4-21 21-21zm35.2 8c8.4 0 15.4 6.8 15.4 15.2S101 82.4 92.6 82.4 77.2 75.6 77.2 67.2 84.2 52 92.6 52zM35.4 58c7 0 12.6 5.6 12.6 12.6S42.4 83.2 35.4 83.2s-12.6-5.6-12.6-12.6S28.4 58 35.4 58z"/></svg>
  ),
  // ─── Development ─────────────────────────────────────────────
  gitea: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#609926" width="128" height="128"/><path fill="#fff" d="M48 28c-4.4 0-8 3.6-8 8v4h-4c-4.4 0-8 3.6-8 8v4c0 4.4 3.6 8 8 8h4v20c0 8.8 7.2 16 16 16h16c8.8 0 16-7.2 16-16V60h4c4.4 0 8-3.6 8-8v-4c0-4.4-3.6-8-8-8h-4v-4c0-4.4-3.6-8-8-8H48z" opacity=".2"/><path fill="#fff" d="M56 40h16v8H56z" opacity=".9"/><path fill="#fff" d="M48 52h32c4.4 0 8 3.6 8 8v20c0 4.4-3.6 8-8 8H48c-4.4 0-8-3.6-8-8V60c0-4.4 3.6-8 8-8z" opacity=".9"/><circle fill="#609926" cx="54" cy="68" r="4"/><circle fill="#609926" cx="74" cy="68" r="4"/><path fill="none" stroke="#609926" strokeWidth="2.5" strokeLinecap="round" d="M56 78c2 2.4 4.8 3.6 8 3.6s6-1.2 8-3.6"/><path fill="#fff" d="M60 40V32c0-2.2 1.8-4 4-4s4 1.8 4 4v8" opacity=".7"/></svg>
  ),
  "code-server": (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#007acc" width="128" height="128"/><path fill="#fff" d="M48 32L16 64l32 32 8-8-24-24 24-24-8-8zm32 0l-8 8 24 24-24 24 8 8 32-32-32-32z" opacity=".9"/><rect fill="#fff" x="56" y="28" width="8" height="72" rx="4" transform="rotate(15 64 64)" opacity=".6"/></svg>
  ),
  drone: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#212121" width="128" height="128"/><path fill="#fff" d="M20 40h88v8H20zm0 20h88v8H20zm0 20h88v8H20z" opacity=".3"/><circle fill="#2196f3" cx="36" cy="44" r="8"/><path fill="#2196f3" d="M44 44h28v1H44z" opacity=".8"/><circle fill="#4caf50" cx="80" cy="44" r="8"/><path fill="#4caf50" d="M80 52v9" stroke="#4caf50" strokeWidth="2"/><circle fill="#ff9800" cx="56" cy="64" r="8"/><path fill="#ff9800" d="M56 72v9" stroke="#ff9800" strokeWidth="2"/><circle fill="#f44336" cx="92" cy="84" r="8"/><circle fill="#9c27b0" cx="36" cy="84" r="8"/></svg>
  ),
  registry: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#2496ed" width="128" height="128"/><path fill="#fff" d="M32 56h18v18H32zm22-22h18v40H54zm22-12h18v52H76z" opacity=".9"/><path fill="#fff" d="M20 82c4-4 10-6 18-6 12 0 18 4 26 4s14-2 26-2c10 0 16 2 20 4v8c-4-2-10-4-20-4-12 0-18 2-26 2s-14-4-26-4c-8 0-14 2-18 6v-8z" opacity=".7"/><circle fill="#fff" cx="22" cy="78" r="4"/></svg>
  ),
  mailhog: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#c48821" width="128" height="128"/><rect fill="#fff" x="20" y="36" width="88" height="56" rx="8" opacity=".2"/><path fill="#fff" d="M24 40l40 28 40-28v8L68 76c-2 1.5-6 1.5-8 0L24 48v-8z" opacity=".9"/><path fill="none" stroke="#fff" strokeWidth="4" opacity=".6" d="M24 40l40 28 40-28"/></svg>
  ),
  // ─── Automation ──────────────────────────────────────────────
  n8n: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#ea4b71" width="128" height="128"/><rect fill="#fff" x="20" y="52" width="24" height="24" rx="6"/><rect fill="#fff" x="84" y="28" width="24" height="24" rx="6"/><rect fill="#fff" x="84" y="76" width="24" height="24" rx="6"/><path fill="none" stroke="#fff" strokeWidth="4" strokeLinecap="round" opacity=".7" d="M44 64h28M76 58l8-12M76 70l8 12"/></svg>
  ),
  // ─── Media ───────────────────────────────────────────────────
  jellyfin: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#00a4dc" d="M64 4C46 4 38 24 38 44c0 16 6 28 16 38 4 4 8 6 10 8v34h-4c-2 0-3.6 1.6-3.6 3.6s1.6 3.4 3.6 3.4h8c2 0 3.6-1.6 3.6-3.6V90c2-2 6-4 10-8 10-10 16-22 16-38C98 24 82 4 64 4z"/><path fill="#fff" d="M64 12c-12 0-18 16-18 32 0 12 4.8 22.4 12.8 30.4L64 80l5.2-5.6C77.2 66.4 82 56 82 44c0-16-6-32-18-32z" opacity=".3"/><ellipse fill="#fff" cx="64" cy="44" rx="10" ry="16" opacity=".6"/></svg>
  ),
  // ─── Networking ──────────────────────────────────────────────
  pihole: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><circle fill="#96060c" cx="64" cy="64" r="60"/><circle fill="#f44" cx="64" cy="64" r="48" opacity=".3"/><path fill="#fff" d="M64 16c-26.5 0-48 20.5-48 46 0 4 .5 8 1.5 12h93c1-4 1.5-8 1.5-12 0-25.5-21.5-46-48-46z" opacity=".1"/><path fill="#fff" d="M46 34h28c4.4 0 8 3.6 8 8v4H46v-4c0-4.4 0-8 0-8zm0 20h36v4c0 2-2 4-4 4H50l-4-8z" opacity=".9"/><path fill="#fff" d="M46 34v28M58 34v28" stroke="#fff" strokeWidth="3" opacity=".6"/><text x="44" y="96" fill="#fff" fontSize="32" fontWeight="bold" fontFamily="serif" opacity=".9">&#x3c0;</text></svg>
  ),
  // ─── CMS (additional) ─────────────────────────────────────────
  drupal: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#0678be" d="M64 4C46 4 42 18 34 26c-4 4-10 6-10 6 0 16 4 32 14 44 10 12 24 18 26 44h0c2-26 16-32 26-44 10-12 14-28 14-44 0 0-6-2-10-6C86 18 82 4 64 4z"/><circle fill="#fff" cx="50" cy="60" r="8" opacity=".7"/><circle fill="#fff" cx="78" cy="60" r="8" opacity=".7"/><circle fill="#fff" cx="64" cy="84" r="6" opacity=".5"/></svg>
  ),
  joomla: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#f44321" d="M42 24c-10 0-18 8-18 18v4l20 20 14-14-20-20h4c2 0 4-2 4-4s-2-4-4-4z" opacity=".9"/><path fill="#f9a541" d="M86 24h-4l-20 20 14 14 20-20v-4c0-10-8-10-10-10z" opacity=".9"/><path fill="#5091cd" d="M24 86v-4l20-20 14 14-20 20h-4c-10 0-10-8-10-10z" opacity=".9"/><path fill="#2fB84f" d="M104 68v4c0 10-8 18-18 18h-4l-20-20 14-14 20 20v-4c0-2 2-4 4-4s4 2 4 0z" opacity=".9"/><circle fill="#fff" cx="64" cy="64" r="8" opacity=".6"/></svg>
  ),
  prestashop: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#df0067" width="128" height="128"/><path fill="#fff" d="M40 40h48c4.4 0 8 3.6 8 8v4H40v-4c0-4.4 3.6-8 0-8zm0 20h56v4H40v-4zm0 12h48v4H40v-4zm0 12h40v4H40v-4z" opacity=".8"/><circle fill="#fff" cx="48" cy="100" r="6"/><circle fill="#fff" cx="80" cy="100" r="6"/><path fill="#fff" d="M32 32l4 48h56l4-48" opacity=".15"/></svg>
  ),
  // ─── Wiki / Docs ──────────────────────────────────────────────
  wikijs: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#1976d2" width="128" height="128"/><path fill="#fff" d="M28 32h72c2.2 0 4 1.8 4 4v56c0 2.2-1.8 4-4 4H28c-2.2 0-4-1.8-4-4V36c0-2.2 1.8-4 4-4z" opacity=".15"/><path fill="#fff" d="M36 48h56v4H36zm0 12h48v4H36zm0 12h52v4H36zm0 12h36v4H36z" opacity=".7"/><path fill="#fff" d="M36 32v64" stroke="#fff" strokeWidth="3" opacity=".4"/><text x="78" y="46" fill="#fff" fontSize="24" fontWeight="bold" fontFamily="sans-serif" opacity=".9">W</text></svg>
  ),
  bookstack: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#0288d1" width="128" height="128"/><rect fill="#fff" x="28" y="24" width="72" height="16" rx="4" opacity=".5"/><rect fill="#fff" x="28" y="44" width="72" height="16" rx="4" opacity=".65"/><rect fill="#fff" x="28" y="64" width="72" height="16" rx="4" opacity=".8"/><rect fill="#fff" x="28" y="84" width="72" height="16" rx="4" opacity=".95"/><path fill="none" stroke="#0288d1" strokeWidth="2" d="M40 30h48M40 50h40M40 70h44M40 90h36" opacity=".5"/></svg>
  ),
  outline: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#0366d6" width="128" height="128"/><path fill="#fff" d="M64 20L28 64l36 44 36-44L64 20z" opacity=".2"/><path fill="#fff" d="M64 32L36 64l28 32 28-32L64 32z" opacity=".5"/><path fill="#fff" d="M64 44L44 64l20 20 20-20L64 44z" opacity=".9"/></svg>
  ),
  // ─── Communication ────────────────────────────────────────────
  mattermost: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#0058cc" width="128" height="128"/><path fill="#fff" d="M64 20c-24.3 0-44 17.9-44 40 0 10 4 19.2 10.6 26.2L26 104l20-10c5.4 2.6 11.6 4 18 4 24.3 0 44-17.9 44-40S88.3 20 64 20z" opacity=".9"/><path fill="#0058cc" d="M48 50h32v6H48zm0 12h24v6H48z"/></svg>
  ),
  rocketchat: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#f5455c" width="128" height="128"/><path fill="#fff" d="M64 20c-26.5 0-48 16.6-48 37 0 11 5.8 20.9 15 27.8L28 104l18-10c5.8 2.2 12.2 3.4 18 3.4 26.5 0 48-16.6 48-37S90.5 20 64 20z" opacity=".9"/><circle fill="#f5455c" cx="48" cy="58" r="5"/><circle fill="#f5455c" cx="64" cy="58" r="5"/><circle fill="#f5455c" cx="80" cy="58" r="5"/></svg>
  ),
  discourse: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><circle fill="#231f20" cx="64" cy="64" r="60"/><path fill="#fff" d="M64 24c-22 0-40 16.6-40 37s18 37 40 37l32 10-12-18c8-7 12-17 12-29 0-20.4-14.3-37-32-37z" opacity=".15"/><path fill="#87ceeb" d="M42 52h44v6H42z" opacity=".9"/><path fill="#f0c040" d="M42 64h36v6H42z" opacity=".9"/><path fill="#e45735" d="M42 76h28v6H42z" opacity=".9"/></svg>
  ),
  // ─── Media (additional) ───────────────────────────────────────
  immich: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#4250af" width="128" height="128"/><rect fill="#fff" x="20" y="28" width="88" height="72" rx="8" opacity=".15"/><circle fill="#fff" cx="52" cy="56" r="16" opacity=".7"/><circle fill="#4250af" cx="52" cy="56" r="8"/><path fill="#fff" d="M68 44h28v28H68z" opacity=".5" rx="4"/><path fill="#fff" d="M20 84l28-20 16 12 20-16 24 18v14c0 4.4-3.6 8-8 8H28c-4.4 0-8-3.6-8-8V84z" opacity=".6"/></svg>
  ),
  photoprism: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#01b189" width="128" height="128"/><circle fill="#fff" cx="64" cy="56" r="28" opacity=".2"/><path fill="#fff" d="M64 32c-13.2 0-24 10.8-24 24s10.8 24 24 24 24-10.8 24-24-10.8-24-24-24zm0 40c-8.8 0-16-7.2-16-16s7.2-16 16-16 16 7.2 16 16-7.2 16-16 16z" opacity=".9"/><circle fill="#fff" cx="64" cy="56" r="8" opacity=".6"/><path fill="#fff" d="M40 92h48v4c0 2.2-1.8 4-4 4H44c-2.2 0-4-1.8-4-4v-4z" opacity=".5"/></svg>
  ),
  // ─── Security (additional) ────────────────────────────────────
  authentik: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#fd4b2d" width="128" height="128"/><path fill="#fff" d="M64 16L24 48v32c0 20 16 32 40 36 24-4 40-16 40-36V48L64 16z" opacity=".15"/><path fill="#fff" d="M64 28L32 52v28c0 16 12 26 32 30 20-4 32-14 32-30V52L64 28z" opacity=".3"/><path fill="#fff" d="M64 40L40 56v20c0 12 8 20 24 24 16-4 24-12 24-24V56L64 40z" opacity=".9"/><circle fill="#fd4b2d" cx="64" cy="64" r="8"/></svg>
  ),
  keycloak: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#4d4d4d" width="128" height="128"/><path fill="#fff" d="M40 28h48l-12 24H52L40 28z" opacity=".6"/><path fill="#fff" d="M52 52h24v24H52z" opacity=".9"/><path fill="#fff" d="M40 100h48l-12-24H52L40 100z" opacity=".6"/><path fill="#00b4f0" d="M60 58h8v12h-8z"/><circle fill="#00b4f0" cx="64" cy="56" r="4"/></svg>
  ),
  // ─── Dev Tools (additional) ───────────────────────────────────
  woodpecker: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#4caf50" width="128" height="128"/><path fill="#fff" d="M24 96V44l20-20v52L24 96zm28 0V32l20-20v64L52 96zm28 0V20l24-8v68L80 96z" opacity=".85"/></svg>
  ),
  sonarqube: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#4e9bcd" width="128" height="128"/><path fill="#fff" d="M28 96c0-40 24-64 72-68v12c-40 4-60 24-60 56H28z" opacity=".9"/><path fill="#fff" d="M44 96c0-28 16-48 56-52v12c-32 4-44 18-44 40H44z" opacity=".6"/><path fill="#fff" d="M60 96c0-18 10-32 40-36v12c-22 3-28 12-28 24H60z" opacity=".35"/></svg>
  ),
  forgejo: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#fb923c" width="128" height="128"/><circle fill="#fff" cx="64" cy="48" r="20" opacity=".9"/><circle fill="#fb923c" cx="58" cy="44" r="4"/><circle fill="#fb923c" cx="70" cy="44" r="4"/><path fill="#fff" d="M44 72c0 16 8 28 20 32 12-4 20-16 20-32H44z" opacity=".7"/><path d="M56 52c2 3 4.8 4 8 4s6-1 8-4" stroke="#fb923c" strokeWidth="2" fill="none" strokeLinecap="round"/></svg>
  ),
  // ─── Business / Productivity ──────────────────────────────────
  "invoice-ninja": (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#2f2e2e" width="128" height="128"/><rect fill="#fff" x="28" y="24" width="72" height="80" rx="4" opacity=".15"/><path fill="#fff" d="M40 36h48v4H40zm0 12h48v2H40zm0 8h48v2H40zm0 8h32v2H40z" opacity=".7"/><path fill="#fff" d="M40 80h20v12H40z" opacity=".4"/><path d="M72 76l8 8 16-20" stroke="#51e898" strokeWidth="4" fill="none" strokeLinecap="round" strokeLinejoin="round"/></svg>
  ),
  erpnext: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#0089ff" width="128" height="128"/><rect fill="#fff" x="20" y="24" width="88" height="80" rx="8" opacity=".15"/><rect fill="#fff" x="28" y="56" width="16" height="36" rx="3" opacity=".9"/><rect fill="#fff" x="52" y="40" width="16" height="52" rx="3" opacity=".9"/><rect fill="#fff" x="76" y="48" width="16" height="44" rx="3" opacity=".9"/><path fill="#fff" d="M28 36h64v8H28z" opacity=".4"/></svg>
  ),
  calcom: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#292929" width="128" height="128"/><rect fill="#fff" x="24" y="32" width="80" height="72" rx="8" opacity=".15"/><rect fill="#fff" x="24" y="32" width="80" height="16" rx="8" opacity=".5"/><rect fill="#fff" x="40" y="24" width="4" height="16" rx="2"/><rect fill="#fff" x="84" y="24" width="4" height="16" rx="2"/><rect fill="#fff" x="36" y="56" width="12" height="12" rx="2" opacity=".3"/><rect fill="#fff" x="56" y="56" width="12" height="12" rx="2" opacity=".3"/><rect fill="#fff" x="76" y="56" width="12" height="12" rx="2" opacity=".3"/><rect fill="#fff" x="36" y="76" width="12" height="12" rx="2" opacity=".3"/><rect fill="#fff" x="56" y="76" width="12" height="12" rx="2" opacity=".9"/><rect fill="#fff" x="76" y="76" width="12" height="12" rx="2" opacity=".3"/></svg>
  ),
  // ─── Support ──────────────────────────────────────────────────
  chatwoot: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#1f93ff" width="128" height="128"/><path fill="#fff" d="M64 20c-24.3 0-44 17.9-44 40 0 8.4 2.8 16.2 7.6 22.6L22 104l23.8-8.6c5.6 2.8 12 4.4 18.2 4.4 24.3 0 44-17.9 44-40S88.3 20 64 20z" opacity=".9"/><circle fill="#1f93ff" cx="48" cy="60" r="5"/><circle fill="#1f93ff" cx="64" cy="60" r="5"/><circle fill="#1f93ff" cx="80" cy="60" r="5"/></svg>
  ),
  typebot: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#ff8e20" width="128" height="128"/><rect fill="#fff" x="20" y="24" width="40" height="28" rx="8" opacity=".9"/><rect fill="#fff" x="68" y="48" width="40" height="28" rx="8" opacity=".9"/><rect fill="#fff" x="20" y="76" width="48" height="28" rx="8" opacity=".7"/><path fill="none" stroke="#fff" strokeWidth="3" strokeLinecap="round" d="M48 52v8l20-4M72 76l-12 8" opacity=".6"/></svg>
  ),
};

const statusColors: Record<string, string> = {
  running: "bg-rust-500/15 text-rust-400",
  exited: "bg-danger-500/15 text-danger-400",
  created: "bg-warn-500/15 text-warn-400",
  paused: "bg-dark-700 text-dark-200",
};

export default function Apps() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const [templates, setTemplates] = useState<AppTemplate[]>([]);
  const [apps, setApps] = useState<DeployedApp[]>([]);
  const [loading, setLoading] = useState(true);
  const [deploying, setDeploying] = useState(false);
  const [deployId, setDeployId] = useState<string | null>(null);
  const [selected, setSelected] = useState<AppTemplate | null>(null);
  const [appName, setAppName] = useState("");
  const [appPort, setAppPort] = useState(0);
  const [envValues, setEnvValues] = useState<Record<string, string>>({});
  const [appDomain, setAppDomain] = useState("");
  const [sslEmail, setSslEmail] = useState("");
  const [memoryMb, setMemoryMb] = useState("");
  const [cpuPercent, setCpuPercent] = useState("");
  const [gpuEnabled, setGpuEnabled] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [logsTarget, setLogsTarget] = useState<string | null>(null);
  const [logLines, setLogLines] = useState<string[]>([]);
  const [logSearch, setLogSearch] = useState("");
  const [logAutoRefresh, setLogAutoRefresh] = useState(false);
  const [logAutoScroll, setLogAutoScroll] = useState(true);
  const logEndRef = useRef<HTMLDivElement>(null);
  const logIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  // Env vars state
  const [envTarget, setEnvTarget] = useState<string | null>(null);
  const [envVars, setEnvVars] = useState<{ key: string; value: string }[]>([]);
  const [envLoading, setEnvLoading] = useState(false);
  const [envEditing, setEnvEditing] = useState(false);
  const [envEditValues, setEnvEditValues] = useState<Record<string, string>>({});
  const [envSaving, setEnvSaving] = useState(false);
  // Update SSE state
  const [updateDeployId, setUpdateDeployId] = useState<string | null>(null);

  // Container stats state
  const [statsTarget, setStatsTarget] = useState<string | null>(null);
  const [statsData, setStatsData] = useState<{ cpu_percent?: string; memory_usage?: string; memory_percent?: string; network_io?: string; block_io?: string; pids?: string } | null>(null);
  const [statsLoading, setStatsLoading] = useState(false);

  // Container shell state
  const [shellContainer, setShellContainer] = useState<{ id: string; name: string } | null>(null);
  const [shellCmd, setShellCmd] = useState("");
  const [shellOutput, setShellOutput] = useState("");
  const shellOutputRef = useRef<HTMLDivElement>(null);

  // Volume browser state
  const [volumeTarget, setVolumeTarget] = useState<string | null>(null);
  const [volumeData, setVolumeData] = useState<{ source: string; destination: string; type: string; size_bytes: number; size_mb: number; listing: string }[]>([]);
  const [volumeLoading, setVolumeLoading] = useState(false);

  // Private registry state
  const [showRegistries, setShowRegistries] = useState(false);
  const [registries, setRegistries] = useState<string[]>([]);
  const [regServer, setRegServer] = useState("");
  const [regUsername, setRegUsername] = useState("");
  const [regPassword, setRegPassword] = useState("");
  const [regLoading, setRegLoading] = useState(false);

  // Compose import state
  const [showCompose, setShowCompose] = useState(false);
  const [composeYaml, setComposeYaml] = useState("");
  const [composeParsed, setComposeParsed] = useState<ComposeService[] | null>(null);
  const [composeDeploying, setComposeDeploying] = useState(false);
  const [composeError, setComposeError] = useState("");
  const [composeName, setComposeName] = useState("");

  // Image management state (Feature #6)
  const [showImages, setShowImages] = useState(false);
  const [images, setImages] = useState<{ repository: string; tag: string; id: string; size: string; created: string }[]>([]);
  const [imagesLoading, setImagesLoading] = useState(false);
  const [imagePruning, setImagePruning] = useState(false);

  // App tags state (Feature #7)
  const [appTags, setAppTags] = useState<Record<string, string>>(() => {
    try { return JSON.parse(localStorage.getItem('dp-app-tags') || '{}'); } catch { return {}; }
  });
  const [tagFilter, setTagFilter] = useState("");

  // Container metrics history state (Feature #8)
  const [statsHistory, setStatsHistory] = useState<Record<string, { cpu: number; mem: number }[]>>({});

  // Health check config state (Feature #9)
  const [enableHealthCheck, setEnableHealthCheck] = useState(false);
  const [healthCmd, setHealthCmd] = useState("");
  const [healthInterval, setHealthInterval] = useState("");
  const [healthTimeout, setHealthTimeout] = useState("");
  const [healthRetries, setHealthRetries] = useState("");

  // Compose view state (Feature #11 — packages tab)
  const [composeView, setComposeView] = useState<"compose" | "packages">("compose");

  // Image update check state
  const [updateInfo, setUpdateInfo] = useState<Record<string, { update_available: boolean; check_error?: string }>>({});
  const [updateChecking, setUpdateChecking] = useState(false);

  // Stacks state
  const [stacks, setStacks] = useState<StackInfo[]>([]);
  const [expandedStack, setExpandedStack] = useState<string | null>(null);
  const [stackActionLoading, setStackActionLoading] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([
      api.get<AppTemplate[]>("/apps/templates").catch(() => []),
      api.get<DeployedApp[]>("/apps").catch(() => []),
      api.get<StackInfo[]>("/stacks").catch(() => []),
    ]).then(([tmpl, deployed, stacksData]) => {
      setTemplates(tmpl);
      setApps(deployed);
      setStacks(stacksData);
      setLoading(false);
    });
  }, []);

  const loadApps = () => {
    api.get<DeployedApp[]>("/apps").then(setApps).catch(() => {});
    api.get<StackInfo[]>("/stacks").then(setStacks).catch(() => {});
  };

  const openDeploy = (tmpl: AppTemplate) => {
    setSelected(tmpl);
    setAppName(tmpl.id);
    setAppPort(tmpl.default_port);
    setAppDomain("");
    setSslEmail("");
    const defaults: Record<string, string> = {};
    tmpl.env_vars.forEach((v) => {
      defaults[v.name] = v.default;
    });
    setEnvValues(defaults);
  };

  const handleDeploy = async () => {
    if (!selected) return;
    setDeploying(true);
    setMessage({ text: "", type: "" });
    try {
      const deployBody: Record<string, unknown> = {
        template_id: selected.id,
        name: appName,
        port: appPort,
        env: envValues,
        ...(appDomain ? { domain: appDomain } : {}),
        ...(appDomain && sslEmail ? { ssl_email: sslEmail } : {}),
        ...(memoryMb ? { memory_mb: parseInt(memoryMb) } : {}),
        ...(cpuPercent ? { cpu_percent: parseInt(cpuPercent) } : {}),
        ...(gpuEnabled ? { gpu_enabled: true } : {}),
      };
      if (enableHealthCheck && healthCmd) {
        deployBody.health_check = {
          cmd: healthCmd,
          ...(healthInterval ? { interval_s: parseInt(healthInterval) } : {}),
          ...(healthTimeout ? { timeout_s: parseInt(healthTimeout) } : {}),
          ...(healthRetries ? { retries: parseInt(healthRetries) } : {}),
        };
      }
      const result = await api.post<{ deploy_id?: string }>("/apps/deploy", deployBody);
      setSelected(null);
      if (result.deploy_id) {
        setDeployId(result.deploy_id);
      } else {
        setMessage({
          text: `${selected.name} deployed successfully`,
          type: "success",
        });
        loadApps();
        setDeploying(false);
      }
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Deploy failed",
        type: "error",
      });
      setDeploying(false);
    }
  };

  const handleAction = async (containerId: string, action: "stop" | "start" | "restart") => {
    setActionLoading(`${containerId}-${action}`);
    setMessage({ text: "", type: "" });
    try {
      await api.post(`/apps/${containerId}/${action}`);
      setMessage({ text: `App ${action}ed successfully`, type: "success" });
      loadApps();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : `${action} failed`,
        type: "error",
      });
    } finally {
      setActionLoading(null);
    }
  };

  const fetchLogs = useCallback(async (containerId: string) => {
    try {
      const data = await api.get<{ logs: string }>(`/apps/${containerId}/logs`);
      setLogLines((data.logs || "").split("\n").filter(Boolean));
    } catch (e) {
      setLogLines([`Error: ${e instanceof Error ? e.message : "Failed to fetch logs"}`]);
    }
  }, []);

  const handleLogs = async (containerId: string) => {
    if (logsTarget === containerId) {
      closeLogs();
      return;
    }
    setLogsTarget(containerId);
    setLogLines([]);
    setLogSearch("");
    setLogAutoRefresh(false);
    setLogAutoScroll(true);
    if (logIntervalRef.current) clearInterval(logIntervalRef.current);
    await fetchLogs(containerId);
  };

  const closeLogs = () => {
    setLogsTarget(null);
    setLogLines([]);
    setLogSearch("");
    setLogAutoRefresh(false);
    if (logIntervalRef.current) { clearInterval(logIntervalRef.current); logIntervalRef.current = null; }
  };

  // Auto-refresh effect
  useEffect(() => {
    if (logAutoRefresh && logsTarget) {
      logIntervalRef.current = setInterval(() => fetchLogs(logsTarget), 3000);
      return () => { if (logIntervalRef.current) clearInterval(logIntervalRef.current); };
    } else {
      if (logIntervalRef.current) { clearInterval(logIntervalRef.current); logIntervalRef.current = null; }
    }
  }, [logAutoRefresh, logsTarget, fetchLogs]);

  // Auto-scroll to bottom
  useEffect(() => {
    if (logAutoScroll && logEndRef.current) {
      logEndRef.current.scrollIntoView({ behavior: "smooth" });
    }
  }, [logLines, logAutoScroll]);

  // Shell output auto-scroll
  useEffect(() => {
    if (shellOutputRef.current) {
      shellOutputRef.current.scrollTop = shellOutputRef.current.scrollHeight;
    }
  }, [shellOutput]);

  const handleEnv = async (containerId: string) => {
    setEnvTarget(containerId);
    setEnvLoading(true);
    try {
      const data = await api.get<{ env: { key: string; value: string }[] }>(`/apps/${containerId}/env`);
      setEnvVars(data.env || []);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load env vars", type: "error" });
      setEnvTarget(null);
    } finally {
      setEnvLoading(false);
    }
  };

  const handleEnvSave = async () => {
    if (!envTarget) return;
    setEnvSaving(true);
    try {
      await api.put(`/apps/${envTarget}/env`, { env: envEditValues });
      setMessage({ text: "Environment variables updated. Container was recreated.", type: "success" });
      setEnvTarget(null);
      setEnvEditing(false);
      loadApps();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to update env vars", type: "error" });
    } finally {
      setEnvSaving(false);
    }
  };

  const handleStats = async (containerId: string) => {
    if (statsTarget === containerId) {
      setStatsTarget(null);
      setStatsData(null);
      return;
    }
    setStatsTarget(containerId);
    setStatsLoading(true);
    try {
      const data = await api.get<{ cpu_percent?: string; memory_usage?: string; memory_percent?: string; network_io?: string; block_io?: string; pids?: string }>(`/apps/${containerId}/stats`);
      setStatsData(data);
    } catch {
      setStatsData(null);
    } finally {
      setStatsLoading(false);
    }
  };

  const handleShell = async (containerId: string, name: string) => {
    setShellContainer({ id: containerId, name });
    setShellCmd("");
    setShellOutput("");
  };

  const handleVolumes = async (containerId: string) => {
    setVolumeTarget(containerId);
    setVolumeLoading(true);
    try {
      const data = await api.get<{ volumes: typeof volumeData }>(`/apps/${containerId}/volumes`);
      setVolumeData(data.volumes || []);
    } catch {
      setVolumeData([]);
    } finally {
      setVolumeLoading(false);
    }
  };

  const loadRegistries = async () => {
    try {
      const data = await api.get<{ registries: string[] }>("/apps/registries");
      setRegistries(data.registries || []);
    } catch {
      setRegistries([]);
    }
  };

  const handleRegistryLogin = async () => {
    if (!regServer || !regUsername || !regPassword) return;
    setRegLoading(true);
    try {
      await api.post("/apps/registry-login", { server: regServer, username: regUsername, password: regPassword });
      setMessage({ text: `Logged in to ${regServer}`, type: "success" });
      setRegServer("");
      setRegUsername("");
      setRegPassword("");
      loadRegistries();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Registry login failed", type: "error" });
    } finally {
      setRegLoading(false);
    }
  };

  const handleRegistryLogout = async (server: string) => {
    try {
      await api.post("/apps/registry-logout", { server });
      setMessage({ text: `Logged out from ${server}`, type: "success" });
      loadRegistries();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Registry logout failed", type: "error" });
    }
  };

  const handleUpdate = async (containerId: string) => {
    setActionLoading(`${containerId}-update`);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.post<{ deploy_id?: string }>(`/apps/${containerId}/update`);
      if (result.deploy_id) {
        setUpdateDeployId(result.deploy_id);
      } else {
        setMessage({ text: "App updated to latest image", type: "success" });
        loadApps();
      }
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Update failed", type: "error" });
    } finally {
      setActionLoading(null);
    }
  };

  const handleCheckUpdates = async () => {
    setUpdateChecking(true);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.get<{ updates: { container_id: string; update_available: boolean; check_error?: string }[]; updates_available: number }>("/apps/updates");
      const info: Record<string, { update_available: boolean; check_error?: string }> = {};
      for (const u of result.updates) {
        info[u.container_id] = { update_available: u.update_available, check_error: u.check_error || undefined };
      }
      setUpdateInfo(info);
      setMessage({
        text: result.updates_available > 0
          ? `${result.updates_available} update${result.updates_available > 1 ? "s" : ""} available`
          : "All containers are up to date",
        type: result.updates_available > 0 ? "warning" : "success",
      });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Update check failed", type: "error" });
    } finally {
      setUpdateChecking(false);
    }
  };

  const downloadLogs = () => {
    const appName = apps.find(a => a.container_id === logsTarget)?.name || "container";
    const blob = new Blob([logLines.join("\n")], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${appName}-logs.txt`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const handleRemove = async (containerId: string) => {
    try {
      await api.delete(`/apps/${containerId}`);
      setDeleteTarget(null);
      if (logsTarget === containerId) {
        closeLogs();
      }
      setMessage({ text: "App removed", type: "success" });
      loadApps();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Remove failed",
        type: "error",
      });
    }
  };

  // Compose import handlers
  const handleComposeParse = async () => {
    if (!composeYaml.trim()) return;
    setComposeError("");
    setComposeParsed(null);
    try {
      const services = await api.post<ComposeService[]>("/apps/compose/parse", { yaml: composeYaml });
      setComposeParsed(services);
    } catch (e) {
      setComposeError(e instanceof Error ? e.message : "Failed to parse YAML");
    }
  };

  const handleComposeDeploy = async () => {
    if (!composeYaml.trim()) return;
    const stackName = composeName.trim() || `stack-${Date.now()}`;
    setComposeDeploying(true);
    setComposeError("");
    try {
      const result = await api.post<{ id: string; deploy_result: ComposeDeployResult }>("/stacks", {
        name: stackName,
        yaml: composeYaml,
      });
      const failed = result.deploy_result.services.filter(s => s.status === "failed");
      if (failed.length > 0) {
        setComposeError(`${failed.length} service(s) failed: ${failed.map(s => `${s.name}: ${s.error}`).join(", ")}`);
      } else {
        setShowCompose(false);
        setComposeYaml("");
        setComposeParsed(null);
        setComposeName("");
        setMessage({
          text: `Stack "${stackName}" deployed with ${result.deploy_result.services.length} service(s)`,
          type: "success",
        });
        loadApps();
      }
    } catch (e) {
      setComposeError(e instanceof Error ? e.message : "Deploy failed");
    } finally {
      setComposeDeploying(false);
    }
  };

  const handleStackAction = async (stackId: string, action: string) => {
    setStackActionLoading(`${stackId}-${action}`);
    try {
      if (action === "remove") {
        await api.delete(`/stacks/${stackId}`);
        setMessage({ text: "Stack removed", type: "success" });
      } else {
        await api.post(`/stacks/${stackId}/${action}`);
        setMessage({ text: `Stack ${action}ed`, type: "success" });
      }
      loadApps();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : `Stack ${action} failed`, type: "error" });
    } finally {
      setStackActionLoading(null);
    }
  };

  // Image management handlers (Feature #6)
  const loadImages = async () => {
    setImagesLoading(true);
    try {
      const data = await api.get<{ images: typeof images }>("/apps/images");
      setImages(data.images || []);
    } catch {
      setImages([]);
    } finally {
      setImagesLoading(false);
    }
  };

  const handlePruneImages = async () => {
    setImagePruning(true);
    try {
      await api.post("/apps/images/prune");
      setMessage({ text: "Unused images pruned", type: "success" });
      loadImages();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Prune failed", type: "error" });
    } finally {
      setImagePruning(false);
    }
  };

  const handleRemoveImage = async (imageId: string) => {
    try {
      await api.delete(`/apps/images/${encodeURIComponent(imageId)}`);
      setMessage({ text: "Image removed", type: "success" });
      loadImages();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Remove failed", type: "error" });
    }
  };

  // Container snapshot handler (Feature #12)
  const [snapshotTarget, setSnapshotTarget] = useState<{ id: string; name: string } | null>(null);
  const [snapshotTag, setSnapshotTag] = useState("");

  const handleSnapshot = async () => {
    if (!snapshotTarget || !snapshotTag) return;
    try {
      await api.post(`/apps/${snapshotTarget.id}/snapshot`, { tag: snapshotTag });
      setMessage({ text: `Snapshot saved as ${snapshotTag}`, type: "success" });
      setSnapshotTarget(null);
      setSnapshotTag("");
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Snapshot failed", type: "error" });
    }
  };

  // Container metrics history polling (Feature #8)
  const fetchAndRecordStats = async (containerId: string) => {
    try {
      const data = await api.get<{ cpu_percent?: string; memory_percent?: string }>(`/apps/${containerId}/stats`);
      const cpu = parseFloat(data.cpu_percent || '0');
      const mem = parseFloat(data.memory_percent || '0');
      setStatsHistory(prev => {
        const history = [...(prev[containerId] || []), { cpu, mem }].slice(-30);
        return { ...prev, [containerId]: history };
      });
    } catch { /* ignore */ }
  };

  // Poll stats history when stats panel is open
  useEffect(() => {
    if (!statsTarget) return;
    fetchAndRecordStats(statsTarget);
    const interval = setInterval(() => fetchAndRecordStats(statsTarget), 10000);
    return () => clearInterval(interval);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [statsTarget]);

  // Stack templates (Feature #11)
  const stackTemplates = [
    {
      id: "wordpress-mysql",
      name: "WordPress + MySQL",
      description: "Full WordPress stack with dedicated MySQL database",
      services: 2,
      yaml: `services:
  wordpress:
    image: wordpress:latest
    ports: ["8080:80"]
    environment:
      WORDPRESS_DB_HOST: db
      WORDPRESS_DB_USER: wordpress
      WORDPRESS_DB_PASSWORD: wp_password
      WORDPRESS_DB_NAME: wordpress
    depends_on: [db]
    labels:
      dockpanel.managed: "true"
      dockpanel.app.template: wordpress-package
      dockpanel.app.name: wordpress
  db:
    image: mysql:8
    environment:
      MYSQL_ROOT_PASSWORD: root_password
      MYSQL_DATABASE: wordpress
      MYSQL_USER: wordpress
      MYSQL_PASSWORD: wp_password
    volumes: [mysql_data:/var/lib/mysql]
    labels:
      dockpanel.managed: "true"
      dockpanel.app.template: wordpress-package
      dockpanel.app.name: wordpress-db
volumes:
  mysql_data:`,
    },
    {
      id: "ghost-mysql",
      name: "Ghost + MySQL",
      description: "Ghost CMS with dedicated MySQL database",
      services: 2,
      yaml: `services:
  ghost:
    image: ghost:latest
    ports: ["2368:2368"]
    environment:
      database__client: mysql
      database__connection__host: db
      database__connection__user: ghost
      database__connection__password: ghost_password
      database__connection__database: ghost
      url: http://localhost:2368
    depends_on: [db]
    labels:
      dockpanel.managed: "true"
  db:
    image: mysql:8
    environment:
      MYSQL_ROOT_PASSWORD: root_password
      MYSQL_DATABASE: ghost
      MYSQL_USER: ghost
      MYSQL_PASSWORD: ghost_password
    volumes: [ghost_mysql:/var/lib/mysql]
    labels:
      dockpanel.managed: "true"
volumes:
  ghost_mysql:`,
    },
    {
      id: "nextcloud-mariadb-redis",
      name: "Nextcloud + MariaDB + Redis",
      description: "Full Nextcloud stack with database and cache",
      services: 3,
      yaml: `services:
  nextcloud:
    image: nextcloud:latest
    ports: ["8081:80"]
    environment:
      MYSQL_HOST: db
      MYSQL_DATABASE: nextcloud
      MYSQL_USER: nextcloud
      MYSQL_PASSWORD: nc_password
      REDIS_HOST: redis
    depends_on: [db, redis]
    volumes: [nextcloud_data:/var/www/html]
    labels:
      dockpanel.managed: "true"
  db:
    image: mariadb:11
    environment:
      MYSQL_ROOT_PASSWORD: root_password
      MYSQL_DATABASE: nextcloud
      MYSQL_USER: nextcloud
      MYSQL_PASSWORD: nc_password
    volumes: [nc_mariadb:/var/lib/mysql]
    labels:
      dockpanel.managed: "true"
  redis:
    image: redis:7-alpine
    labels:
      dockpanel.managed: "true"
volumes:
  nextcloud_data:
  nc_mariadb:`,
    },
  ];

  // Filter apps by tag (Feature #7)
  const standaloneApps = apps.filter(a => !a.stack_id);
  const filteredApps = standaloneApps.filter(a => !tagFilter || appTags[a.container_id] === tagFilter);

  // Crashed apps detection (Feature #10)
  const crashedApps = apps.filter(a => a.status === 'exited' || a.status === 'dead');

  return (
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Docker Apps</h1>
          <p className="page-header-subtitle">One-click deploy popular applications</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => { setShowImages(true); loadImages(); }}
            className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg text-sm font-medium hover:bg-dark-600 flex items-center gap-2 border border-dark-500"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M20.25 7.5l-.625 10.632a2.25 2.25 0 01-2.247 2.118H6.622a2.25 2.25 0 01-2.247-2.118L3.75 7.5M10 11.25h4M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125z" />
            </svg>
            Images
          </button>
          <button
            onClick={() => { setShowRegistries(true); loadRegistries(); }}
            className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg text-sm font-medium hover:bg-dark-600 flex items-center gap-2 border border-dark-500"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M16.5 10.5V6.75a4.5 4.5 0 10-9 0v3.75m-.75 11.25h10.5a2.25 2.25 0 002.25-2.25v-6.75a2.25 2.25 0 00-2.25-2.25H6.75a2.25 2.25 0 00-2.25 2.25v6.75a2.25 2.25 0 002.25 2.25z" />
            </svg>
            Registries
          </button>
          <button
            onClick={() => setShowCompose(true)}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 flex items-center gap-2"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 14.25v-2.625a3.375 3.375 0 00-3.375-3.375h-1.5A1.125 1.125 0 0113.5 7.125v-1.5a3.375 3.375 0 00-3.375-3.375H8.25m6.75 12l-3-3m0 0l-3 3m3-3v6m-1.5-15H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 00-9-9z" />
            </svg>
            Import Compose
          </button>
        </div>
      </div>

      <div className="p-6 lg:p-8">

      {/* Deploy provisioning log */}
      {deployId && (
        <ProvisionLog
          sseUrl={`/api/apps/deploy/${deployId}/log`}
          onComplete={() => {
            setDeployId(null);
            setDeploying(false);
            loadApps();
          }}
        />
      )}

      {/* Update provisioning log */}
      {updateDeployId && (
        <ProvisionLog
          sseUrl={`/api/apps/deploy/${updateDeployId}/log`}
          onComplete={() => {
            setUpdateDeployId(null);
            loadApps();
          }}
        />
      )}

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
              : "bg-danger-500/10 text-danger-400 border-danger-500/20"
          }`}
        >
          {message.text}
        </div>
      )}

      {/* Crashed apps banner (Feature #10) */}
      {crashedApps.length > 0 && (
        <div className="bg-danger-500/10 border border-danger-500/20 rounded-lg p-4 mb-4">
          <div className="flex items-center gap-2">
            <svg className="w-5 h-5 text-danger-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126zM12 15.75h.007v.008H12v-.008z" />
            </svg>
            <span className="text-sm text-danger-400 font-medium">{crashedApps.length} container(s) crashed</span>
          </div>
          <div className="mt-2 space-y-1">
            {crashedApps.map(a => (
              <div key={a.container_id} className="flex items-center justify-between text-xs">
                <span className="text-dark-100 font-mono">{a.name} <span className="text-dark-300">({a.status})</span></span>
                <button onClick={() => handleAction(a.container_id, "start")} className="text-rust-400 hover:text-rust-300 font-medium">Restart</button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Compose Stacks */}
      {stacks.length > 0 && (
        <div className="mb-8">
          <h2 className="text-sm font-medium text-dark-200 uppercase font-mono tracking-widest mb-3">
            Compose Stacks
          </h2>
          <div className="space-y-3">
            {stacks.map((stack) => (
              <div key={stack.id} className="bg-dark-800 rounded-lg border border-dark-500">
                <div
                  className="flex items-center justify-between px-5 py-3 cursor-pointer hover:bg-dark-700/30 transition-colors"
                  onClick={() => setExpandedStack(expandedStack === stack.id ? null : stack.id)}
                >
                  <div className="flex items-center gap-3">
                    <svg className="w-4 h-4 text-dark-300" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M6.429 9.75L2.25 12l4.179 2.25m0-4.5l5.571 3 5.571-3m-11.142 0L2.25 7.5 12 2.25l9.75 5.25-4.179 2.25m0 0L12 12.75 6.429 9.75m11.142 0l4.179 2.25-4.179 2.25m0 0L12 17.25l-5.571-3m11.142 0l4.179 2.25L12 21.75l-9.75-5.25 4.179-2.25" />
                    </svg>
                    <span className="font-mono text-sm text-dark-50 font-medium">{stack.name}</span>
                    <span className={`px-2 py-0.5 rounded text-xs font-medium ${
                      stack.status === "running" ? "bg-rust-500/15 text-rust-400" :
                      stack.status === "stopped" ? "bg-dark-600 text-dark-300" :
                      stack.status === "partial" ? "bg-warn-500/15 text-warn-400" :
                      "bg-dark-600 text-dark-400"
                    }`}>
                      {stack.status === "running" ? `${stack.running}/${stack.total} running` :
                       stack.status === "stopped" ? "stopped" :
                       stack.status === "partial" ? `${stack.running}/${stack.total} running` :
                       "removed"}
                    </span>
                  </div>
                  <div className="flex items-center gap-2">
                    {stack.status !== "running" && stack.total > 0 && (
                      <button
                        onClick={(e) => { e.stopPropagation(); handleStackAction(stack.id, "start"); }}
                        disabled={stackActionLoading !== null}
                        className="px-2 py-1 text-xs bg-dark-700 text-dark-200 rounded hover:bg-dark-600 disabled:opacity-50"
                      >
                        {stackActionLoading === `${stack.id}-start` ? "..." : "Start"}
                      </button>
                    )}
                    {stack.status === "running" && (
                      <button
                        onClick={(e) => { e.stopPropagation(); handleStackAction(stack.id, "stop"); }}
                        disabled={stackActionLoading !== null}
                        className="px-2 py-1 text-xs bg-dark-700 text-dark-200 rounded hover:bg-dark-600 disabled:opacity-50"
                      >
                        {stackActionLoading === `${stack.id}-stop` ? "..." : "Stop"}
                      </button>
                    )}
                    {stack.total > 0 && (
                      <button
                        onClick={(e) => { e.stopPropagation(); handleStackAction(stack.id, "restart"); }}
                        disabled={stackActionLoading !== null}
                        className="px-2 py-1 text-xs bg-dark-700 text-dark-200 rounded hover:bg-dark-600 disabled:opacity-50"
                      >
                        {stackActionLoading === `${stack.id}-restart` ? "..." : "Restart"}
                      </button>
                    )}
                    <button
                      onClick={(e) => { e.stopPropagation(); handleStackAction(stack.id, "remove"); }}
                      disabled={stackActionLoading !== null}
                      className="px-2 py-1 text-xs text-danger-400 hover:text-danger-300 disabled:opacity-50"
                    >
                      {stackActionLoading === `${stack.id}-remove` ? "..." : "Remove"}
                    </button>
                    <svg className={`w-4 h-4 text-dark-400 transition-transform ${expandedStack === stack.id ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
                    </svg>
                  </div>
                </div>
                {expandedStack === stack.id && stack.services.length > 0 && (
                  <div className="border-t border-dark-600 px-5 py-3">
                    <table className="w-full">
                      <thead>
                        <tr className="text-left">
                          <th className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest pb-2">Service</th>
                          <th className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest pb-2 hidden sm:table-cell">Image</th>
                          <th className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest pb-2 w-20">Port</th>
                          <th className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest pb-2 w-24">Status</th>
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-dark-700">
                        {stack.services.map((svc) => (
                          <tr key={svc.container_id} className="text-sm">
                            <td className="py-1.5 font-mono text-dark-100">{svc.name}</td>
                            <td className="py-1.5 text-dark-300 text-xs font-mono hidden sm:table-cell">{svc.image}</td>
                            <td className="py-1.5 text-dark-200 font-mono">{svc.port || "\u2014"}</td>
                            <td className="py-1.5">
                              <span className={`text-xs font-medium ${svc.status === "running" ? "text-rust-400" : "text-dark-400"}`}>
                                {svc.status}
                              </span>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Deployed Apps (standalone, not in stacks) */}
      {standaloneApps.length > 0 && (
        <div className="mb-8">
          <div className="flex items-center justify-between mb-3">
            <h2 className="text-sm font-medium text-dark-200 uppercase font-mono tracking-widest">
              Running Apps
            </h2>
            <div className="flex items-center gap-2">
              <button
                onClick={handleCheckUpdates}
                disabled={updateChecking}
                className="px-3 py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-medium hover:bg-dark-600 border border-dark-500 disabled:opacity-50 transition-colors"
              >
                {updateChecking ? (
                  <span className="flex items-center gap-1.5">
                    <svg className="w-3 h-3 animate-spin" fill="none" viewBox="0 0 24 24"><circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" /><path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" /></svg>
                    Checking...
                  </span>
                ) : "Check for Updates"}
              </button>
              <select value={tagFilter} onChange={e => setTagFilter(e.target.value)} className="px-2 py-1.5 bg-dark-700 text-dark-200 rounded text-xs border border-dark-500">
                <option value="">All Apps</option>
                <option value="production">Production</option>
                <option value="staging">Staging</option>
                <option value="development">Development</option>
                <option value="internal">Internal</option>
              </select>
            </div>
          </div>
          <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
            <table className="w-full">
              <thead>
                <tr className="bg-dark-900 border-b border-dark-500">
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">App</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 hidden sm:table-cell">Template</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 hidden md:table-cell">Domain</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 hidden sm:table-cell w-20">Port</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 hidden lg:table-cell w-24">Health</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-24">Status</th>
                  <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">Actions</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-600">
                {filteredApps.map((app) => (
                  <tr key={app.container_id} className="hover:bg-dark-700/30 transition-colors">
                    <td className="px-5 py-4 text-sm text-dark-50 font-medium font-mono">
                      <div className="flex items-center gap-2">
                        {app.name}
                        {updateInfo[app.container_id]?.update_available && (
                          <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-accent-500/15 text-accent-400 animate-pulse" title="Newer image available">
                            <svg className="w-2.5 h-2.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}><path strokeLinecap="round" strokeLinejoin="round" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4-4m0 0l-4 4m4-4v12" /></svg>
                            Update
                          </span>
                        )}
                        <select
                          value={appTags[app.container_id] || ''}
                          onChange={e => {
                            const next = { ...appTags, [app.container_id]: e.target.value };
                            setAppTags(next);
                            localStorage.setItem('dp-app-tags', JSON.stringify(next));
                          }}
                          onClick={e => e.stopPropagation()}
                          className="px-1 py-0.5 bg-dark-700 text-dark-200 rounded text-[10px] border-0 cursor-pointer"
                        >
                          <option value="">No tag</option>
                          <option value="production">Production</option>
                          <option value="staging">Staging</option>
                          <option value="development">Development</option>
                          <option value="internal">Internal</option>
                        </select>
                      </div>
                    </td>
                    <td className="px-5 py-4 text-sm text-dark-200 hidden sm:table-cell">{app.template}</td>
                    <td className="px-5 py-4 text-sm hidden md:table-cell">
                      {app.domain ? (
                        <a href={`https://${app.domain}`} target="_blank" rel="noopener noreferrer" className="text-rust-400 hover:underline font-mono">{app.domain}</a>
                      ) : (
                        <span className="text-dark-300">{"\u2014"}</span>
                      )}
                    </td>
                    <td className="px-5 py-4 text-sm text-dark-200 font-mono hidden sm:table-cell">{app.port || "\u2014"}</td>
                    <td className="px-5 py-4 hidden lg:table-cell">
                      {app.health ? (
                        <span className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium ${
                          app.health === "healthy" ? "bg-rust-500/15 text-rust-400" :
                          app.health === "unhealthy" ? "bg-danger-500/15 text-danger-400" :
                          "bg-warn-500/15 text-warn-400"
                        }`}>
                          <span className={`w-1.5 h-1.5 rounded-full ${
                            app.health === "healthy" ? "bg-rust-400" :
                            app.health === "unhealthy" ? "bg-danger-400" :
                            "bg-warn-400"
                          }`} />
                          {app.health}
                        </span>
                      ) : (
                        <span className="text-dark-300 text-xs">{"\u2014"}</span>
                      )}
                    </td>
                    <td className="px-5 py-4">
                      <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${statusColors[app.status] || "bg-dark-700 text-dark-200"}`}>
                        {app.status}
                      </span>
                    </td>
                    <td className="px-5 py-4 text-right">
                      <div className="flex items-center justify-end gap-1 flex-wrap">
                        {app.status === "running" ? (
                          <button
                            onClick={() => handleAction(app.container_id, "stop")}
                            disabled={actionLoading === `${app.container_id}-stop`}
                            className="px-2 py-1 bg-warn-500/10 text-warn-400 rounded text-xs font-medium hover:bg-warn-400/15 disabled:opacity-50"
                          >
                            {actionLoading === `${app.container_id}-stop` ? "..." : "Stop"}
                          </button>
                        ) : (
                          <button
                            onClick={() => handleAction(app.container_id, "start")}
                            disabled={actionLoading === `${app.container_id}-start`}
                            className="px-2 py-1 bg-rust-500/10 text-rust-400 rounded text-xs font-medium hover:bg-rust-500/20 disabled:opacity-50"
                          >
                            {actionLoading === `${app.container_id}-start` ? "..." : "Start"}
                          </button>
                        )}
                        <button
                          onClick={() => handleAction(app.container_id, "restart")}
                          disabled={!!actionLoading}
                          className="px-2 py-1 bg-accent-500/10 text-accent-400 rounded text-xs font-medium hover:bg-accent-500/15 disabled:opacity-50"
                        >
                          {actionLoading === `${app.container_id}-restart` ? "..." : "Restart"}
                        </button>
                        <button
                          onClick={() => handleLogs(app.container_id)}
                          className={`px-2 py-1 rounded text-xs font-medium ${
                            logsTarget === app.container_id
                              ? "bg-dark-500/20 text-dark-100"
                              : "bg-dark-700 text-dark-300 hover:bg-dark-600"
                          }`}
                        >
                          Logs
                        </button>
                        <button
                          onClick={() => handleEnv(app.container_id)}
                          className="px-2 py-1 rounded text-xs font-medium bg-accent-600/10 text-accent-400 hover:bg-accent-600/15"
                        >
                          Env
                        </button>
                        <button
                          onClick={() => handleStats(app.container_id)}
                          className={`px-2 py-1 rounded text-xs font-medium ${
                            statsTarget === app.container_id
                              ? "bg-rust-500/20 text-rust-400"
                              : "bg-dark-700 text-dark-300 hover:bg-dark-600"
                          }`}
                        >
                          Stats
                        </button>
                        {app.status === "running" && (
                          <button
                            onClick={() => handleShell(app.container_id, app.name)}
                            className="px-2 py-1 rounded text-xs font-medium bg-dark-600 text-dark-100 hover:bg-dark-500"
                          >
                            Shell
                          </button>
                        )}
                        <button
                          onClick={() => handleVolumes(app.container_id)}
                          className="px-2 py-1 rounded text-xs font-medium bg-dark-700 text-dark-300 hover:bg-dark-600"
                        >
                          Volumes
                        </button>
                        {snapshotTarget?.id === app.container_id ? (
                          <span className="inline-flex items-center gap-1">
                            <input
                              type="text"
                              value={snapshotTag}
                              onChange={(e) => setSnapshotTag(e.target.value)}
                              onKeyDown={(e) => { if (e.key === "Enter") handleSnapshot(); if (e.key === "Escape") { setSnapshotTarget(null); setSnapshotTag(""); } }}
                              autoFocus
                              className="w-32 px-1.5 py-0.5 bg-dark-900 border border-dark-500 rounded text-xs font-mono text-dark-100"
                              placeholder="Tag name"
                            />
                            <button onClick={handleSnapshot} disabled={!snapshotTag} className="px-1.5 py-0.5 bg-rust-500 text-white rounded text-[10px] font-medium disabled:opacity-50">Save</button>
                            <button onClick={() => { setSnapshotTarget(null); setSnapshotTag(""); }} className="px-1.5 py-0.5 bg-dark-600 text-dark-200 rounded text-[10px]">Cancel</button>
                          </span>
                        ) : (
                          <button
                            onClick={() => { setSnapshotTarget({ id: app.container_id, name: app.name }); setSnapshotTag(`snapshot-${app.name}-${Date.now()}`); }}
                            className="px-2 py-1 rounded text-xs font-medium bg-dark-700 text-dark-300 hover:bg-dark-600"
                          >
                            Snapshot
                          </button>
                        )}
                        <button
                          onClick={() => handleUpdate(app.container_id)}
                          disabled={!!actionLoading}
                          className={`px-2 py-1 rounded text-xs font-medium disabled:opacity-50 ${
                            updateInfo[app.container_id]?.update_available
                              ? "bg-accent-500/20 text-accent-400 hover:bg-accent-500/30"
                              : "bg-dark-700 text-dark-300 hover:bg-dark-600"
                          }`}
                        >
                          {actionLoading === `${app.container_id}-update` ? "Updating..." : updateInfo[app.container_id]?.update_available ? "Update Now" : "Update"}
                        </button>
                        <button
                          onClick={async () => {
                            const cfg = await api.get<{ auto_sleep_enabled: boolean; sleep_after_minutes: number }>(`/apps/${app.container_id}/sleep-config`);
                            const newEnabled = !cfg.auto_sleep_enabled;
                            await api.put(`/apps/${app.container_id}/sleep-config`, {
                              auto_sleep_enabled: newEnabled,
                              sleep_after_minutes: cfg.sleep_after_minutes || 30,
                            });
                            setMessage({ text: `Auto-sleep ${newEnabled ? "enabled" : "disabled"}`, type: "success" });
                          }}
                          className="px-2 py-1 rounded text-xs font-medium bg-dark-700 text-dark-300 hover:bg-dark-600"
                          title="Toggle auto-sleep (stop container when idle)"
                        >
                          Sleep
                        </button>
                        {deleteTarget === app.container_id ? (
                          <>
                            <button onClick={() => handleRemove(app.container_id)} className="px-2 py-1 bg-danger-500 text-white rounded text-xs">Confirm</button>
                            <button onClick={() => setDeleteTarget(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs">Cancel</button>
                          </>
                        ) : (
                          <button onClick={() => setDeleteTarget(app.container_id)} className="px-2 py-1 bg-danger-500/10 text-danger-400 rounded text-xs font-medium hover:bg-danger-500/15">Remove</button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* Stats inline */}
          {statsTarget && (
            <div className="mt-3 bg-dark-800 rounded-lg border border-dark-500 p-4">
              <div className="flex items-center justify-between mb-2">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                  Resource Stats: {apps.find(a => a.container_id === statsTarget)?.name}
                </h3>
                <div className="flex items-center gap-2">
                  <button onClick={() => handleStats(statsTarget)} className="text-xs text-dark-300 hover:text-dark-100">Refresh</button>
                  <button onClick={() => { setStatsTarget(null); setStatsData(null); }} className="text-dark-300 hover:text-dark-50">
                    <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
                  </button>
                </div>
              </div>
              {statsLoading ? (
                <div className="flex items-center gap-2 text-dark-300 text-sm">
                  <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24"><circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" /><path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" /></svg>
                  Loading stats...
                </div>
              ) : statsData?.cpu_percent ? (
                <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
                  <div className="bg-dark-900 rounded-lg p-3 border border-dark-600">
                    <div className="text-[10px] text-dark-300 uppercase">CPU</div>
                    <div className="text-lg font-mono text-dark-50 font-medium">{statsData.cpu_percent}%</div>
                  </div>
                  <div className="bg-dark-900 rounded-lg p-3 border border-dark-600">
                    <div className="text-[10px] text-dark-300 uppercase">Memory</div>
                    <div className="text-sm font-mono text-dark-50">{statsData.memory_usage}</div>
                    <div className="text-[10px] text-dark-300">{statsData.memory_percent}%</div>
                  </div>
                  <div className="bg-dark-900 rounded-lg p-3 border border-dark-600">
                    <div className="text-[10px] text-dark-300 uppercase">Network I/O</div>
                    <div className="text-sm font-mono text-dark-50">{statsData.network_io}</div>
                  </div>
                  <div className="bg-dark-900 rounded-lg p-3 border border-dark-600">
                    <div className="text-[10px] text-dark-300 uppercase">Block I/O</div>
                    <div className="text-sm font-mono text-dark-50">{statsData.block_io}</div>
                  </div>
                  <div className="bg-dark-900 rounded-lg p-3 border border-dark-600">
                    <div className="text-[10px] text-dark-300 uppercase">PIDs</div>
                    <div className="text-lg font-mono text-dark-50 font-medium">{statsData.pids}</div>
                  </div>
                </div>
              ) : (
                <p className="text-sm text-dark-300">Container not running or stats unavailable</p>
              )}
              {/* Stats history sparkline (Feature #8) */}
              {statsTarget && (statsHistory[statsTarget] || []).length > 1 && (
                <div className="mt-3 grid grid-cols-1 md:grid-cols-2 gap-3">
                  <div className="bg-dark-900 rounded-lg p-3 border border-dark-600">
                    <div className="text-[10px] text-dark-300 uppercase mb-1">CPU History ({(statsHistory[statsTarget] || []).length} samples)</div>
                    <svg viewBox={`0 0 ${(statsHistory[statsTarget] || []).length * 10} 40`} className="w-full h-10" preserveAspectRatio="none">
                      <polyline
                        fill="none"
                        stroke="#e8956a"
                        strokeWidth="1.5"
                        points={(statsHistory[statsTarget] || []).map((p, i) => `${i * 10},${40 - (p.cpu / 100) * 40}`).join(' ')}
                      />
                    </svg>
                  </div>
                  <div className="bg-dark-900 rounded-lg p-3 border border-dark-600">
                    <div className="text-[10px] text-dark-300 uppercase mb-1">Memory History ({(statsHistory[statsTarget] || []).length} samples)</div>
                    <svg viewBox={`0 0 ${(statsHistory[statsTarget] || []).length * 10} 40`} className="w-full h-10" preserveAspectRatio="none">
                      <polyline
                        fill="none"
                        stroke="#7c3aed"
                        strokeWidth="1.5"
                        points={(statsHistory[statsTarget] || []).map((p, i) => `${i * 10},${40 - (p.mem / 100) * 40}`).join(' ')}
                      />
                    </svg>
                  </div>
                </div>
              )}
            </div>
          )}

          {/* Logs modal */}
          {logsTarget && (() => {
            const filtered = logSearch
              ? logLines.filter(l => l.toLowerCase().includes(logSearch.toLowerCase()))
              : logLines;
            return (
              <div
                className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4 dp-modal-overlay"
                role="dialog"
                aria-labelledby="logs-dialog-title"
                onKeyDown={(e) => { if (e.key === "Escape") closeLogs(); }}
              >
                <div className="bg-dark-800 rounded-lg shadow-xl w-full max-w-4xl max-h-[85vh] flex flex-col overflow-hidden border border-dark-500 dp-modal">
                  {/* Header */}
                  <div className="flex items-center justify-between px-5 py-3 border-b border-dark-600">
                    <div className="flex items-center gap-3">
                      <svg className="w-4 h-4 text-dark-200" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" /></svg>
                      <h3 id="logs-dialog-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                        {apps.find(a => a.container_id === logsTarget)?.name}
                      </h3>
                      {logAutoRefresh && (
                        <span className="flex items-center gap-1 text-[10px] text-rust-400">
                          <span className="w-1.5 h-1.5 rounded-full bg-rust-400 animate-pulse" />
                          Live
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-2">
                      <span className="text-[10px] text-dark-300">{filtered.length} lines</span>
                      <button onClick={closeLogs} className="text-dark-300 hover:text-dark-50 p-1">
                        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
                      </button>
                    </div>
                  </div>

                  {/* Toolbar */}
                  <div className="flex items-center gap-2 px-5 py-2 border-b border-dark-600 bg-dark-900/50">
                    <div className="relative flex-1 max-w-xs">
                      <svg className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-dark-300" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><circle cx="11" cy="11" r="8" /><path d="M21 21l-4.35-4.35" /></svg>
                      <input
                        type="text"
                        value={logSearch}
                        onChange={(e) => setLogSearch(e.target.value)}
                        placeholder="Filter logs..."
                        className="w-full pl-8 pr-3 py-1.5 bg-dark-800 border border-dark-500 rounded-lg text-xs text-dark-100 focus:ring-1 focus:ring-accent-500 focus:border-accent-500"
                      />
                    </div>
                    <button
                      onClick={() => setLogAutoRefresh(!logAutoRefresh)}
                      className={`px-2.5 py-1.5 rounded-lg text-xs font-medium flex items-center gap-1.5 ${
                        logAutoRefresh ? "bg-rust-500/15 text-rust-400" : "bg-dark-700 text-dark-200 hover:bg-dark-600"
                      }`}
                    >
                      <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" /></svg>
                      Auto
                    </button>
                    <button
                      onClick={() => setLogAutoScroll(!logAutoScroll)}
                      className={`px-2.5 py-1.5 rounded-lg text-xs font-medium flex items-center gap-1.5 ${
                        logAutoScroll ? "bg-accent-500/15 text-accent-400" : "bg-dark-700 text-dark-200 hover:bg-dark-600"
                      }`}
                    >
                      <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M19 14l-7 7m0 0l-7-7m7 7V3" /></svg>
                      Scroll
                    </button>
                    <button
                      onClick={() => logsTarget && fetchLogs(logsTarget)}
                      className="px-2.5 py-1.5 rounded-lg text-xs font-medium bg-dark-700 text-dark-200 hover:bg-dark-600"
                    >
                      Refresh
                    </button>
                    <button
                      onClick={downloadLogs}
                      className="px-2.5 py-1.5 rounded-lg text-xs font-medium bg-dark-700 text-dark-200 hover:bg-dark-600 flex items-center gap-1.5"
                    >
                      <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" /></svg>
                      Save
                    </button>
                  </div>

                  {/* Log content */}
                  <div className="flex-1 overflow-y-auto p-4 bg-dark-950 font-mono text-xs min-h-0">
                    {logLines.length === 0 ? (
                      <div className="flex items-center justify-center h-32 text-dark-300">
                        <svg className="w-5 h-5 animate-spin mr-2" fill="none" viewBox="0 0 24 24"><circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" /><path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" /></svg>
                        Loading logs...
                      </div>
                    ) : filtered.length === 0 ? (
                      <div className="flex items-center justify-center h-32 text-dark-300">
                        No matching log lines
                      </div>
                    ) : (
                      filtered.map((line, i) => {
                        const ll = line.toLowerCase();
                        const lvl = ll.includes("error") || ll.includes("fatal") || ll.includes("panic")
                          ? "text-danger-400"
                          : ll.includes("warn")
                          ? "text-warn-400"
                          : ll.includes("info")
                          ? "text-accent-400"
                          : "text-dark-200";
                        return (
                          <div key={i} className={`py-0.5 whitespace-pre-wrap break-all leading-relaxed ${lvl}`}>
                            <span className="select-none text-dark-300 mr-3 inline-block w-8 text-right">{i + 1}</span>
                            {logSearch ? (() => {
                              const idx = line.toLowerCase().indexOf(logSearch.toLowerCase());
                              if (idx === -1) return line;
                              return <>{line.slice(0, idx)}<mark className="bg-warn-500/30 text-warn-400 rounded px-0.5">{line.slice(idx, idx + logSearch.length)}</mark>{line.slice(idx + logSearch.length)}</>;
                            })() : line}
                          </div>
                        );
                      })
                    )}
                    <div ref={logEndRef} />
                  </div>
                </div>
              </div>
            );
          })()}
        </div>
      )}

      {/* Template Gallery */}
      <h2 className="text-sm font-medium text-dark-200 uppercase font-mono tracking-widest mb-3">
        App Templates
      </h2>
      {loading ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
          {[...Array(8)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-lg border border-dark-500 p-5 animate-pulse">
              <div className="h-5 bg-dark-700 rounded w-24 mb-2" />
              <div className="h-4 bg-dark-700 rounded w-full" />
            </div>
          ))}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
          {templates.length === 0 && (
            <div className="col-span-full py-12 text-center">
              <svg className="w-12 h-12 text-dark-500 mx-auto mb-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M21 7.5l-2.25-1.313M21 7.5v2.25m0-2.25l-2.25 1.313M3 7.5l2.25-1.313M3 7.5l2.25 1.313M3 7.5v2.25m9 3l2.25-1.313M12 12.75l-2.25-1.313M12 12.75V15m0 6.75l2.25-1.313M12 21.75V19.5m0 2.25l-2.25-1.313m0-16.875L12 2.25l2.25 1.313M21 14.25v2.25l-2.25 1.313m-13.5 0L3 16.5v-2.25" />
              </svg>
              <p className="text-dark-300 font-medium">Could not load app templates</p>
              <p className="text-dark-300 text-xs mt-1">Check that the agent is running and accessible</p>
            </div>
          )}
          {templates.map((tmpl) => (
            <button
              key={tmpl.id}
              onClick={() => openDeploy(tmpl)}
              className="bg-dark-800 rounded-lg border border-dark-500 p-5 text-left hover:border-indigo-300 hover:shadow-sm transition-all group card-interactive hover-lift"
            >
              <div className="flex items-center gap-3 mb-3">
                <div className={`w-10 h-10 rounded-lg flex items-center justify-center ${categoryColors[tmpl.category] || "bg-dark-700"}`}>
                  {appIcons[tmpl.id] || <span className="text-sm font-bold text-dark-200">{tmpl.name[0]}</span>}
                </div>
                <div>
                  <h3 className="text-sm font-semibold text-dark-50 group-hover:text-accent-400">
                    {tmpl.name}
                  </h3>
                  <span className="text-[10px] text-dark-300 uppercase">{tmpl.category}</span>
                </div>
              </div>
              <p className="text-xs text-dark-200 line-clamp-2">{tmpl.description}</p>
              <div className="flex items-center justify-between mt-3">
                <span className="text-[10px] text-dark-300 font-mono">{tmpl.image}</span>
                <span className="text-[10px] text-dark-300">:{tmpl.default_port}</span>
              </div>
            </button>
          ))}
        </div>
      )}

      {/* Deploy dialog */}
      {selected && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 p-4 dp-modal-overlay"
          role="dialog"
          aria-labelledby="deploy-dialog-title"
          onKeyDown={(e) => { if (e.key === "Escape") setSelected(null); }}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-[480px] max-h-[80vh] overflow-y-auto dp-modal">
            <h3 id="deploy-dialog-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-1">
              Deploy {selected.name}
            </h3>
            <p className="text-sm text-dark-200 mb-4">{selected.description}</p>

            <div className="space-y-4">
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Container Name</label>
                <input
                  type="text"
                  value={appName}
                  onChange={(e) => setAppName(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Host Port</label>
                <input
                  type="number"
                  value={appPort}
                  onChange={(e) => setAppPort(Number(e.target.value))}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                />
              </div>

              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">
                  Domain <span className="text-dark-300 font-normal">(optional — auto reverse proxy)</span>
                </label>
                <input
                  type="text"
                  value={appDomain}
                  onChange={(e) => setAppDomain(e.target.value)}
                  placeholder="app.example.com"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                />
              </div>

              {appDomain && (
                <div>
                  <label className="block text-sm font-medium text-dark-100 mb-1">
                    SSL Email <span className="text-dark-300 font-normal">(optional — Let's Encrypt)</span>
                  </label>
                  <input
                    type="email"
                    value={sslEmail}
                    onChange={(e) => setSslEmail(e.target.value)}
                    placeholder="you@example.com"
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                  />
                </div>
              )}

              {/* Resource Limits */}
              <div>
                <p className="text-sm font-medium text-dark-100 mb-2">Resource Limits <span className="text-dark-300 font-normal">(optional)</span></p>
                <div className="grid grid-cols-2 gap-3">
                  <div>
                    <label className="block text-xs text-dark-200 mb-0.5">Memory (MB)</label>
                    <input
                      type="number"
                      value={memoryMb}
                      onChange={(e) => setMemoryMb(e.target.value)}
                      placeholder="e.g. 512"
                      min="0"
                      className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                    />
                  </div>
                  <div>
                    <label className="block text-xs text-dark-200 mb-0.5">CPU (%)</label>
                    <input
                      type="number"
                      value={cpuPercent}
                      onChange={(e) => setCpuPercent(e.target.value)}
                      placeholder="e.g. 50"
                      min="0"
                      max="100"
                      className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                    />
                  </div>
                </div>
              </div>

              {/* GPU Passthrough */}
              <label className="flex items-center gap-2 text-sm font-medium text-dark-100 mt-2">
                <input type="checkbox" checked={gpuEnabled} onChange={e => setGpuEnabled(e.target.checked)} className="rounded border-dark-500" />
                Enable GPU passthrough <span className="text-dark-300 font-normal text-xs">(requires NVIDIA Container Toolkit)</span>
              </label>

              {/* Health Check Configuration (Feature #9) */}
              <div>
                <label className="flex items-center gap-2 text-sm font-medium text-dark-100">
                  <input type="checkbox" checked={enableHealthCheck} onChange={e => setEnableHealthCheck(e.target.checked)} className="rounded border-dark-500" />
                  Configure health check <span className="text-dark-300 font-normal text-xs">(optional)</span>
                </label>
                {enableHealthCheck && (
                  <div className="mt-2 space-y-2">
                    <input
                      type="text"
                      value={healthCmd}
                      onChange={e => setHealthCmd(e.target.value)}
                      placeholder="CMD curl -f http://localhost/ || exit 1"
                      className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-xs font-mono focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                    />
                    <div className="flex gap-2">
                      <input
                        type="number"
                        value={healthInterval}
                        onChange={e => setHealthInterval(e.target.value)}
                        placeholder="Interval (s)"
                        min="1"
                        className="w-1/3 px-2 py-1.5 border border-dark-500 rounded-lg text-xs focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                      />
                      <input
                        type="number"
                        value={healthTimeout}
                        onChange={e => setHealthTimeout(e.target.value)}
                        placeholder="Timeout (s)"
                        min="1"
                        className="w-1/3 px-2 py-1.5 border border-dark-500 rounded-lg text-xs focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                      />
                      <input
                        type="number"
                        value={healthRetries}
                        onChange={e => setHealthRetries(e.target.value)}
                        placeholder="Retries"
                        min="0"
                        className="w-1/3 px-2 py-1.5 border border-dark-500 rounded-lg text-xs focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                      />
                    </div>
                    <p className="text-[10px] text-dark-300">Docker health check command, interval/timeout in seconds, and retry count.</p>
                  </div>
                )}
              </div>

              {selected.env_vars.length > 0 && (
                <div>
                  <p className="text-sm font-medium text-dark-100 mb-2">Environment Variables</p>
                  <div className="space-y-2">
                    {selected.env_vars.map((v) => (
                      <div key={v.name}>
                        <label className="block text-xs text-dark-200 mb-0.5">
                          {v.label} {v.required && <span className="text-danger-500">*</span>}
                        </label>
                        <input
                          type={v.secret ? "password" : "text"}
                          value={envValues[v.name] || ""}
                          onChange={(e) =>
                            setEnvValues({ ...envValues, [v.name]: e.target.value })
                          }
                          placeholder={v.default || v.name}
                          className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                        />
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>

            <div className="flex justify-end gap-2 mt-6">
              <button
                onClick={() => setSelected(null)}
                className="px-4 py-2 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400"
              >
                Cancel
              </button>
              <button
                onClick={handleDeploy}
                disabled={deploying || !appName}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {deploying ? "Deploying..." : "Deploy"}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Env Vars Modal */}
      {envTarget && (
        <div
          className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4 dp-modal-overlay"
          role="dialog"
          aria-labelledby="env-dialog-title"
          onKeyDown={(e) => { if (e.key === "Escape") { setEnvTarget(null); setEnvEditing(false); }}}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-lg max-h-[80vh] overflow-y-auto border border-dark-500 dp-modal">
            <div className="flex items-center justify-between mb-4">
              <h3 id="env-dialog-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                Environment Variables
              </h3>
              <div className="flex items-center gap-2">
                {!envEditing && envVars.length > 0 && (
                  <button
                    onClick={() => {
                      setEnvEditing(true);
                      const vals: Record<string, string> = {};
                      envVars.forEach(ev => { vals[ev.key] = ev.value; });
                      setEnvEditValues(vals);
                    }}
                    className="px-2 py-1 text-xs font-medium bg-accent-600/15 text-accent-400 rounded hover:bg-accent-600/25"
                  >
                    Edit
                  </button>
                )}
                <button onClick={() => { setEnvTarget(null); setEnvEditing(false); }} className="text-dark-300 hover:text-dark-50">
                  <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
                </button>
              </div>
            </div>
            <p className="text-xs text-dark-300 mb-3">
              {apps.find(a => a.container_id === envTarget)?.name}
            </p>
            {envEditing && (
              <div className="mb-3 px-3 py-2 bg-warn-500/10 border border-warn-500/20 rounded-lg">
                <p className="text-xs text-warn-400">Container will be recreated with the new environment variables. This causes brief downtime.</p>
              </div>
            )}
            {envLoading ? (
              <div className="flex items-center justify-center py-8 text-dark-300">
                <svg className="w-5 h-5 animate-spin mr-2" fill="none" viewBox="0 0 24 24"><circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" /><path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" /></svg>
                Loading...
              </div>
            ) : envVars.length === 0 ? (
              <div className="text-center py-8 text-dark-300 text-sm">No environment variables set</div>
            ) : envEditing ? (
              <div className="space-y-2">
                {envVars.map((ev, i) => (
                  <div key={i} className="flex gap-2 items-start">
                    <div className="flex-1 min-w-0">
                      <div className="text-xs font-mono text-dark-200 truncate">{ev.key}</div>
                      <input
                        type="text"
                        value={envEditValues[ev.key] || ""}
                        onChange={(e) => setEnvEditValues({ ...envEditValues, [ev.key]: e.target.value })}
                        className="w-full text-xs font-mono text-dark-100 bg-dark-900 rounded px-2 py-1.5 mt-0.5 border border-dark-600 focus:ring-1 focus:ring-purple-500 focus:border-purple-500"
                      />
                    </div>
                  </div>
                ))}
                <div className="flex justify-end gap-2 mt-4 pt-3 border-t border-dark-600">
                  <button onClick={() => setEnvEditing(false)} className="px-3 py-1.5 text-xs text-dark-300 border border-dark-600 rounded hover:text-dark-100">
                    Cancel
                  </button>
                  <button
                    onClick={handleEnvSave}
                    disabled={envSaving}
                    className="px-3 py-1.5 text-xs bg-accent-600 text-white rounded hover:bg-accent-700 disabled:opacity-50"
                  >
                    {envSaving ? "Saving..." : "Save & Recreate"}
                  </button>
                </div>
              </div>
            ) : (
              <div className="space-y-2">
                {envVars.map((ev, i) => (
                  <div key={i} className="flex gap-2 items-start">
                    <div className="flex-1 min-w-0">
                      <div className="text-xs font-mono text-dark-200 truncate">{ev.key}</div>
                      <div className="text-xs font-mono text-dark-100 break-all bg-dark-900 rounded px-2 py-1 mt-0.5 border border-dark-600">
                        {ev.key.toLowerCase().includes("password") || ev.key.toLowerCase().includes("secret") || ev.key.toLowerCase().includes("token") || ev.key.toLowerCase().includes("key")
                          ? "\u2022\u2022\u2022\u2022\u2022\u2022\u2022\u2022"
                          : ev.value}
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            )}
            {/* Volumes */}
            {!envEditing && (() => {
              const app = apps.find(a => a.container_id === envTarget);
              return app && app.volumes && app.volumes.length > 0 ? (
                <div className="mt-4 pt-3 border-t border-dark-600">
                  <p className="text-xs font-medium text-dark-200 mb-2">Persistent Volumes</p>
                  <div className="space-y-1.5">
                    {app.volumes.map((v, i) => (
                      <div key={i} className="text-xs font-mono text-dark-100 bg-dark-900 rounded px-2 py-1.5 border border-dark-600 flex items-center gap-2">
                        <svg className="w-3.5 h-3.5 text-dark-300 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M5 8h14M5 8a2 2 0 110-4h14a2 2 0 110 4M5 8v10a2 2 0 002 2h10a2 2 0 002-2V8m-9 4h4" /></svg>
                        {v}
                      </div>
                    ))}
                  </div>
                </div>
              ) : null;
            })()}
            {!envEditing && (
              <div className="mt-4 pt-3 border-t border-dark-600">
                <p className="text-[10px] text-dark-300">
                  Click "Edit" to modify environment variables. The container will be recreated with updated values.
                </p>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Container Shell Modal */}
      {shellContainer && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4 dp-modal-overlay">
          <div className="bg-dark-800 rounded-lg border border-dark-500 w-full max-w-2xl max-h-[80vh] flex flex-col dp-modal">
            <div className="px-4 py-3 border-b border-dark-600 flex justify-between items-center">
              <div className="flex items-center gap-2">
                <svg className="w-4 h-4 text-dark-200" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" /></svg>
                <h3 className="text-sm font-medium text-dark-100 font-mono">{shellContainer.name}</h3>
                <span className="text-[10px] text-dark-300">30s timeout per command</span>
              </div>
              <button onClick={() => setShellContainer(null)} className="text-dark-300 hover:text-dark-100">
                <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
              </button>
            </div>
            <div ref={shellOutputRef} className="flex-1 overflow-y-auto p-4 bg-dark-950 font-mono text-xs text-dark-200 whitespace-pre-wrap min-h-[300px]">
              {shellOutput || "Type a command and press Enter.\n"}
            </div>
            <div className="px-4 py-3 border-t border-dark-600 flex gap-2 items-center">
              <span className="text-rust-400 font-mono text-sm">$</span>
              <input
                value={shellCmd}
                onChange={(e) => setShellCmd(e.target.value)}
                onKeyDown={async (e) => {
                  if (e.key === "Enter" && shellCmd.trim()) {
                    const cmd = shellCmd;
                    setShellCmd("");
                    setShellOutput((prev) => prev + `$ ${cmd}\n`);
                    try {
                      const result = await api.post<{ stdout?: string; stderr?: string; exit_code?: number }>(`/apps/${shellContainer.id}/exec`, { command: cmd });
                      setShellOutput((prev) => prev + (result.stdout || "") + (result.stderr ? result.stderr : "") + "\n");
                    } catch (err) {
                      setShellOutput((prev) => prev + `Error: ${err}\n`);
                    }
                  }
                }}
                placeholder="Enter command..."
                autoFocus
                className="flex-1 bg-transparent text-dark-100 font-mono text-sm outline-none"
              />
            </div>
          </div>
        </div>
      )}

      {/* Volume Browser Modal */}
      {volumeTarget && (
        <div
          className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4 dp-modal-overlay"
          onKeyDown={(e) => { if (e.key === "Escape") setVolumeTarget(null); }}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-2xl max-h-[80vh] overflow-y-auto border border-dark-500 dp-modal">
            <div className="flex items-center justify-between mb-4">
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                Volume Browser: {apps.find(a => a.container_id === volumeTarget)?.name}
              </h3>
              <button onClick={() => setVolumeTarget(null)} className="text-dark-300 hover:text-dark-50">
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
              </button>
            </div>
            {volumeLoading ? (
              <div className="flex items-center justify-center py-8 text-dark-300">
                <svg className="w-5 h-5 animate-spin mr-2" fill="none" viewBox="0 0 24 24"><circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" /><path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" /></svg>
                Loading volumes...
              </div>
            ) : volumeData.length === 0 ? (
              <div className="text-center py-8 text-dark-300 text-sm">No volumes mounted</div>
            ) : (
              <div className="space-y-4">
                {volumeData.map((vol, i) => (
                  <div key={i} className="bg-dark-900 rounded-lg border border-dark-600 p-4">
                    <div className="flex items-center justify-between mb-2">
                      <div className="flex items-center gap-2">
                        <svg className="w-4 h-4 text-warn-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M5 8h14M5 8a2 2 0 110-4h14a2 2 0 110 4M5 8v10a2 2 0 002 2h10a2 2 0 002-2V8m-9 4h4" /></svg>
                        <span className="text-xs font-mono text-dark-100">{vol.destination}</span>
                      </div>
                      <div className="flex items-center gap-2">
                        <span className={`px-1.5 py-0.5 text-[10px] rounded ${vol.type === "bind" ? "bg-accent-500/15 text-accent-400" : "bg-accent-600/15 text-accent-400"}`}>
                          {vol.type}
                        </span>
                        <span className="text-xs text-dark-300">{vol.size_mb} MB</span>
                      </div>
                    </div>
                    <div className="text-[10px] text-dark-300 mb-2 font-mono">{vol.source}</div>
                    {vol.listing && (
                      <pre className="text-[10px] font-mono text-dark-200 bg-dark-800 rounded p-2 overflow-x-auto max-h-40 overflow-y-auto border border-dark-700">
                        {vol.listing}
                      </pre>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Private Registries Section */}
      {showRegistries && (
        <div
          className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4 dp-modal-overlay"
          onKeyDown={(e) => { if (e.key === "Escape") setShowRegistries(false); }}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-lg max-h-[80vh] overflow-y-auto border border-dark-500 dp-modal">
            <div className="flex items-center justify-between mb-4">
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                Private Registries
              </h3>
              <button onClick={() => setShowRegistries(false)} className="text-dark-300 hover:text-dark-50">
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
              </button>
            </div>

            {/* Configured registries */}
            {registries.length > 0 && (
              <div className="mb-4">
                <p className="text-xs font-medium text-dark-200 mb-2">Configured Registries</p>
                <div className="space-y-1.5">
                  {registries.map((reg) => (
                    <div key={reg} className="flex items-center justify-between bg-dark-900 rounded px-3 py-2 border border-dark-600">
                      <span className="text-xs font-mono text-dark-100">{reg}</span>
                      <button
                        onClick={() => handleRegistryLogout(reg)}
                        className="text-xs text-danger-400 hover:text-danger-300"
                      >
                        Logout
                      </button>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Login form */}
            <div className="border-t border-dark-600 pt-4">
              <p className="text-xs font-medium text-dark-200 mb-3">Add Registry</p>
              <div className="space-y-3">
                <input
                  type="text"
                  value={regServer}
                  onChange={(e) => setRegServer(e.target.value)}
                  placeholder="registry.example.com"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                />
                <input
                  type="text"
                  value={regUsername}
                  onChange={(e) => setRegUsername(e.target.value)}
                  placeholder="Username"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                />
                <input
                  type="password"
                  value={regPassword}
                  onChange={(e) => setRegPassword(e.target.value)}
                  placeholder="Password or token"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                />
                <button
                  onClick={handleRegistryLogin}
                  disabled={regLoading || !regServer || !regUsername || !regPassword}
                  className="w-full px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                >
                  {regLoading ? "Logging in..." : "Login"}
                </button>
              </div>
              <p className="text-[10px] text-dark-300 mt-3">
                Registry credentials persist on the server. Images from private registries can be used in templates and compose deployments.
              </p>
            </div>
          </div>
        </div>
      )}

      {/* Docker Images Modal (Feature #6) */}
      {showImages && (
        <div
          className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4 dp-modal-overlay"
          onKeyDown={(e) => { if (e.key === "Escape") setShowImages(false); }}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-3xl max-h-[80vh] overflow-y-auto border border-dark-500 dp-modal">
            <div className="flex items-center justify-between mb-4">
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                Docker Images
              </h3>
              <div className="flex items-center gap-2">
                <button
                  onClick={handlePruneImages}
                  disabled={imagePruning}
                  className="px-3 py-1.5 text-xs font-medium bg-warn-500/10 text-warn-400 rounded hover:bg-warn-500/20 disabled:opacity-50"
                >
                  {imagePruning ? "Pruning..." : "Prune Unused"}
                </button>
                <button
                  onClick={loadImages}
                  className="px-3 py-1.5 text-xs font-medium bg-dark-700 text-dark-200 rounded hover:bg-dark-600"
                >
                  Refresh
                </button>
                <button onClick={() => setShowImages(false)} className="text-dark-300 hover:text-dark-50">
                  <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
                </button>
              </div>
            </div>
            {imagesLoading ? (
              <div className="flex items-center justify-center py-8 text-dark-300">
                <svg className="w-5 h-5 animate-spin mr-2" fill="none" viewBox="0 0 24 24"><circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" /><path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" /></svg>
                Loading images...
              </div>
            ) : images.length === 0 ? (
              <div className="text-center py-8 text-dark-300 text-sm">No Docker images found</div>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full">
                  <thead>
                    <tr className="border-b border-dark-600">
                      <th className="text-left text-xs font-medium text-dark-300 uppercase font-mono tracking-widest pb-2">Repository</th>
                      <th className="text-left text-xs font-medium text-dark-300 uppercase font-mono tracking-widest pb-2">Tag</th>
                      <th className="text-left text-xs font-medium text-dark-300 uppercase font-mono tracking-widest pb-2">Size</th>
                      <th className="text-left text-xs font-medium text-dark-300 uppercase font-mono tracking-widest pb-2">Created</th>
                      <th className="text-right text-xs font-medium text-dark-300 uppercase font-mono tracking-widest pb-2"></th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-700">
                    {images.map((img, i) => (
                      <tr key={i} className="hover:bg-dark-700/30">
                        <td className="py-2 text-xs font-mono text-dark-100">{img.repository}</td>
                        <td className="py-2 text-xs font-mono text-dark-200">{img.tag}</td>
                        <td className="py-2 text-xs text-dark-200">{img.size}</td>
                        <td className="py-2 text-xs text-dark-300">{img.created}</td>
                        <td className="py-2 text-right">
                          <button
                            onClick={() => handleRemoveImage(img.id)}
                            className="text-xs text-danger-400 hover:text-danger-300"
                          >
                            Remove
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <p className="text-[10px] text-dark-300 mt-3">{images.length} image(s) total</p>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Compose Import Dialog */}
      {showCompose && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 p-4 dp-modal-overlay"
          role="dialog"
          aria-labelledby="compose-dialog-title"
          onKeyDown={(e) => { if (e.key === "Escape") { setShowCompose(false); setComposeParsed(null); setComposeError(""); }}}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-2xl max-h-[85vh] overflow-y-auto dp-modal">
            <div className="flex items-center justify-between mb-4">
              <h3 id="compose-dialog-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                Deploy Multi-Container
              </h3>
              <div className="flex items-center gap-1 bg-dark-900 rounded-lg p-0.5">
                <button
                  onClick={() => setComposeView('compose')}
                  className={`px-3 py-1.5 rounded text-xs font-medium transition-colors ${composeView === 'compose' ? 'bg-rust-500/15 text-rust-400' : 'text-dark-300 hover:text-dark-200'}`}
                >
                  Compose
                </button>
                <button
                  onClick={() => setComposeView('packages')}
                  className={`px-3 py-1.5 rounded text-xs font-medium transition-colors ${composeView === 'packages' ? 'bg-rust-500/15 text-rust-400' : 'text-dark-300 hover:text-dark-200'}`}
                >
                  Packages
                </button>
              </div>
            </div>

            {/* Packages view (Feature #11) */}
            {composeView === 'packages' && (
              <div>
                <p className="text-sm text-dark-200 mb-4">
                  Pre-configured multi-container stacks. Click to customize and deploy.
                </p>
                <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                  {stackTemplates.map(pkg => (
                    <div
                      key={pkg.id}
                      className="bg-dark-900 rounded-lg border border-dark-500 p-4 hover:border-dark-400 cursor-pointer transition-colors"
                      onClick={() => {
                        // Replace placeholder passwords with random ones for security
                        const randPw = () => crypto.randomUUID().replace(/-/g, '').slice(0, 20);
                        const yaml = pkg.yaml
                          .replace(/wp_password/g, randPw())
                          .replace(/root_password/g, randPw())
                          .replace(/ghost_password/g, randPw())
                          .replace(/nc_password/g, randPw());
                        setComposeYaml(yaml); setComposeView('compose'); setComposeParsed(null);
                      }}
                    >
                      <h4 className="text-sm font-medium text-dark-50">{pkg.name}</h4>
                      <p className="text-xs text-dark-300 mt-1">{pkg.description}</p>
                      <span className="text-[10px] text-dark-400 mt-2 inline-block">{pkg.services} services</span>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Compose view (original) */}
            {composeView === 'compose' && (<>
            <p className="text-sm text-dark-200 mb-4">
              Paste your docker-compose.yml to parse and deploy services. Only services with an <code className="text-xs bg-dark-700 px-1 rounded">image:</code> field are supported (no build context).
            </p>

            {composeError && (
              <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-danger-500/10 text-danger-400 border-danger-500/20">
                {composeError}
              </div>
            )}

            <div className="mb-4">
              <label htmlFor="compose-name" className="block text-sm font-medium text-dark-100 mb-1">Stack Name</label>
              <input
                id="compose-name"
                type="text"
                value={composeName}
                onChange={(e) => setComposeName(e.target.value)}
                placeholder="my-stack"
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
              />
              <p className="text-xs text-dark-300 mt-1">Name for this stack (used for grouping services)</p>
            </div>

            <div className="mb-4">
              <label htmlFor="compose-yaml" className="block text-sm font-medium text-dark-100 mb-1">docker-compose.yml</label>
              <textarea
                id="compose-yaml"
                value={composeYaml}
                onChange={(e) => { setComposeYaml(e.target.value); setComposeParsed(null); }}
                rows={12}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                placeholder={`version: "3.8"\nservices:\n  web:\n    image: nginx:latest\n    ports:\n      - "8080:80"\n  redis:\n    image: redis:7-alpine\n    ports:\n      - "6379:6379"`}
                spellCheck={false}
              />
            </div>

            {!composeParsed && (
              <div className="flex justify-end gap-2">
                <button
                  onClick={() => { setShowCompose(false); setComposeYaml(""); setComposeParsed(null); setComposeError(""); }}
                  className="px-4 py-2 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400"
                >
                  Cancel
                </button>
                <button
                  onClick={handleComposeParse}
                  disabled={!composeYaml.trim()}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                >
                  Parse & Preview
                </button>
              </div>
            )}

            {/* Parsed services preview */}
            {composeParsed && (
              <>
                <div className="mb-4">
                  <h4 className="text-sm font-medium text-dark-50 mb-2">
                    {composeParsed.length} service{composeParsed.length !== 1 ? "s" : ""} found:
                  </h4>
                  <div className="space-y-2">
                    {composeParsed.map((svc) => (
                      <div key={svc.name} className="bg-dark-900 rounded-lg p-3 border border-dark-500">
                        <div className="flex items-center justify-between">
                          <div className="flex items-center gap-2">
                            <span className="text-sm font-semibold text-dark-50">{svc.name}</span>
                            <span className="text-xs text-dark-300 font-mono">{svc.image}</span>
                          </div>
                          {svc.restart !== "no" && (
                            <span className="text-[10px] text-dark-300">restart: {svc.restart}</span>
                          )}
                        </div>
                        {svc.ports.length > 0 && (
                          <div className="mt-1.5 flex gap-2 flex-wrap">
                            {svc.ports.map((p, i) => (
                              <span key={i} className="px-1.5 py-0.5 bg-accent-500/10 text-accent-400 rounded text-[10px] font-mono">
                                {p.host}:{p.container}/{p.protocol}
                              </span>
                            ))}
                          </div>
                        )}
                        {Object.keys(svc.environment).length > 0 && (
                          <div className="mt-1.5 flex gap-2 flex-wrap">
                            {Object.entries(svc.environment).slice(0, 5).map(([k, v]) => (
                              <span key={k} className="px-1.5 py-0.5 bg-accent-600/10 text-accent-400 rounded text-[10px] font-mono">
                                {k}={v.length > 20 ? v.slice(0, 20) + "..." : v}
                              </span>
                            ))}
                            {Object.keys(svc.environment).length > 5 && (
                              <span className="text-[10px] text-dark-300">+{Object.keys(svc.environment).length - 5} more</span>
                            )}
                          </div>
                        )}
                        {svc.volumes.length > 0 && (
                          <div className="mt-1.5 flex gap-2 flex-wrap">
                            {svc.volumes.map((v, i) => (
                              <span key={i} className="px-1.5 py-0.5 bg-warn-500/10 text-warn-500 rounded text-[10px] font-mono">
                                {v}
                              </span>
                            ))}
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                </div>

                <div className="flex justify-end gap-2">
                  <button
                    onClick={() => { setComposeParsed(null); setComposeError(""); }}
                    className="px-4 py-2 text-sm text-dark-200"
                  >
                    Edit YAML
                  </button>
                  <button
                    onClick={handleComposeDeploy}
                    disabled={composeDeploying}
                    className="px-4 py-2 bg-rust-600 text-white rounded-lg text-sm font-medium hover:bg-rust-700 disabled:opacity-50 flex items-center gap-2"
                  >
                    {composeDeploying && (
                      <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
                        <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                        <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                      </svg>
                    )}
                    {composeDeploying ? "Deploying..." : `Deploy ${composeParsed.length} Service${composeParsed.length !== 1 ? "s" : ""}`}
                  </button>
                </div>
              </>
            )}
            </>)}
          </div>
        </div>
      )}
      </div>
    </div>
  );
}
