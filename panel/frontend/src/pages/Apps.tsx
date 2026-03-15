import { useState, useEffect, useRef, useCallback, type ReactNode } from "react";
import { api } from "../api";

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
  CMS: "bg-blue-500/15",
  Database: "bg-rust-500/15",
  Monitoring: "bg-purple-500/15",
  DevOps: "bg-warn-500/15",
  Automation: "bg-pink-500/15",
  Tools: "bg-slate-500/15",
  Development: "bg-cyan-500/15",
  Analytics: "bg-indigo-500/15",
  Storage: "bg-sky-500/15",
  Security: "bg-rose-500/15",
  Media: "bg-violet-500/15",
  Networking: "bg-teal-500/15",
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
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#43ef82" width="128" height="128"/><path fill="#fff" d="M64 20c-22 0-40 7.2-40 16v56c0 8.8 18 16 40 16s40-7.2 40-16V36c0-8.8-18-16-40-16z" opacity=".15"/><ellipse fill="#fff" cx="64" cy="36" rx="36" ry="12" opacity=".9"/><path fill="#fff" d="M28 48c0 6.6 16.1 12 36 12s36-5.4 36-12" opacity=".6" stroke="#fff" strokeWidth="2" fill="none"/><path fill="#fff" d="M28 68c0 6.6 16.1 12 36 12s36-5.4 36-12" opacity=".4" stroke="#fff" strokeWidth="2" fill="none"/><path fill="none" stroke="#fff" strokeWidth="2" opacity=".3" d="M28 36v56M100 36v56"/></svg>
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
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#efb043" width="128" height="128"/><rect fill="#fff" x="20" y="36" width="88" height="56" rx="8" opacity=".2"/><path fill="#fff" d="M24 40l40 28 40-28v8L68 76c-2 1.5-6 1.5-8 0L24 48v-8z" opacity=".9"/><path fill="none" stroke="#fff" strokeWidth="4" opacity=".6" d="M24 40l40 28 40-28"/></svg>
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
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#fb923c" width="128" height="128"/><circle fill="#fff" cx="64" cy="48" r="20" opacity=".9"/><circle fill="#fb923c" cx="58" cy="44" r="4"/><circle fill="#fb923c" cx="70" cy="44" r="4"/><path fill="#fff" d="M44 72c0 16 8 28 20 32 12-4 20-16 20-32H44z" opacity=".7"/><path fill="#fb923c" d="M56 52c2 3 4.8 4 8 4s6-1 8-4" stroke="#fb923c" strokeWidth="2" fill="none" strokeLinecap="round"/></svg>
  ),
  // ─── Business / Productivity ──────────────────────────────────
  "invoice-ninja": (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#2f2e2e" width="128" height="128"/><rect fill="#fff" x="28" y="24" width="72" height="80" rx="4" opacity=".15"/><path fill="#fff" d="M40 36h48v4H40zm0 12h48v2H40zm0 8h48v2H40zm0 8h32v2H40z" opacity=".7"/><path fill="#fff" d="M40 80h20v12H40z" opacity=".4"/><path fill="#51e898" d="M72 76l8 8 16-20" stroke="#51e898" strokeWidth="4" fill="none" strokeLinecap="round" strokeLinejoin="round"/></svg>
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
  exited: "bg-red-500/15 text-red-400",
  created: "bg-warn-500/15 text-warn-400",
  paused: "bg-dark-700 text-dark-200",
};

