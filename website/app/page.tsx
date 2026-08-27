import Image from "next/image";
import { Alert, Link, buttonVariants, cn } from "@heroui/react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ComputerTerminal01Icon,
  DatabaseIcon,
  Download04Icon,
  FileExportIcon,
  GithubIcon,
  GridTableIcon,
  Key01Icon,
  PaintBrush01Icon,
  SecurityCheckIcon,
  SecurityLockIcon,
  Tick02Icon,
} from "@hugeicons/core-free-icons";

const sourceUrl = "https://github.com/HakimIno/plusplus";

function ButtonLink({
  href,
  children,
  className,
  size = "md",
  variant = "primary",
  target,
  rel,
}: {
  href: string;
  children: React.ReactNode;
  className?: string;
  size?: "sm" | "md" | "lg";
  variant?: "primary" | "secondary" | "tertiary" | "outline" | "ghost" | "danger";
  target?: string;
  rel?: string;
}) {
  return (
    <a
      href={href}
      target={target}
      rel={rel}
      className={cn(buttonVariants({ size, variant }), className)}
    >
      {children}
    </a>
  );
}

const databases = [
  {
    name: "PostgreSQL",
    detail: "Native wire protocol with schema browsing, staged edits, and production safeguards across every supported workflow.",
    icon: "/databases/postgresql.svg",
  },
  {
    name: "MySQL / MariaDB",
    detail: "One connection flow for MySQL-compatible servers and MariaDB clusters, with the same editor and result grid everywhere.",
    icon: "/databases/mysql.svg",
  },
  {
    name: "SQL Server",
    detail: "TDS protocol support for Microsoft SQL Server instances, including read-only enforcement where the session allows it.",
    icon: "/databases/microsoftsqlserver.svg",
  },
  {
    name: "SQLite",
    detail: "Open a local file and work offline with the full schema browser, SQL editor, and export tools built into plusplus.",
    icon: "/databases/sqlite.svg",
  },
  {
    name: "Cassandra",
    detail: "CQL native protocol for Cassandra clusters, with schema introspection and query tooling tuned for wide-column workloads.",
    icon: "/databases/cassandra.svg",
  },
  {
    name: "ScyllaDB",
    detail: "CQL-compatible cluster support for ScyllaDB deployments, sharing the same connection and query experience as Cassandra.",
    icon: "/databases/scylladb.svg",
  },
];

