import Image from "next/image";
import { Alert, Link, buttonVariants, cn } from "@heroui/react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AppleIcon,
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
  WindowsNewIcon,
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
  { name: "PostgreSQL", detail: "Native protocol", icon: "/databases/postgresql.svg" },
  { name: "MySQL / MariaDB", detail: "Shared connection flow", icon: "/databases/mysql.svg" },
  { name: "SQL Server", detail: "TDS protocol", icon: "/databases/microsoftsqlserver.svg" },
  { name: "SQLite", detail: "Open a local file", icon: "/databases/sqlite.svg" },
  { name: "Cassandra", detail: "CQL native protocol", icon: "/databases/cassandra.svg" },
  { name: "ScyllaDB", detail: "CQL-compatible cluster", icon: "/databases/scylladb.svg" },
];

const platforms = [
  { id: "macos", name: "macOS", format: "Universal DMG", detail: "Apple Silicon + Intel", icon: AppleIcon },
  { id: "windows", name: "Windows", format: "Portable ZIP", detail: "Windows x86_64", icon: WindowsNewIcon },
  { id: "linux", name: "Linux", format: "AppImage", detail: "Linux x86_64", icon: ComputerTerminal01Icon },
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
            {platforms.map(({ id, name, format, detail, icon: Icon }) => (
              <a key={id} href={`/download/${id}`} className="download-card">
                <div className="flex items-start justify-between">
                  <span className="inline-flex size-10 items-center justify-center rounded-lg border border-[var(--bd-border)] bg-[var(--bd-elevated)]">
                    <HugeiconsIcon icon={Icon} size={18} aria-hidden="true" />
                  </span>
                  <HugeiconsIcon icon={Download04Icon} size={16} aria-hidden="true" className="text-[var(--bd-muted)]" />
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

          <div className="engine-grid" style={{ marginTop: "var(--block-gap)" }}>
            {databases.map(({ name, detail, icon }) => (
              <div key={name} className="engine-card">
                <Image src={icon} alt="" width={32} height={32} className="size-8" />
                <div>
                  <p className="text-sm font-medium">{name}</p>
                  <p className="mt-0.5 text-sm text-[var(--bd-muted)]">{detail}</p>
                </div>
              </div>
            ))}
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
