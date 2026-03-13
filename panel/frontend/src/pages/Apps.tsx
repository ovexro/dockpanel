import { useState, useEffect, type ReactNode } from "react";
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
  Database: "bg-emerald-500/15",
  Monitoring: "bg-purple-500/15",
  DevOps: "bg-amber-500/15",
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
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#6644ff" d="M64 8L8 64l56 56 56-56L64 8zm0 20l36 36-36 36-36-36 36-36zm0 16L44 64l20 20 20-20-20-20z"/></svg>
  ),
  // ─── Databases ───────────────────────────────────────────────
  postgres: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#336791" d="M93.8 18.3C87.5 12 78.6 8 64 8S40.5 12 34.2 18.3C28 24.5 24 33.5 24 44c0 14 5 24 12 31v29c0 10 6 16 16 16h24c10 0 16-6 16-16V75c7-7 12-17 12-31 0-10.5-4-19.5-10.2-25.7zM52 108V80h24v28H52zM80 68c-4.4 4.4-8 6-8 6V60c6-2 10-8 10-14 0-6.6-5.4-12-12-12h-12c-6.6 0-12 5.4-12 12 0 6 4 12 10 14v14s-3.6-1.6-8-6C42 62 40 54 40 44c0-8 3-15 7.5-19.5C52 20 57.5 18 64 18s12 2 16.5 6.5C85 29 88 36 88 44c0 10-2 18-8 24z"/><circle fill="#336791" cx="54" cy="44" r="4"/><circle fill="#336791" cx="74" cy="44" r="4"/></svg>
  ),
  mysql: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#4479a1" d="M64 16c-26.5 0-48 9-48 20v56c0 11 21.5 20 48 20s48-9 48-20V36c0-11-21.5-20-48-20zm0 8c22 0 40 7 40 12s-18 12-40 12S24 41 24 36s18-12 40-12zM24 52c8 6 22 10 40 10s32-4 40-10v16c0 5-18 12-40 12S24 73 24 68V52zm0 28c8 6 22 10 40 10s32-4 40-10v12c0 5-18 12-40 12S24 97 24 92V80z"/></svg>
  ),
  mariadb: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#c0765a" d="M64 16c-26.5 0-48 9-48 20v56c0 11 21.5 20 48 20s48-9 48-20V36c0-11-21.5-20-48-20zm0 8c22 0 40 7 40 12s-18 12-40 12S24 41 24 36s18-12 40-12zM24 52c8 6 22 10 40 10s32-4 40-10v16c0 5-18 12-40 12S24 73 24 68V52zm0 28c8 6 22 10 40 10s32-4 40-10v12c0 5-18 12-40 12S24 97 24 92V80z"/><circle fill="#fff" cx="64" cy="36" r="6" opacity=".3"/></svg>
  ),
  mongo: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#47a248" d="M68.6 5.8c-1.6-2.4-3.2-4-3.8-5.8-.8 1.4-2 3-3.6 5.2C48.4 22.8 28 45.6 28 74c0 18 12.4 34 32.4 39.4l.6.2V88c-8-2-14-9-14-17.4 0-8.4 5.2-15.2 12.4-17.8V6h1.2v46.8c7.2 2.6 12.4 9.4 12.4 17.8 0 8.4-6 15.4-14 17.4v25.6l.6-.2C79.6 108 100 92 100 74 100 44.4 78.4 21.4 68.6 5.8z"/></svg>
  ),
  redis: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#d82c20" d="M121.8 69.8c-3.8 2-23.4 10-27.6 12.2-4.2 2.2-6.5 2.1-9.8.4-3.3-1.7-24.8-10.3-28.6-12-3.8-1.8-3.7-3.1 0-4.7 3.8-1.7 25.2-10.4 28.5-11.8 3.3-1.5 5.6-1.5 9.5.3 3.9 1.8 24 9.8 28 11.6 4.2 1.8 3.8 2.1 0 4zM121.8 55.6c-3.8 2-23.4 10-27.6 12.2-4.2 2.2-6.5 2.1-9.8.4-3.3-1.7-24.8-10.3-28.6-12-3.8-1.8-3.7-3.1 0-4.7 3.8-1.7 25.2-10.4 28.5-11.8 3.3-1.5 5.6-1.5 9.5.3 3.9 1.8 24 9.8 28 11.6 4.2 1.8 3.8 2.1 0 4z"/><path fill="#a41209" d="M121.8 83.6c-3.8 2-23.4 10-27.6 12.2-4.2 2.2-6.5 2.1-9.8.4-3.3-1.7-24.8-10.3-28.6-12-3.8-1.8-3.7-3.1 0-4.7 3.8-1.7 25.2-10.4 28.5-11.8 3.3-1.5 5.6-1.5 9.5.3 3.9 1.8 24 9.8 28 11.6 4.2 1.8 3.8 2.1 0 4z"/><path fill="#d82c20" d="M68.2 32.5l24.2 4.8-8.2 3.8-24-5z"/><ellipse fill="#fff" cx="91.6" cy="41.6" rx="8" ry="3.2"/><path fill="#7a0c00" d="M63.2 27.6l8.8 17.8 19-8.4-8.8-17.8z"/><path fill="#ad1c12" d="M63.2 27.6l8.8 17.8 8.4-3.8-8.8-14z"/></svg>
  ),
  // ─── Monitoring ──────────────────────────────────────────────
  grafana: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#f46800" d="M105 56c-1-4-3-7-6-10 0-4-2-8-5-11s-6-6-10-7c-2-4-5-7-9-9s-8-3-13-3-9 1-13 3-7 5-9 9c-4 1-7 4-10 7s-5 7-5 11c-3 3-5 6-6 10s-1 8 0 12c-2 4-2 8-1 12s3 8 6 10c1 4 3 8 6 11s7 5 11 6c2 5 5 8 9 10s9 3 13 3 9-1 13-3 7-5 9-10c4-1 8-3 11-6s5-7 6-11c3-2 5-6 6-10s1-8-1-12c1-4 2-8 0-12zM64 100c-20 0-36-16-36-36S44 28 64 28s36 16 36 36-16 36-36 36z"/><path fill="#f46800" d="M78 52H66v24h4V60h8v16h4V56c0-2.2-1.8-4-4-4zM54 60c2.2 0 4-1.8 4-4s-1.8-4-4-4-4 1.8-4 4 1.8 4 4 4zm-2 4v12h4V64h-4z"/></svg>
  ),
  prometheus: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#e6522c" d="M64 4C30.9 4 4 30.9 4 64s26.9 60 60 60 60-26.9 60-60S97.1 4 64 4zm0 108.6c-7.8 0-14.2-4.8-14.2-10.8h28.4c0 6-6.4 10.8-14.2 10.8zm23.4-15.4H40.6v-8.6h46.8v8.6zm-.2-13H40.8c-.4-.4-.8-1-1.2-1.4-5.8-7.2-7.2-10.8-8.8-14.2-.2 0 6.2 2.8 10.6 4.6 0 0 2.2 1 5.4 2.2-3.6-4.2-5.8-9.8-5.8-11.4 0 0 4.4 3.6 10.2 5.8-2.6-4.2-4.6-11.2-3.6-15.2 3.2 4.8 10 9.2 12.2 10.2-2.4-5.4-2-13.8 1.2-18.6 1.8 5.4 7.6 11.6 11.4 13.6-1.2-3-2-9-1-12.8 2.4 4 7.2 9 8 15.2 2.2-1.8 4-4.2 5.6-6.8l1.6 8c.4 2.4.8 4.2.2 7.4-.2 1.4-.6 2.8-1.2 4.2-1 2.4-2.6 4.4-5.2 7.6-.4.4-.6.8-1 1.2z"/></svg>
  ),
  "uptime-kuma": (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="20" fill="#5cdd8b" width="128" height="128"/><path fill="#fff" d="M64 28c-16 0-28 10-28 28 0 12 8 22 16 30l12 12 12-12c8-8 16-18 16-30 0-18-12-28-28-28zm0 12c8.8 0 16 7.2 16 16s-7.2 16-16 16-16-7.2-16-16 7.2-16 16-16z"/><circle fill="#5cdd8b" cx="64" cy="56" r="8"/></svg>
  ),
  loki: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#f46800" width="128" height="128"/><path fill="#fff" d="M24 40h80v8H24zm0 20h60v8H24zm0 20h72v8H24z" opacity=".9"/><circle fill="#fff" cx="100" cy="64" r="8"/></svg>
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
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#10b981" width="128" height="128"/><path fill="#fff" d="M28 40c0-4.4 3.6-8 8-8h56c4.4 0 8 3.6 8 8v48c0 4.4-3.6 8-8 8H36c-4.4 0-8-3.6-8-8V40z" opacity=".2"/><path fill="#fff" d="M40 44h48v8H40zm0 16h48v8H40zm0 16h32v8H40z"/><circle fill="#fff" cx="88" cy="80" r="8" opacity=".5"/></svg>
  ),
  portainer: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#13bef9" d="M64 4L4 34v60l60 30 60-30V34L64 4z"/><path fill="#fff" d="M64 20L16 44v40l48 24 48-24V44L64 20zm0 12l36 18-36 18-36-18 36-18z" opacity=".9"/><path fill="#fff" d="M28 52l36 18v32L28 84V52z" opacity=".6"/><path fill="#fff" d="M100 52L64 70v32l36-18V52z" opacity=".3"/></svg>
  ),
  pgadmin: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#336791" width="128" height="128"/><path fill="#fff" d="M64 20c-18 0-32 12-32 28 0 10 6 18 14 24v20c0 6 4 10 10 10h16c6 0 10-4 10-10V72c8-6 14-14 14-24 0-16-14-28-32-28zm-10 28a6 6 0 110-12 6 6 0 010 12zm20 0a6 6 0 110-12 6 6 0 010 12zM52 84V74h24v10H52z" opacity=".9"/></svg>
  ),
  minio: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#c72c48" width="128" height="128"/><path fill="#fff" d="M24 44c8-12 20-20 40-20s32 8 40 20c-2 4-6 8-12 12-6-8-16-12-28-12s-22 4-28 12c-6-4-10-8-12-12z" opacity=".9"/><path fill="#fff" d="M24 68c8-12 20-20 40-20s32 8 40 20c-2 4-6 8-12 12-6-8-16-12-28-12s-22 4-28 12c-6-4-10-8-12-12z" opacity=".6"/><path fill="#fff" d="M24 92c8-12 20-20 40-20s32 8 40 20c-2 4-6 8-12 12-6-8-16-12-28-12s-22 4-28 12c-6-4-10-8-12-12z" opacity=".3"/></svg>
  ),
  meilisearch: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#ff5caa" d="M20 100l24-72h20l-24 72H20zm22 0l24-72h20l-24 72H42zm22 0l24-72h20l-24 72H64z" opacity=".9"/></svg>
  ),
  nocodb: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#7f66ff" width="128" height="128"/><rect fill="#fff" x="20" y="28" width="88" height="72" rx="8" opacity=".2"/><path fill="#fff" d="M28 36h72v12H28zm0 20h72v12H28zm0 20h72v12H28z" opacity=".8"/><path fill="#fff" d="M60 36h4v56h-4z" opacity=".4"/></svg>
  ),
  searxng: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#3050ff" width="128" height="128"/><circle fill="none" stroke="#fff" strokeWidth="10" cx="54" cy="54" r="22"/><path fill="#fff" d="M70 74l24 24c2.8 2.8 2.8 7.2 0 10s-7.2 2.8-10 0L60 84l10-10z"/><circle fill="#fff" cx="54" cy="54" r="8" opacity=".3"/></svg>
  ),
  vaultwarden: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#175ddc" d="M64 4L12 28v36c0 28 22 52 52 60 30-8 52-32 52-60V28L64 4z"/><path fill="#fff" d="M64 20L24 38v26c0 22 16 42 40 48 24-6 40-26 40-48V38L64 20z" opacity=".15"/><rect fill="#fff" x="48" y="48" width="32" height="36" rx="4"/><path fill="#fff" d="M56 48V38c0-4.4 3.6-8 8-8s8 3.6 8 8v10h-4V38c0-2.2-1.8-4-4-4s-4 1.8-4 4v10h-4z"/><circle fill="#175ddc" cx="64" cy="64" r="4"/><rect fill="#175ddc" x="62" y="64" width="4" height="10" rx="2"/></svg>
  ),
  // ─── Storage ─────────────────────────────────────────────────
  nextcloud: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#0082c9" d="M64 28c-14 0-25 9-29 22-6-4-14-6-21-2-10 5-14 18-8 28s18 14 28 8c4 13 15 22 30 22s26-9 30-22c10 6 22 2 28-8s2-23-8-28c-4-13-16-22-30-22h-20zm0 14c12 0 22 10 22 22s-10 22-22 22-22-10-22-22 10-22 22-22z"/></svg>
  ),
  // ─── Development ─────────────────────────────────────────────
  gitea: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#609926" width="128" height="128"/><path fill="#fff" d="M64 24c-22 0-40 18-40 40s18 40 40 40 40-18 40-40-18-40-40-40zm0 12c15.5 0 28 12.5 28 28S79.5 92 64 92 36 79.5 36 64s12.5-28 28-28z"/><circle fill="#fff" cx="52" cy="56" r="5"/><circle fill="#fff" cx="76" cy="56" r="5"/><path fill="none" stroke="#fff" strokeWidth="3" strokeLinecap="round" d="M52 72s4 8 12 8 12-8 12-8"/></svg>
  ),
  "code-server": (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#007acc" width="128" height="128"/><path fill="#fff" d="M48 32L16 64l32 32 8-8-24-24 24-24-8-8zm32 0l-8 8 24 24-24 24 8 8 32-32-32-32z" opacity=".9"/><rect fill="#fff" x="56" y="28" width="8" height="72" rx="4" transform="rotate(15 64 64)" opacity=".6"/></svg>
  ),
  drone: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#212121" width="128" height="128"/><circle fill="none" stroke="#fff" strokeWidth="8" cx="64" cy="64" r="28"/><path fill="#fff" d="M64 36a28 28 0 010 56V36z" opacity=".4"/><circle fill="#fff" cx="64" cy="64" r="8"/></svg>
  ),
  registry: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#2496ed" width="128" height="128"/><path fill="#fff" d="M24 68h16v16H24zm20-20h16v36H44zm20-16h16v52H64zm20 8h16v44H84z" opacity=".9"/><ellipse fill="#fff" cx="64" cy="96" rx="36" ry="8" opacity=".3"/></svg>
  ),
  mailhog: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#f59e0b" width="128" height="128"/><rect fill="#fff" x="20" y="36" width="88" height="56" rx="8" opacity=".2"/><path fill="#fff" d="M24 40l40 28 40-28v8L68 76c-2 1.5-6 1.5-8 0L24 48v-8z" opacity=".9"/><path fill="none" stroke="#fff" strokeWidth="4" opacity=".6" d="M24 40l40 28 40-28"/></svg>
  ),
  // ─── Automation ──────────────────────────────────────────────
  n8n: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><rect rx="16" fill="#ea4b71" width="128" height="128"/><circle fill="#fff" cx="32" cy="64" r="12"/><circle fill="#fff" cx="96" cy="40" r="12"/><circle fill="#fff" cx="96" cy="88" r="12"/><path fill="none" stroke="#fff" strokeWidth="6" strokeLinecap="round" opacity=".7" d="M44 64h28M72 64l12-18M72 64l12 18"/></svg>
  ),
  // ─── Media ───────────────────────────────────────────────────
  jellyfin: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><path fill="#00a4dc" d="M64 8C33 8 8 33 8 64s25 56 56 56 56-25 56-56S95 8 64 8z"/><path fill="#fff" d="M52 40v48l40-24-40-24z" opacity=".9"/></svg>
  ),
  // ─── Networking ──────────────────────────────────────────────
  pihole: (
    <svg className="w-6 h-6" viewBox="0 0 128 128"><circle fill="#96060c" cx="64" cy="64" r="60"/><circle fill="#f44" cx="64" cy="64" r="48" opacity=".3"/><path fill="#fff" d="M64 16C38.5 16 18 36.5 18 62c0 4 .5 8 1.5 12h89c1-4 1.5-8 1.5-12 0-25.5-20.5-46-46-46z" opacity=".15"/><rect fill="#fff" x="56" y="28" width="16" height="72" rx="8"/><rect fill="#fff" x="28" y="56" width="72" height="16" rx="8"/></svg>
  ),
};