function MacOSPlatformIcon({ className }: { className?: string }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" className={className} aria-hidden="true">
      <path d="M0 0h256v256H0z" fill="none" />
      <g fill="none">
        <g clipPath="url(#macos-platform-clip)">
          <path fill="#f4f2ed" d="M196 0H60C26.863 0 0 26.863 0 60v136c0 33.137 26.863 60 60 60h136c33.137 0 60-26.863 60-60V60c0-33.137-26.863-60-60-60" />
          <path fill="#00a0e2" fillRule="evenodd" d="M191.072 195.009c-3.27 5.387-6.54 9.426-10.484 13.176c-4.424 3.944-5.674 7.406-15.87 8.754c-7.213 1.347-13.465-1.348-15.87-2.405c-7.214-3.367-10.966-4.424-15.293-4.424c-4.232 0-7.791 1.058-14.909 4.328c-2.212 1.153-8.175 3.751-15.485 2.405c-7.502-1.347-11.35-4.232-14.139-6.637c-5.771-5.097-10.1-9.907-14.043-15.485z" clipRule="evenodd" />
          <path fill="#34be2d" fillRule="evenodd" d="M58.726 105.27c3.366-7.598 7.79-12.696 12.215-16.255c11.253-9.233 29.624-9.81 38.088-7.598c6.926 1.731 11.831 5.963 19.622 5.963c8.175 0 12.887-4.136 19.333-5.963c8.464-2.116 26.931-1.443 38.954 7.79c3.559 2.694 6.827 6.349 8.655 8.369c-4.327 3.174-6.925 5.482-9.041 7.694z" clipRule="evenodd" />
          <path fill="#ffb400" fillRule="evenodd" d="M186.553 105.27c-2.02 2.212-3.462 4.329-5.098 7.31c-1.922 3.463-4.232 8.176-4.809 15.293H53.051c.096-1.154.192-2.404.384-3.655c1.155-7.598 2.982-13.85 5.29-18.948z" clipRule="evenodd" />
          <path fill="#ff7a00" fillRule="evenodd" d="M176.646 127.873a74 74 0 0 0 0 6.541c.289 5.29 2.116 11.157 4.521 15.87l-125.712-.289c-1.731-7.598-2.693-15.389-2.404-22.122z" clipRule="evenodd" />
          <path fill="#f41e34" fillRule="evenodd" d="M181.166 150.284a33 33 0 0 0 3.558 5.771c8.272 10.58 11.831 10.58 18.275 13.851c-.479 1.152-.864 2.212-1.346 3.174l-138.888-.289c-2.693-5.867-5.482-14.139-7.31-22.795z" clipRule="evenodd" />
          <path fill="#a2359c" fillRule="evenodd" d="M201.653 173.08c-4.039 9.426-7.31 16.349-10.581 21.929l-116.091-.288c-3.848-5.675-7.31-11.928-11.254-19.719c-.288-.673-.673-1.443-.962-2.211z" clipRule="evenodd" />
          <path fill="#34be2d" fillRule="evenodd" d="M161.352 52.658c-.674 4.81-3.078 10.965-6.925 14.908c-4.138 4.425-10.581 9.234-14.429 11.639c-2.116 1.346-7.599 1.538-12.118 2.02c-.577-4.04-.673-7.503.577-11.254c1.635-4.424 3.753-10.772 7.118-15.197c4.135-5.482 8.848-9.233 11.445-10.58c3.464-1.731 9.235-4.328 14.236-5.194c.193 4.424.867 9.426.096 13.658" clipRule="evenodd" />
        </g>
        <defs>
          <clipPath id="macos-platform-clip">
            <path fill="#fff" d="M0 0h256v256H0z" />
          </clipPath>
        </defs>
      </g>
    </svg>
  );
}

function WindowsPlatformIcon({ className }: { className?: string }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" className={className} aria-hidden="true">
      <path d="M0 0h256v256H0z" fill="none" />
      <g fill="none">
        <g clipPath="url(#windows-platform-clip)">
          <path fill="#f4f2ed" d="M196 0H60C26.863 0 0 26.863 0 60v136c0 33.137 26.863 60 60 60h136c33.137 0 60-26.863 60-60V60c0-33.137-26.863-60-60-60" />
          <path fill="#00adef" d="m40 65.663l70.968-9.665l.032 68.455l-70.934.404zm70.935 66.677l.055 68.515l-70.934-9.753l-.004-59.221zm8.602-77.607L213.636 41v82.582l-94.099.748zm94.121 78.251l-.022 82.211l-94.099-13.281l-.131-69.083z" />
        </g>
        <defs>
          <clipPath id="windows-platform-clip">
            <path fill="#fff" d="M0 0h256v256H0z" />
          </clipPath>
        </defs>
      </g>
    </svg>
  );
}