export default function Apps() {
  const [templates, setTemplates] = useState<AppTemplate[]>([]);
  const [apps, setApps] = useState<DeployedApp[]>([]);
  const [loading, setLoading] = useState(true);
  const [deploying, setDeploying] = useState(false);
  const [selected, setSelected] = useState<AppTemplate | null>(null);
  const [appName, setAppName] = useState("");
  const [appPort, setAppPort] = useState(0);
  const [envValues, setEnvValues] = useState<Record<string, string>>({});
  const [appDomain, setAppDomain] = useState("");
  const [sslEmail, setSslEmail] = useState("");
  const [memoryMb, setMemoryMb] = useState("");
  const [cpuPercent, setCpuPercent] = useState("");
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

  // Compose import state
  const [showCompose, setShowCompose] = useState(false);
  const [composeYaml, setComposeYaml] = useState("");
  const [composeParsed, setComposeParsed] = useState<ComposeService[] | null>(null);
  const [composeDeploying, setComposeDeploying] = useState(false);
  const [composeError, setComposeError] = useState("");

  useEffect(() => {
    Promise.all([
      api.get<AppTemplate[]>("/apps/templates").catch(() => []),
      api.get<DeployedApp[]>("/apps").catch(() => []),
    ]).then(([tmpl, deployed]) => {
      setTemplates(tmpl);
      setApps(deployed);
      setLoading(false);
    });
  }, []);

  const loadApps = () => {
    api.get<DeployedApp[]>("/apps").then(setApps).catch((e) => console.error("Failed to load apps:", e));
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
      await api.post("/apps/deploy", {
        template_id: selected.id,
        name: appName,
        port: appPort,
        env: envValues,
        ...(appDomain ? { domain: appDomain } : {}),
        ...(appDomain && sslEmail ? { ssl_email: sslEmail } : {}),
        ...(memoryMb ? { memory_mb: parseInt(memoryMb) } : {}),
        ...(cpuPercent ? { cpu_percent: parseInt(cpuPercent) } : {}),
      });
      setSelected(null);
      setMessage({
        text: `${selected.name} deployed successfully${appDomain ? ` → ${appDomain}` : ""}${sslEmail ? " (with SSL)" : ""}`,
        type: "success",
      });
      loadApps();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Deploy failed",
        type: "error",
      });
    } finally {
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

  const handleUpdate = async (containerId: string) => {
    setActionLoading(`${containerId}-update`);
    setMessage({ text: "", type: "" });
    try {
      await api.post(`/apps/${containerId}/update`);
      setMessage({ text: "App updated to latest image", type: "success" });
      loadApps();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Update failed", type: "error" });
    } finally {
      setActionLoading(null);
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
    setComposeDeploying(true);
    setComposeError("");
    try {
      const result = await api.post<ComposeDeployResult>("/apps/compose/deploy", { yaml: composeYaml });
      const failed = result.services.filter(s => s.status === "failed");
      if (failed.length > 0) {
        setComposeError(`${failed.length} service(s) failed: ${failed.map(s => `${s.name}: ${s.error}`).join(", ")}`);
      } else {
        setShowCompose(false);
        setComposeYaml("");
        setComposeParsed(null);
        setMessage({
          text: `Compose import: ${result.services.length} service(s) deployed`,
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

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Docker Apps</h1>
          <p className="text-sm text-dark-200 font-mono mt-1">
            One-click deploy popular applications
          </p>
        </div>
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

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-rust-50 text-rust-400 border-rust-200"
              : "bg-red-500/10 text-red-400 border-red-500/20"
          }`}
        >
          {message.text}
        </div>
      )}

      {/* Deployed Apps */}
      {apps.length > 0 && (
        <div className="mb-8">
          <h2 className="text-sm font-medium text-dark-200 uppercase font-mono tracking-widest mb-3">
            Running Apps
          </h2>
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
                {apps.map((app) => (
                  <tr key={app.container_id} className="hover:bg-dark-700/30 transition-colors">
                    <td className="px-5 py-4 text-sm text-dark-50 font-medium font-mono">{app.name}</td>
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
                          app.health === "unhealthy" ? "bg-red-500/15 text-red-400" :
                          "bg-warn-500/15 text-warn-400"
                        }`}>
                          <span className={`w-1.5 h-1.5 rounded-full ${
                            app.health === "healthy" ? "bg-rust-400" :
                            app.health === "unhealthy" ? "bg-red-400" :
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
                            className="px-2 py-1 bg-rust-50 text-rust-400 rounded text-xs font-medium hover:bg-rust-100 disabled:opacity-50"
                          >
                            {actionLoading === `${app.container_id}-start` ? "..." : "Start"}
                          </button>
                        )}
                        <button
                          onClick={() => handleAction(app.container_id, "restart")}
                          disabled={!!actionLoading}
                          className="px-2 py-1 bg-blue-50 text-blue-400 rounded text-xs font-medium hover:bg-blue-100 disabled:opacity-50"
                        >
                          {actionLoading === `${app.container_id}-restart` ? "..." : "Restart"}
                        </button>
                        <button
                          onClick={() => handleLogs(app.container_id)}
                          className={`px-2 py-1 rounded text-xs font-medium ${
                            logsTarget === app.container_id
                              ? "bg-slate-200 text-slate-700"
                              : "bg-slate-50 text-slate-600 hover:bg-slate-100"
                          }`}
                        >
                          Logs
                        </button>
                        <button
                          onClick={() => handleEnv(app.container_id)}
                          className="px-2 py-1 rounded text-xs font-medium bg-purple-50 text-purple-600 hover:bg-purple-100"
                        >
                          Env
                        </button>
                        <button
                          onClick={() => handleUpdate(app.container_id)}
                          disabled={!!actionLoading}
                          className="px-2 py-1 bg-cyan-50 text-cyan-600 rounded text-xs font-medium hover:bg-cyan-100 disabled:opacity-50"
                        >
                          {actionLoading === `${app.container_id}-update` ? "Updating..." : "Update"}
                        </button>
                        {deleteTarget === app.container_id ? (
                          <>
                            <button onClick={() => handleRemove(app.container_id)} className="px-2 py-1 bg-red-600 text-white rounded text-xs">Confirm</button>
                            <button onClick={() => setDeleteTarget(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs">Cancel</button>
                          </>
                        ) : (
                          <button onClick={() => setDeleteTarget(app.container_id)} className="px-2 py-1 bg-red-500/10 text-red-400 rounded text-xs font-medium hover:bg-red-100">Remove</button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* Logs modal */}
          {logsTarget && (() => {
            const filtered = logSearch
              ? logLines.filter(l => l.toLowerCase().includes(logSearch.toLowerCase()))
              : logLines;
            return (
              <div
                className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4"
                role="dialog"
                aria-labelledby="logs-dialog-title"
                onKeyDown={(e) => { if (e.key === "Escape") closeLogs(); }}
              >
                <div className="bg-dark-800 rounded-lg shadow-xl w-full max-w-4xl max-h-[85vh] flex flex-col overflow-hidden border border-dark-500">
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
                        logAutoScroll ? "bg-blue-500/15 text-blue-400" : "bg-dark-700 text-dark-200 hover:bg-dark-600"
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
                  <div className="flex-1 overflow-y-auto p-4 bg-[#0d1117] font-mono text-xs min-h-0">
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
                          ? "text-red-400"
                          : ll.includes("warn")
                          ? "text-warn-400"
                          : ll.includes("info")
                          ? "text-blue-300"
                          : "text-gray-300";
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
              className="bg-dark-800 rounded-lg border border-dark-500 p-5 text-left hover:border-indigo-300 hover:shadow-sm transition-all group"
            >
              <div className="flex items-center gap-3 mb-3">
                <div className={`w-10 h-10 rounded-lg flex items-center justify-center ${categoryColors[tmpl.category] || "bg-dark-700"}`}>
                  {appIcons[tmpl.id] || <span className="text-sm font-bold text-dark-200">{tmpl.name[0]}</span>}
                </div>
                <div>
                  <h3 className="text-sm font-semibold text-dark-50 group-hover:text-indigo-600">
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
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 p-4"
          role="dialog"
          aria-labelledby="deploy-dialog-title"
          onKeyDown={(e) => { if (e.key === "Escape") setSelected(null); }}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-[480px] max-h-[80vh] overflow-y-auto">
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

              {selected.env_vars.length > 0 && (
                <div>
                  <p className="text-sm font-medium text-dark-100 mb-2">Environment Variables</p>
                  <div className="space-y-2">
                    {selected.env_vars.map((v) => (
                      <div key={v.name}>
                        <label className="block text-xs text-dark-200 mb-0.5">
                          {v.label} {v.required && <span className="text-red-500">*</span>}
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
                className="px-4 py-2 text-sm text-dark-200 hover:text-gray-800"
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
          className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4"
          role="dialog"
          aria-labelledby="env-dialog-title"
          onKeyDown={(e) => { if (e.key === "Escape") setEnvTarget(null); }}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-lg max-h-[80vh] overflow-y-auto border border-dark-500">
            <div className="flex items-center justify-between mb-4">
              <h3 id="env-dialog-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                Environment Variables
              </h3>
              <button onClick={() => setEnvTarget(null)} className="text-dark-300 hover:text-dark-50">
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
              </button>
            </div>
            <p className="text-xs text-dark-300 mb-3">
              {apps.find(a => a.container_id === envTarget)?.name}
            </p>
            {envLoading ? (
              <div className="flex items-center justify-center py-8 text-dark-300">
                <svg className="w-5 h-5 animate-spin mr-2" fill="none" viewBox="0 0 24 24"><circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" /><path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" /></svg>
                Loading...
              </div>
            ) : envVars.length === 0 ? (
              <div className="text-center py-8 text-dark-300 text-sm">No environment variables set</div>
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
            {(() => {
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
            <div className="mt-4 pt-3 border-t border-dark-600">
              <p className="text-[10px] text-dark-300">
                Environment variables are set at deploy time. To change them, redeploy the app with updated values.
              </p>
            </div>
          </div>
        </div>
      )}

      {/* Compose Import Dialog */}
      {showCompose && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 p-4"
          role="dialog"
          aria-labelledby="compose-dialog-title"
          onKeyDown={(e) => { if (e.key === "Escape") { setShowCompose(false); setComposeParsed(null); setComposeError(""); }}}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-2xl max-h-[85vh] overflow-y-auto">
            <h3 id="compose-dialog-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-1">
              Import Docker Compose
            </h3>
            <p className="text-sm text-dark-200 mb-4">
              Paste your docker-compose.yml to parse and deploy services. Only services with an <code className="text-xs bg-dark-700 px-1 rounded">image:</code> field are supported (no build context).
            </p>

            {composeError && (
              <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-red-500/10 text-red-400 border-red-500/20">
                {composeError}
              </div>
            )}

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
                  className="px-4 py-2 text-sm text-dark-200"
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
                              <span key={i} className="px-1.5 py-0.5 bg-blue-50 text-blue-600 rounded text-[10px] font-mono">
                                {p.host}:{p.container}/{p.protocol}
                              </span>
                            ))}
                          </div>
                        )}
                        {Object.keys(svc.environment).length > 0 && (
                          <div className="mt-1.5 flex gap-2 flex-wrap">
                            {Object.entries(svc.environment).slice(0, 5).map(([k, v]) => (
                              <span key={k} className="px-1.5 py-0.5 bg-purple-50 text-purple-600 rounded text-[10px] font-mono">
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
          </div>
        </div>
      )}
    </div>
  );
}