const statusColors: Record<string, string> = {
  running: "bg-emerald-500/15 text-emerald-400",
  exited: "bg-red-500/15 text-red-400",
  created: "bg-amber-500/15 text-amber-400",
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

  const handleLogs = async (containerId: string) => {
    if (logsTarget === containerId) {
      setLogsTarget(null);
      setLogLines([]);
      return;
    }
    setLogsTarget(containerId);
    setLogLines([]);
    try {
      const data = await api.get<{ logs: string }>(`/apps/${containerId}/logs`);
      setLogLines((data.logs || "").split("\n").filter(Boolean));
    } catch (e) {
      setLogLines([`Error: ${e instanceof Error ? e.message : "Failed to fetch logs"}`]);
    }
  };

  const handleRemove = async (containerId: string) => {
    try {
      await api.delete(`/apps/${containerId}`);
      setDeleteTarget(null);
      if (logsTarget === containerId) {
        setLogsTarget(null);
        setLogLines([]);
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
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-bold text-dark-50">Docker Apps</h1>
          <p className="text-sm text-dark-200 mt-1">
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
              ? "bg-emerald-50 text-emerald-400 border-emerald-200"
              : "bg-red-500/10 text-red-400 border-red-500/20"
          }`}
        >
          {message.text}
        </div>
      )}

      {/* Deployed Apps */}
      {apps.length > 0 && (
        <div className="mb-8">
          <h2 className="text-sm font-medium text-dark-200 uppercase mb-3">
            Running Apps
          </h2>
          <div className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden">
            <table className="w-full">
              <thead>
                <tr className="bg-dark-900 border-b border-dark-500">
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3">App</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 hidden sm:table-cell">Template</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 hidden md:table-cell">Domain</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 hidden sm:table-cell w-20">Port</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 hidden lg:table-cell w-24">Health</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 w-24">Status</th>
                  <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase px-5 py-3">Actions</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-600">
                {apps.map((app) => (
                  <tr key={app.container_id} className="hover:bg-dark-800">
                    <td className="px-5 py-4 text-sm text-dark-50 font-medium">{app.name}</td>
                    <td className="px-5 py-4 text-sm text-dark-200 hidden sm:table-cell">{app.template}</td>
                    <td className="px-5 py-4 text-sm hidden md:table-cell">
                      {app.domain ? (
                        <a href={`https://${app.domain}`} target="_blank" rel="noopener noreferrer" className="text-rust-400 hover:underline">{app.domain}</a>
                      ) : (
                        <span className="text-dark-300">{"\u2014"}</span>
                      )}
                    </td>
                    <td className="px-5 py-4 text-sm text-dark-200 font-mono hidden sm:table-cell">{app.port || "\u2014"}</td>
                    <td className="px-5 py-4 hidden lg:table-cell">
                      {app.health ? (
                        <span className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium ${
                          app.health === "healthy" ? "bg-emerald-500/15 text-emerald-400" :
                          app.health === "unhealthy" ? "bg-red-500/15 text-red-400" :
                          "bg-amber-500/15 text-amber-400"
                        }`}>
                          <span className={`w-1.5 h-1.5 rounded-full ${
                            app.health === "healthy" ? "bg-emerald-400" :
                            app.health === "unhealthy" ? "bg-red-400" :
                            "bg-amber-400"
                          }`} />
                          {app.health}
                        </span>
                      ) : (
                        <span className="text-dark-400 text-xs">{"\u2014"}</span>
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
                            className="px-2 py-1 bg-amber-50 text-amber-400 rounded text-xs font-medium hover:bg-amber-100 disabled:opacity-50"
                          >
                            {actionLoading === `${app.container_id}-stop` ? "..." : "Stop"}
                          </button>
                        ) : (
                          <button
                            onClick={() => handleAction(app.container_id, "start")}
                            disabled={actionLoading === `${app.container_id}-start`}
                            className="px-2 py-1 bg-emerald-50 text-emerald-400 rounded text-xs font-medium hover:bg-emerald-100 disabled:opacity-50"
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

          {/* Logs panel */}
          {logsTarget && (
            <div className="mt-3 bg-[#1e1e2e] rounded-xl border border-gray-700 overflow-hidden">
              <div className="flex items-center justify-between px-4 py-2 border-b border-gray-700">
                <span className="text-xs text-dark-300 font-mono">
                  Logs: {apps.find(a => a.container_id === logsTarget)?.name}
                </span>
                <button
                  onClick={() => { setLogsTarget(null); setLogLines([]); }}
                  className="text-dark-200 hover:text-gray-300 text-xs"
                >
                  Close
                </button>
              </div>
              <div className="p-4 max-h-64 overflow-y-auto font-mono text-xs">
                {logLines.length === 0 ? (
                  <div className="text-dark-200">Loading...</div>
                ) : (
                  logLines.map((line, i) => (
                    <div key={i} className="py-0.5 text-gray-300 whitespace-pre-wrap break-all">
                      {line}
                    </div>
                  ))
                )}
              </div>
            </div>
          )}
        </div>
      )}

      {/* Template Gallery */}
      <h2 className="text-sm font-medium text-dark-200 uppercase mb-3">
        App Templates
      </h2>
      {loading ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
          {[...Array(8)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-xl border border-dark-500 p-5 animate-pulse">
              <div className="h-5 bg-dark-600 rounded w-24 mb-2" />
              <div className="h-4 bg-dark-600 rounded w-full" />
            </div>
          ))}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
          {templates.map((tmpl) => (
            <button
              key={tmpl.id}
              onClick={() => openDeploy(tmpl)}
              className="bg-dark-800 rounded-xl border border-dark-500 p-5 text-left hover:border-indigo-300 hover:shadow-sm transition-all group"
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
          <div className="bg-dark-800 rounded-xl shadow-xl p-6 w-full max-w-[480px] max-h-[80vh] overflow-y-auto">
            <h3 id="deploy-dialog-title" className="text-lg font-semibold text-dark-50 mb-1">
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
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Host Port</label>
                <input
                  type="number"
                  value={appPort}
                  onChange={(e) => setAppPort(Number(e.target.value))}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
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
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
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
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
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
                      className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
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
                      className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
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
                          className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
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

      {/* Compose Import Dialog */}
      {showCompose && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 p-4"
          role="dialog"
          aria-labelledby="compose-dialog-title"
          onKeyDown={(e) => { if (e.key === "Escape") { setShowCompose(false); setComposeParsed(null); setComposeError(""); }}}
        >
          <div className="bg-dark-800 rounded-xl shadow-xl p-6 w-full max-w-2xl max-h-[85vh] overflow-y-auto">
            <h3 id="compose-dialog-title" className="text-lg font-semibold text-dark-50 mb-1">
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
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
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
                              <span key={i} className="px-1.5 py-0.5 bg-amber-50 text-amber-600 rounded text-[10px] font-mono">
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
                    className="px-4 py-2 bg-emerald-600 text-white rounded-lg text-sm font-medium hover:bg-emerald-700 disabled:opacity-50 flex items-center gap-2"
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