function LinuxPlatformIcon({ className }: { className?: string }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" className={className} aria-hidden="true">
      <path d="M0 0h256v256H0z" fill="none" />
      <g fill="none">
        <rect width="256" height="256" fill="#f4f2ed" rx="60" />
        <path fill="#eceff1" d="m85.95 199.926l24.53 13.62h37.096l34.702-26.055l15.556-40.859l-35.899-43.227l-10.171-24.278l-49.66 1.776l.598 13.62l-9.573 17.764l-14.958 29.016l-2.991 24.278z" />
        <path fill="#263238" d="M187.064 114.656c-9.573-13.62-17.351-21.91-21.539-39.082s1.197-12.435-2.393-27.24c-1.795-7.697-4.787-13.027-7.778-17.172c-3.59-4.145-7.778-6.514-10.172-7.106c-5.384-2.96-17.949-7.698-33.505.592c-16.155 8.29-14.36 26.055-11.368 62.177c0 2.368-.599 5.33-1.795 7.698c-2.393 5.33-6.582 10.066-10.171 14.212c-4.189 5.921-8.377 11.843-11.368 18.356c-7.18 13.62-13.762 30.792-11.967 37.306c2.992-.592 40.686 56.255 40.686 57.439c2.393-.592 12.564-.592 21.539-.592c12.565-.592 19.744-1.184 29.916 1.184c0-1.776-.599-3.553-.599-5.329c0-3.553.599-6.514 1.197-10.659c.598-2.961 1.197-5.921 1.795-9.474c-5.983 5.329-16.753 11.251-26.924 13.027c-8.975 1.776-23.933-1.184-31.113-10.067c.599 0 1.795 0 2.394-.592c1.795-.592 3.59-1.184 4.188-2.368c1.795-2.961.598-5.922-.598-7.698c-1.197-1.777-10.172-8.291-14.36-11.843c-4.188-3.553-6.581-5.33-8.975-7.698l-4.786-4.738c-1.197-1.184-1.795-2.368-2.393-2.961c-1.197-2.96-1.795-6.513-1.197-11.25c.598-6.514 2.991-11.844 5.983-17.765c1.197-2.369 4.188-7.106 4.188-7.106s-10.171 24.871-4.786 32.569c0 0 .598-7.698 2.991-15.396c1.795-5.33 4.787-13.028 8.377-17.173s12.564-19.541 13.163-29.016c0-4.145.598-8.29.598-11.25c-2.393-2.37 39.489-8.29 41.882-1.777c.598 2.369 8.975 23.686 13.761 34.937c2.393 5.33 5.385 10.067 7.18 15.988c1.795 6.514 2.991 15.396 2.991 24.279c0 1.776 0 4.737-.598 7.698c1.197 0 24.531-24.871-2.991-45.596c0 0 16.752 7.698 17.351 23.094c.598 12.435-4.787 22.502-5.983 24.278c.598 0 12.564 5.33 13.162 5.33c2.394 0 7.18-1.777 7.18-1.777c.599-1.776 2.393-6.514 2.393-8.29c4.189-13.62-5.983-35.529-15.556-49.149" />
        <path fill="#eceff1" d="M111.078 75.574c4.296 0 7.778-5.303 7.778-11.843c0-6.541-3.482-11.843-7.778-11.843S103.3 57.19 103.3 63.73s3.483 11.843 7.778 11.843m26.924 1.185c5.618 0 10.172-6.098 10.172-13.62S143.62 49.52 138.002 49.52c-5.617 0-10.171 6.098-10.171 13.62s4.554 13.62 10.171 13.62" />
        <path fill="#212121" d="M115.424 64.541c-.497-3.893-2.761-6.817-5.056-6.53s-3.752 3.676-3.254 7.57c.497 3.893 2.76 6.817 5.055 6.53c2.295-.288 3.752-3.677 3.255-7.57m21.981 8.664c3.304 0 5.983-3.446 5.983-7.698c0-4.251-2.679-7.698-5.983-7.698c-3.305 0-5.984 3.447-5.984 7.698s2.679 7.698 5.984 7.698" />
        <path fill="#ffc107" d="M216.98 195.781c-2.393-1.184-6.582-2.961-10.172-8.29c-1.794-2.961-1.196-11.251-4.188-14.804c-1.795-2.368-4.188-1.184-4.786-1.184c-5.385 1.184-17.95 9.474-26.326 0c-1.197-1.184-2.992-2.961-5.983-2.961c-2.992 0-4.188 1.184-5.385 3.553s-1.197 4.145-1.197 10.067c0 4.737 0 10.066-.598 14.211c-1.197 10.067-2.991 15.989-2.991 21.91c0 6.514 1.794 10.659 4.188 12.435c1.795 1.777 4.786 2.961 11.368 2.961c6.581 0 10.769-2.368 14.958-6.514c2.991-2.96 5.384-4.145 13.761-10.066c6.581-4.145 16.753-9.475 18.547-11.251c1.197-1.184 2.992-1.777 2.992-5.33c0-2.96-2.393-4.145-4.188-4.737m-120.261 1.777c-5.983-9.475-6.582-11.251-10.77-17.173c-3.59-5.921-11.368-17.172-16.154-17.172c-3.59 0-5.385 1.776-7.778 4.145c-2.394 2.368-4.787 7.698-8.975 10.659c-3.59 2.96-13.761 2.368-16.154 5.921s2.393 8.883 2.393 17.765c0 3.553-2.992 5.921-3.59 8.29c-.598 2.961-1.197 4.737 0 7.106c2.393 3.553 5.385 4.737 25.727 8.882c10.77 2.369 20.941 8.29 27.523 8.883c6.581.592 17.949 0 17.949-15.989c.599-9.474-4.786-11.843-10.171-21.317m11.368-107.18c-3.59-2.369-6.582-4.738-6.582-8.29c0-3.553 2.394-4.738 5.984-7.698c.598-.593 7.179-6.514 13.761-6.514s14.359 4.145 17.351 5.33c5.385 1.183 10.769 2.368 10.171 6.513c-.598 5.921-1.196 7.106-7.18 10.067c-4.188 1.184-11.966 7.698-17.351 7.698c-2.393 0-5.983 0-8.376-.593c-1.795-.592-4.787-3.553-7.778-6.513" />
        <path fill="#634703" d="M106.89 85.64c1.197 1.185 2.992 2.37 4.787 2.961c1.196.592 2.991 1.185 2.991 1.185h5.385c2.992 0 7.18-1.185 11.368-3.553c4.188-1.777 4.787-2.961 7.778-4.145c2.992-1.777 5.983-3.553 4.787-4.145c-1.197-.593-2.394 0-6.582 2.368c-3.59 2.369-6.581 3.553-10.171 5.33c-1.795.592-4.188 1.776-5.983 1.776h-5.385c-1.795 0-2.992-.592-4.787-1.184c-1.196-.593-1.795-1.185-2.393-1.185c-1.196-.592-3.59-2.96-4.786-3.553c0 0-1.197 0-.599.593zm17.95-13.027c.598 1.184 1.795 1.184 2.393 1.776s1.197.593 1.197.593c.598-.593 0-1.777-.599-1.777c0-1.184-2.991-1.184-2.991-.592m-9.573 1.184c0 .593 1.196 1.185 1.196.593c.599-.593 1.197-1.185 1.795-1.185c1.197-.592.598-1.184-1.196-1.184c-1.197.592-1.197 1.184-1.795 1.776" />
        <path fill="#455a64" d="M173.303 178.609v1.776c1.197 2.369 4.188 2.961 6.581 2.961c3.59 0 7.18-2.369 8.975-4.737c0-.592.598-1.185 1.197-1.777c1.196-1.776 1.795-2.96 2.393-3.553c0 0-.598-.592-.598-1.184c-.599-1.184-2.394-2.369-4.787-2.961c-1.795-.592-4.786-1.184-5.983-1.184c-5.385-.592-8.376 1.184-10.171 2.961c0 0 .598 0 .598.592c1.197 1.184 1.795 2.369 1.795 4.145c.598 1.184 0 1.776 0 2.961" />
      </g>
    </svg>
  );
}

const platforms = [
  { id: "macos", name: "macOS", format: "Universal DMG", detail: "Apple Silicon + Intel", icon: MacOSPlatformIcon },
  { id: "windows", name: "Windows", format: "Portable ZIP", detail: "Windows x86_64", icon: WindowsPlatformIcon },
  { id: "linux", name: "Linux", format: "AppImage", detail: "Linux x86_64", icon: LinuxPlatformIcon },
];

const pillars = [
  {
    title: "Safety-first policies",
    text: "Destructive SQL and missing WHERE clauses are flagged before they run. Production connections ask for confirmation.",
    variant: "diamond" as const,
  },
  {
    title: "Local by design",
    text: "Queries, results, history, and credentials stay on your machine. Passwords live in the OS keychain.",
    variant: "circle" as const,
  },
  {
    title: "Native performance",
    text: "A focused Rust desktop app with no Electron, browser runtime, cloud account, or telemetry.",
    variant: "grid" as const,
  },
];

const features = [
  { icon: DatabaseIcon, title: "Schema browser", text: "Tables, columns, keys, indexes, views, routines, and triggers stay within reach." },
  { icon: ComputerTerminal01Icon, title: "SQL editor", text: "One focused editor and the same shortcuts across every connection and dialect." },
  { icon: GridTableIcon, title: "Staged edits", text: "Cell edits, inserts, and deletions stay staged until you save or discard them." },
  { icon: FileExportIcon, title: "Streaming export", text: "Export full tables to CSV or JSON without loading the whole dataset into memory." },
];

const safeguards = [
  { icon: SecurityCheckIcon, title: "Risk checks before execution", text: "Destructive SQL and UPDATE or DELETE without a WHERE clause are flagged before they run." },
  { icon: SecurityLockIcon, title: "Read-only that blocks writes", text: "Read-only mode is enforced in the app and, where supported, in the database session." },
  { icon: Key01Icon, title: "Credentials stay on device", text: "Passwords live in the OS keychain. Query history and optional audit logs remain local." },
];

const themes = [
  { name: "Midnight Conversational", mode: "Dark", base: "#08090D", panel: "#0F1218", surface: "#171B24", code: "#06070A", text: "#EAEFF6", weak: "#687484", accent: "#66D9EF" },
  { name: "Graphite", mode: "Dark", base: "#1E1E1E", panel: "#252525", surface: "#2A2A2A", code: "#1E1E1E", text: "#E8E8E8", weak: "#737373", accent: "#0F7EFF" },
  { name: "Carbon", mode: "Dark", base: "#0A0A0B", panel: "#0E0E10", surface: "#1B1B1E", code: "#000000", text: "#E8E8EA", weak: "#5F5F66", accent: "#6E8EFF" },
  { name: "Daylight", mode: "Light", base: "#F8FAFC", panel: "#EEF2F7", surface: "#E1E6EF", code: "#FFFFFF", text: "#1B2533", weak: "#718096", accent: "#4D6BFE" },
];

type Theme = (typeof themes)[number];

function Brand({ compact = false }: { compact?: boolean }) {
  return (
    <a href="#top" className="inline-flex items-center gap-2.5" aria-label="plusplus home">
      <Image src="/app-icon.png" alt="" width={28} height={28} className="rounded-md" priority />
      <span className={cn("brand-label", compact && "hidden sm:inline")}>plusplus</span>
    </a>
  );
}

function ThemePreview({ theme }: { theme: Theme }) {
  return (
    <article className="group min-w-[250px] flex-1 sm:min-w-[280px]">
      <div className="overflow-hidden rounded-lg border border-[var(--bd-border)] p-1.5 transition-transform duration-300 group-hover:-translate-y-1">
        <div className="overflow-hidden rounded-md" style={{ backgroundColor: theme.base, color: theme.text }}>
          <div className="flex h-8 items-center justify-between px-3" style={{ backgroundColor: theme.panel }}>
            <span className="font-mono text-[8px] opacity-60">schema · query</span>
            <span className="size-2 rounded-full" style={{ backgroundColor: theme.accent }} />
          </div>
          <div className="grid h-[150px] grid-cols-[34%_1fr]">
            <div className="space-y-2 p-3" style={{ backgroundColor: theme.panel }}>
              <span className="block h-4 rounded-md" style={{ backgroundColor: theme.surface }} />
              {[70, 86, 58].map((width) => (
                <span key={width} className="block h-1.5 rounded-full opacity-55" style={{ width: `${width}%`, backgroundColor: theme.weak }} />
              ))}
            </div>
            <div className="p-3" style={{ backgroundColor: theme.code }}>
              <div className="flex gap-1.5">
                <span className="h-1.5 w-10 rounded-full" style={{ backgroundColor: theme.accent }} />
                <span className="h-1.5 w-6 rounded-full opacity-40" style={{ backgroundColor: theme.weak }} />
              </div>
              <div className="mt-5 grid grid-cols-3 gap-2">
                {[0, 1, 2].map((item) => (
                  <span key={item} className="h-10 rounded-md" style={{ backgroundColor: theme.surface }} />
                ))}
              </div>
            </div>
          </div>
        </div>
      </div>
      <div className="mt-3 flex items-center justify-between px-1">
        <p className="text-sm font-medium">{theme.name}</p>
        <span className="rounded-full border border-[var(--bd-border)] px-2 py-0.5 text-xs text-[var(--bd-muted)]">{theme.mode}</span>
      </div>
    </article>
  );
}

export default async function Home({
  searchParams,
}: {
  searchParams: Promise<{ download?: string }>;
}) {
  const { download } = await searchParams;

  return (
    <main id="top" className="min-h-screen bg-black text-white">
      {/* Header */}
      <header className="site-header">
        <div className="page-shell flex h-16 items-center justify-between">
          <Brand />
          <nav aria-label="Primary" className="hidden items-center gap-1 md:flex">
            <ButtonLink href="#product" size="sm" variant="ghost">Product</ButtonLink>
            <ButtonLink href="#safety" size="sm" variant="ghost">Safety</ButtonLink>
            <ButtonLink href="#themes" size="sm" variant="ghost">Themes</ButtonLink>
          </nav>
          <div className="flex items-center gap-2">
            <ButtonLink
              href={sourceUrl}
              size="sm"
              target="_blank"
              rel="noreferrer"
              variant="ghost"
              className="hidden sm:inline-flex"
            >
              <HugeiconsIcon icon={GithubIcon} size={15} aria-hidden="true" />
              GitHub
            </ButtonLink>
            <ButtonLink href="#download" size="sm">
              Download
            </ButtonLink>
          </div>
        </div>
      </header>

      {/* Hero */}
      <section className="hero-section">
        <div className="page-shell-wide relative">
          <div className="mx-auto max-w-4xl text-center">
            <p className="mb-6 text-xs font-medium tracking-[0.16em] text-[var(--bd-muted)] uppercase">
              Native · Open source · Local-first
            </p>
            <h1 className="hero-title text-balance">
              Enterprise-grade database work, without the cloud tax
            </h1>
            <p className="hero-subtitle mx-auto max-w-2xl">
              Explore schemas, run SQL, stage edits, and export complete datasets
              with production safeguards built into every connection.
            </p>
            <div className="hero-actions">
              <ButtonLink href="#download" size="lg">
                <HugeiconsIcon icon={Download04Icon} size={16} aria-hidden="true" />
                Download plusplus
              </ButtonLink>
              <ButtonLink
                href={sourceUrl}
                size="lg"
                target="_blank"
                rel="noreferrer"
                variant="secondary"
              >
                <HugeiconsIcon icon={GithubIcon} size={16} aria-hidden="true" />
                View source
              </ButtonLink>
            </div>
            <div className="hero-badges">
              {["No account", "No Electron", "No telemetry"].map((item) => (
                <span key={item} className="trust-badge">
                  <HugeiconsIcon icon={Tick02Icon} size={12} aria-hidden="true" />
                  {item}
                </span>
              ))}
            </div>
          </div>

          <div className="product-frame hero-shot mx-auto max-w-6xl">
            <Image
              src="/screenshots/image1.png"
              alt="plusplus entity relationship diagram"
              width={2720}
              height={1700}
              className="h-auto w-full"
              priority
            />
          </div>
        </div>
      </section>

      {/* Pillars — 3-column value props */}
      <section aria-label="Core values">
        <div className="pillar-grid">
          {pillars.map(({ title, text, variant }) => (
            <article key={title} className={`pillar-card pillar-card--${variant}`}>
              <h3>{title}</h3>
              <p>{text}</p>
            </article>
          ))}
        </div>
      </section>

      <div className="glow-separator my-[var(--section-y)]" aria-hidden="true" />

      {/* Database strip */}
      <section aria-label="Supported databases" className="db-strip">
        <div className="page-shell db-strip-inner">
          <p className="shrink-0 text-xs font-medium tracking-[0.16em] text-[var(--bd-muted)] uppercase">
            Supported engines
          </p>
          <div className="db-list">
            {databases.map(({ name, icon }) => (
              <div key={name} className="db-item">
                <Image src={icon} alt="" width={24} height={24} className="size-6" />
                <span>{name}</span>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Features — 4-column cards */}
      <section className="section-block">
        <div className="page-shell">
          <div className="section-intro text-center">
            <h2 className="section-heading">Core capabilities</h2>
            <p className="section-lead">
              One workspace across every supported engine — schema browser, query
              editor, result grid, and shortcuts stay consistent from local SQLite
              to production clusters.
            </p>
          </div>

          <div className="feature-grid">
            {features.map(({ icon: Icon, title, text }) => (
              <article key={title} className="feature-card">
                <div className="feature-card__icon">
                  <HugeiconsIcon icon={Icon} size={20} aria-hidden="true" />
                </div>
                <h3>{title}</h3>
                <p>{text}</p>
              </article>
            ))}
          </div>
        </div>
      </section>

      {/* Product */}
      <section id="product" className="section-block border-t border-[var(--bd-border)] bg-[var(--bd-panel)]">
        <div className="page-shell-wide">
          <div className="split-section split-section--end section-intro">
            <div>
              <p className="section-eyebrow">Workflow</p>
              <h2 className="section-heading">
                Built for teams that treat production carefully
              </h2>
            </div>
            <p className="max-w-xl text-base leading-8 text-[var(--bd-muted)] lg:justify-self-end">
              plusplus combines a familiar SQL workspace with policies that make
              irreversible mistakes harder.
            </p>
          </div>

          <div className="product-frame">
            <Image
              src="/screenshots/image2.png"
              alt="plusplus query editor and result grid"
              width={2400}
              height={1500}
              className="h-auto w-full"
            />
          </div>
        </div>
      </section>

      {/* Safety */}
      <section id="safety" className="section-block">
        <div className="page-shell-wide">
          <div className="split-section section-intro">
            <div>
              <p className="section-eyebrow">Security model</p>
              <h2 className="section-heading">
                Guardrails for the queries that matter
              </h2>
              <p className="mt-5 max-w-lg text-base leading-8 text-[var(--bd-muted)]">
                Warnings stay visible. Read-only mode is enforced. Sensitive
                connection details never need to leave your device.
              </p>
              <Link
                href="https://github.com/HakimIno/plusplus/blob/main/SECURITY.md"
                target="_blank"
                rel="noreferrer"
                className="mt-8 inline-flex text-[var(--bd-accent)]"
              >
                Read the security model
                <Link.Icon />
              </Link>
            </div>

            <div className="overflow-hidden rounded-xl border border-[var(--bd-border)] bg-[var(--bd-panel)]">
              {safeguards.map(({ icon: Icon, title, text }) => (
                <div key={title} className="safeguard-item">
                  <span className="safeguard-item__icon">
                    <HugeiconsIcon icon={Icon} size={18} aria-hidden="true" />
                  </span>
                  <div>
                    <h3 className="text-base font-semibold">{title}</h3>
                    <p className="mt-1.5 text-sm leading-7 text-[var(--bd-muted)]">{text}</p>
                  </div>
                </div>
              ))}
            </div>
          </div>

          <div className="product-frame" style={{ marginTop: "var(--block-gap)" }}>
            <Image
              src="/screenshots/image3.png"
              alt="plusplus table editor with staged edits"
              width={1180}
              height={760}
              className="h-auto w-full"
            />
          </div>
        </div>
      </section>

      <div className="glow-separator mb-[var(--section-y)]" aria-hidden="true" />

      {/* Themes */}
      <section id="themes" className="section-block border-t border-[var(--bd-border)] bg-[var(--bd-panel)]">
        <div className="page-shell-wide">
          <div className="split-section split-section--end section-intro">
            <div>
              <div className="mb-4 inline-flex items-center gap-2 section-eyebrow">
                <HugeiconsIcon icon={PaintBrush01Icon} size={14} aria-hidden="true" />
                Themes
              </div>
              <h2 className="section-heading">
                Built-in moods. Custom themes via JSON.
              </h2>
            </div>
            <p className="max-w-lg text-base leading-8 text-[var(--bd-muted)]">
              Switch from bright and airy to deep-focus dark—or drop in a theme
              file and make every surface feel like yours.
            </p>
          </div>

          <div className="theme-track flex gap-6 overflow-x-auto pb-3">
            {themes.map((theme) => (
              <ThemePreview key={theme.name} theme={theme} />
            ))}
          </div>

          <div className="mt-10 flex flex-col gap-5 border-t border-[var(--bd-border)] pt-10 text-sm text-[var(--bd-muted)] sm:flex-row sm:items-center sm:justify-between">
            <p>Changes apply immediately · Custom themes need no recompile</p>
            <Link
              href="https://github.com/HakimIno/plusplus/blob/main/docs/THEMES.md"
              target="_blank"
              rel="noreferrer"
              className="text-[var(--bd-accent)]"
            >
              Create a custom theme
              <Link.Icon />
            </Link>
          </div>
        </div>
      </section>

      {/* Download */}
      <section id="download" className="section-block">
        <div className="page-shell">
          <div className="section-intro text-center">
            <p className="section-eyebrow">Get started</p>
            <h2 className="section-heading">Pick your platform. Keep your data.</h2>
            <p className="section-lead">
              The latest package downloads here. No account and no detour
              through a release screen.
            </p>
          </div>

          <div className="download-grid">
            {platforms.map(({ id, name, format, detail, icon: PlatformIcon }) => (
              <a key={id} href={`/download/${id}`} className="download-card">
                <div className="flex items-start justify-between">
                  <span className="inline-flex size-16 shrink-0 overflow-hidden rounded-xl">
                    <PlatformIcon className="size-full" />
                  </span>
                  <HugeiconsIcon icon={Download04Icon} size={18} aria-hidden="true" className="text-[var(--bd-muted)]" />
                </div>
                <h3>{name}</h3>
                <p>{format} · {detail}</p>
              </a>
            ))}
          </div>

          {download === "unavailable" ? (
            <Alert status="warning" className="mt-5">
              <Alert.Indicator />
              <Alert.Content>
                <Alert.Title>Download unavailable</Alert.Title>
                <Alert.Description>
                  The latest package could not be located. Please try again in a moment.
                </Alert.Description>
              </Alert.Content>
            </Alert>
          ) : null}

          <div style={{ marginTop: "var(--block-gap)" }}>
            <div className="section-intro text-center">
              <h2 className="section-heading">Supported engines</h2>
              <p className="section-lead">
                One native workspace from local SQLite files to production PostgreSQL,
                MySQL, SQL Server, and CQL clusters.
              </p>
            </div>

            <div className="engine-grid p-2">
              {databases.map(({ name, detail, icon }) => (
                <article key={name} className="engine-card">
                  <div className="engine-card__icon">
                    <Image src={icon} alt="" width={96} height={96} className="size-20" />
                  </div>
                  <div className="engine-card__body">
                    <h3>{name}</h3>
                    <p>{detail}</p>
                  </div>
                </article>
              ))}
            </div>
          </div>

          <p className="mt-10 text-center text-sm leading-7 text-[var(--bd-muted)]">
            Pre-1.0 software. Start read-only and keep a current backup.{" "}
            <Link
              href="https://github.com/HakimIno/plusplus/blob/main/docs/RELEASE_SIGNING.md"
              target="_blank"
              rel="noreferrer"
              className="text-[var(--bd-accent)]"
            >
              Verify a release
              <Link.Icon />
            </Link>
          </p>
        </div>
      </section>

      {/* Footer */}
      <footer className="site-footer">
        <div className="page-shell flex flex-col gap-10 py-16 md:flex-row md:items-end md:justify-between">
          <div>
            <Brand />
            <p className="mt-3 max-w-sm text-sm text-[var(--bd-muted)]">
              A production-safe native database client for macOS, Windows, and Linux.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
            <a href={sourceUrl} target="_blank" rel="noreferrer" className="footer-link">GitHub</a>
            <a href="https://github.com/HakimIno/plusplus/blob/main/ROADMAP.md" target="_blank" rel="noreferrer" className="footer-link">Roadmap</a>
            <a href="https://github.com/HakimIno/plusplus/blob/main/CONTRIBUTING.md" target="_blank" rel="noreferrer" className="footer-link">Contribute</a>
            <span className="text-sm text-[var(--bd-muted)]">MIT OR Apache-2.0</span>
          </div>
        </div>
      </footer>
    </main>
  );
}
