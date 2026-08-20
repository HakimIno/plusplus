import Image from "next/image";
import {
  Alert,
  Card,
  Chip,
  Link,
  Separator,
  Surface,
  buttonVariants,
  cn,
} from "@heroui/react";
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

const sourceUrl = "https://github.com/HakimIno/plusplus";

const databases = [
  {
    name: "PostgreSQL",
    detail: "Native protocol",
    icon: "/databases/postgresql.svg",
  },
  {
    name: "MySQL / MariaDB",
    detail: "Shared connection flow",
    icon: "/databases/mysql.svg",
  },
  {
    name: "SQL Server",
    detail: "TDS protocol",
    icon: "/databases/microsoftsqlserver.svg",
  },
  {
    name: "SQLite",
    detail: "Open a local file",
    icon: "/databases/sqlite.svg",
  },
  {
    name: "Cassandra",
    detail: "CQL native protocol",
    icon: "/databases/cassandra.svg",
  },
  {
    name: "ScyllaDB",
    detail: "CQL-compatible cluster",
    icon: "/databases/scylladb.svg",
  },
];

const platforms = [
  {
    id: "macos",
    name: "macOS",
    format: "Universal DMG",
    detail: "Apple Silicon + Intel",
    icon: AppleIcon,
  },
  {
    id: "windows",
    name: "Windows",
    format: "Portable ZIP",
    detail: "Windows x86_64",
    icon: WindowsNewIcon,
  },
  {
    id: "linux",
    name: "Linux",
    format: "AppImage",
    detail: "Linux x86_64",
    icon: ComputerTerminal01Icon,
  },
];

const pillars = [
  {
    title: "Safety-first policies",
    text: "Destructive SQL and missing WHERE clauses are flagged before they run. Production connections ask for confirmation.",
  },
  {
    title: "Local by design",
    text: "Queries, results, history, and credentials stay on your machine. Passwords live in the OS keychain.",
  },
  {
    title: "Native performance",
    text: "A focused Rust desktop app with no Electron, browser runtime, cloud account, or telemetry.",
  },
];

const features = [
  {
    icon: DatabaseIcon,
    title: "Schema browser",
    text: "Tables, columns, keys, indexes, views, routines, and triggers stay within reach.",
  },
  {
    icon: ComputerTerminal01Icon,
    title: "SQL editor",
    text: "One focused editor and the same shortcuts across every connection and dialect.",
  },
  {
    icon: GridTableIcon,
    title: "Staged edits",
    text: "Cell edits, inserts, and deletions stay staged until you save or discard them.",
  },
  {
    icon: FileExportIcon,
    title: "Streaming export",
    text: "Export full tables to CSV or JSON without loading the whole dataset into memory.",
  },
];

const safeguards = [
  {
    icon: SecurityCheckIcon,
    title: "Risk checks before execution",
    text: "Destructive SQL and UPDATE or DELETE without a WHERE clause are flagged before they run.",
  },
  {
    icon: SecurityLockIcon,
    title: "Read-only that blocks writes",
    text: "Read-only mode is enforced in the app and, where supported, in the database session.",
  },
  {
    icon: Key01Icon,
    title: "Credentials stay on device",
    text: "Passwords live in the OS keychain. Query history and optional audit logs remain local.",
  },
];

const themes = [
  {
    name: "Tidal Ledger",
    mode: "Light",
    base: "#F7FBFC",
    panel: "#EAF2F4",
    surface: "#DCE9EC",
    code: "#FCFEFF",
    text: "#16343A",
    weak: "#5B777C",
    accent: "#087F8C",
  },
  {
    name: "Lotus Dusk",
    mode: "Dark",
    base: "#15131C",
    panel: "#1C1926",
    surface: "#292335",
    code: "#100E16",
    text: "#F1EAF4",
    weak: "#897B91",
    accent: "#E18BB7",
  },
  {
    name: "Copper Circuit",
    mode: "Dark",
    base: "#10161B",
    panel: "#151E24",
    surface: "#202B32",
    code: "#0B1014",
    text: "#EDF1EF",
    weak: "#71827F",
    accent: "#D98A48",
  },
  {
    name: "Daylight",
    mode: "Light",
    base: "#F8FAFC",
    panel: "#EEF2F7",
    surface: "#E1E6EF",
    code: "#FFFFFF",
    text: "#1B2533",
    weak: "#718096",
    accent: "#4D6BFE",
  },
];

type Theme = (typeof themes)[number];

function Brand() {
  return (
    <a href="#top" className="inline-flex items-center gap-2.5" aria-label="plusplus home">
      <Image
        src="/app-icon.png"
        alt=""
        width={30}
        height={30}
        className="rounded-lg"
        priority
      />
      <span className="font-display text-[16px] font-semibold tracking-[-0.03em]">
        plusplus
      </span>
    </a>
  );
}

function ProductShot({
  src,
  alt,
  width,
  height,
  priority = false,
}: {
  src: string;
  alt: string;
  width: number;
  height: number;
  priority?: boolean;
}) {
  return (
    <Card className="overflow-hidden p-0 bg-none" >
      <Image
        src={src}
        alt={alt}
        width={width}
        height={height}
        className="h-auto w-full rounded-xl"
        priority={priority}
      />
    </Card>
  );
}

function ThemePreview({ theme }: { theme: Theme }) {
  return (
    <article className="group min-w-[250px] flex-1 sm:min-w-[280px]">
      <Card className="overflow-hidden p-1.5 transition-transform duration-300 group-hover:-translate-y-1">
        <div
          className="overflow-hidden rounded-[calc(var(--radius)*1.5)]"
          style={{ backgroundColor: theme.base, color: theme.text }}
        >
          <div
            className="flex h-8 items-center justify-between px-3"
            style={{ backgroundColor: theme.panel }}
          >
            <span className="font-mono text-[8px] opacity-60">schema · query</span>
            <span
              className="size-2 rounded-full"
              style={{ backgroundColor: theme.accent }}
            />
          </div>
          <div className="grid h-[150px] grid-cols-[34%_1fr]">
            <div className="space-y-2 p-3" style={{ backgroundColor: theme.panel }}>
              <span
                className="block h-4 rounded-md"
                style={{ backgroundColor: theme.surface }}
              />
              {[70, 86, 58].map((width) => (
                <span
                  key={width}
                  className="block h-1.5 rounded-full opacity-55"
                  style={{ width: `${width}%`, backgroundColor: theme.weak }}
                />
              ))}
            </div>
            <div className="p-3" style={{ backgroundColor: theme.code }}>
              <div className="flex gap-1.5">
                <span
                  className="h-1.5 w-10 rounded-full"
                  style={{ backgroundColor: theme.accent }}
                />
                <span
                  className="h-1.5 w-6 rounded-full opacity-40"
                  style={{ backgroundColor: theme.weak }}
                />
              </div>
              <div className="mt-5 grid grid-cols-3 gap-2">
                {[0, 1, 2].map((item) => (
                  <span
                    key={item}
                    className="h-10 rounded-md"
                    style={{ backgroundColor: theme.surface }}
                  />
                ))}
              </div>
            </div>
          </div>
        </div>
      </Card>
      <div className="mt-3 flex items-center justify-between px-1">
        <p className="text-sm font-medium">{theme.name}</p>
        <Chip size="sm" variant="soft">
          {theme.mode}
        </Chip>
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
    <main id="top" className="min-h-screen bg-background text-foreground">
      <header className="sticky top-0 z-50  bg-background/80 backdrop-blur-xl">
        <div className="mx-auto flex h-14 max-w-6xl items-center justify-between px-5 sm:px-6">
          <Brand />
          <nav
            aria-label="Primary"
            className="hidden items-center gap-1 md:flex"
          >
            <ButtonLink href="#product" size="sm" variant="ghost">
              Product
            </ButtonLink>
            <ButtonLink href="#safety" size="sm" variant="ghost">
              Safety
            </ButtonLink>
            <ButtonLink href="#themes" size="sm" variant="ghost">
              Themes
            </ButtonLink>
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

      <section className="hero-glow relative overflow-hidden">
        <div className="mx-auto max-w-6xl px-5 pt-16 pb-12 sm:px-6 sm:pt-24 sm:pb-16">
          <div className="mx-auto max-w-3xl text-center">
            <Chip color="accent" variant="soft" className="mb-6">
              Native · Open source · Local-first
            </Chip>
            <h1 className="font-display text-4xl font-semibold tracking-tight text-balance sm:text-6xl lg:text-[4.25rem] lg:leading-[1.05]">
              Enterprise-grade database work, without the cloud tax
            </h1>
            <p className="mx-auto mt-5 max-w-2xl text-base leading-7 text-muted sm:text-lg sm:leading-8">
              Explore schemas, run SQL, stage edits, and export complete datasets
              with production safeguards built into every connection.
            </p>
            <div className="mt-8 flex flex-col items-center justify-center gap-3 sm:flex-row">
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
            <div className="mt-6 flex flex-wrap items-center justify-center gap-2">
              {["No account", "No Electron", "No telemetry"].map((item) => (
                <Chip key={item} size="sm" variant="tertiary">
                  <HugeiconsIcon icon={Tick02Icon} size={12} aria-hidden="true" />
                  <Chip.Label>{item}</Chip.Label>
                </Chip>
              ))}
            </div>
          </div>

          <div className="product-frame mx-auto mt-14 max-w-5xl">
            <ProductShot
              src="/screenshots/image1.png"
              alt="plusplus entity relationship diagram"
              width={2720}
              height={1700}
              priority
            />
          </div>
        </div>
      </section>

      <Separator />

      <section aria-label="Supported databases" className="bg-surface">
        <div className="mx-auto flex max-w-6xl flex-col items-center gap-8 px-5 py-10 sm:px-6 md:flex-row md:justify-between">
          <p className="text-xs font-medium tracking-[0.14em] text-muted uppercase">
            Supported engines
          </p>
          <div className="flex flex-wrap items-center justify-center gap-x-8 gap-y-4">
            {databases.map(({ name, icon }) => (
              <div key={name} className="flex items-center gap-2.5">
                <Image src={icon} alt="" width={24} height={24} className="size-6" />
                <span className="text-sm font-medium text-foreground/90">{name}</span>
              </div>
            ))}
          </div>
        </div>
      </section>

      <Separator />

      <section className="py-20 sm:py-28">
        <div className="mx-auto max-w-6xl px-5 sm:px-6">
          <div className="mx-auto max-w-2xl text-center">
            <p className="text-sm font-medium text-accent">Platform</p>
            <h2 className="font-display mt-3 text-3xl font-semibold tracking-tight sm:text-5xl">
              Built for teams that treat production carefully
            </h2>
            <p className="mt-4 text-base leading-7 text-muted">
              plusplus combines a familiar SQL workspace with policies that make
              irreversible mistakes harder.
            </p>
          </div>

          <div className="mt-12 grid gap-4 md:grid-cols-3">
            {pillars.map(({ title, text }) => (
              <Card key={title} variant="secondary">
                <Card.Header>
                  <Card.Title>{title}</Card.Title>
                  <Card.Description>{text}</Card.Description>
                </Card.Header>
              </Card>
            ))}
          </div>
        </div>
      </section>

      <Separator />

      <section id="product" className="bg-surface py-20 sm:py-28">
        <div className="mx-auto max-w-6xl px-5 sm:px-6">
          <div className="grid gap-6 lg:grid-cols-[0.9fr_1.1fr] lg:items-end">
            <div>
              <p className="text-sm font-medium text-accent">Workflow</p>
              <h2 className="font-display mt-3 text-3xl font-semibold tracking-tight sm:text-5xl">
                One workspace across every supported engine
              </h2>
            </div>
            <p className="max-w-xl text-base leading-7 text-muted lg:justify-self-end">
              Schema browser, query editor, result grid, and shortcuts stay
              consistent from local SQLite to SQL and CQL production clusters.
            </p>
          </div>

          <div className="mt-12 grid gap-4 sm:grid-cols-2">
            {features.map(({ icon: Icon, title, text }) => (
              <Card key={title}>
                <Card.Header>
                  <span className="mb-3 inline-flex size-10 items-center justify-center rounded-xl bg-accent/15 text-accent">
                    <HugeiconsIcon icon={Icon} size={18} aria-hidden="true" />
                  </span>
                  <Card.Title>{title}</Card.Title>
                  <Card.Description>{text}</Card.Description>
                </Card.Header>
              </Card>
            ))}
          </div>

          <div className="mt-12">
            <ProductShot
              src="/screenshots/image2.png"
              alt="plusplus query editor and result grid"
              width={2400}
              height={1500}
            />
          </div>
        </div>
      </section>

      <Separator />

      <section id="safety" className="py-20 sm:py-28">
        <div className="mx-auto max-w-6xl px-5 sm:px-6">
          <div className="grid gap-12 lg:grid-cols-2 lg:gap-16">
            <div>
              <p className="text-sm font-medium text-accent">Security model</p>
              <h2 className="font-display mt-3 text-3xl font-semibold tracking-tight sm:text-5xl">
                Guardrails for the queries that matter
              </h2>
              <p className="mt-4 max-w-md text-base leading-7 text-muted">
                Warnings stay visible. Read-only mode is enforced. Sensitive
                connection details never need to leave your device.
              </p>
              <Link
                href="https://github.com/HakimIno/plusplus/blob/main/SECURITY.md"
                target="_blank"
                rel="noreferrer"
                className="mt-6 inline-flex"
              >
                Read the security model
                <Link.Icon />
              </Link>
            </div>

            <Surface variant="secondary" className="overflow-hidden rounded-2xl p-0">
              {safeguards.map(({ icon: Icon, title, text }, index) => (
                <div key={title}>
                  {index > 0 ? <Separator variant="secondary" /> : null}
                  <div className="grid grid-cols-[40px_1fr] gap-4 p-5 sm:p-6">
                    <span className="inline-flex size-10 items-center justify-center rounded-xl bg-accent/15 text-accent">
                      <HugeiconsIcon icon={Icon} size={18} aria-hidden="true" />
                    </span>
                    <div>
                      <h3 className="text-[15px] font-semibold">{title}</h3>
                      <p className="mt-1 text-sm leading-6 text-muted">{text}</p>
                    </div>
                  </div>
                </div>
              ))}
            </Surface>
          </div>

          <div className="mt-12">
            <ProductShot
              src="/screenshots/image3.png"
              alt="plusplus table editor with staged edits"
              width={1180}
              height={760}
            />
          </div>
        </div>
      </section>

      <Separator />

      <section id="themes" className="bg-surface py-20 sm:py-28">
        <div className="mx-auto max-w-6xl px-5 sm:px-6">
          <div className="grid gap-6 lg:grid-cols-[1fr_0.85fr] lg:items-end">
            <div>
              <div className="mb-3 inline-flex items-center gap-2 text-sm font-medium text-accent">
                <HugeiconsIcon
                  icon={PaintBrush01Icon}
                  size={14}
                  aria-hidden="true"
                />
                Themes
              </div>
              <h2 className="font-display text-3xl font-semibold tracking-tight sm:text-5xl">
                Built-in moods. Custom themes via JSON.
              </h2>
            </div>
            <p className="max-w-md text-base leading-7 text-muted">
              Switch from bright and airy to deep-focus dark—or drop in a theme
              file and make every surface feel like yours.
            </p>
          </div>

          <div className="theme-track mt-12 flex gap-4 overflow-x-auto pb-2">
            {themes.map((theme) => (
              <ThemePreview key={theme.name} theme={theme} />
            ))}
          </div>

          <Separator className="my-8" />

          <div className="flex flex-col gap-4 text-sm text-muted sm:flex-row sm:items-center sm:justify-between">
            <p>Changes apply immediately · Custom themes need no recompile</p>
            <Link
              href="https://github.com/HakimIno/plusplus/blob/main/docs/THEMES.md"
              target="_blank"
              rel="noreferrer"
            >
              Create a custom theme
              <Link.Icon />
            </Link>
          </div>
        </div>
      </section>

      <Separator />

      <section id="download" className="bg-surface py-20 sm:py-28">
        <div className="mx-auto max-w-6xl px-5 sm:px-6">
          <Card className="overflow-hidden" variant="tertiary">
            <Card.Header className="gap-3 p-6 sm:p-10 lg:p-12">
              <Chip color="accent" size="sm" variant="soft">
                Get started
              </Chip>
              <Card.Title className="font-display text-3xl sm:text-5xl">
                Pick your platform. Keep your data.
              </Card.Title>
              <Card.Description className="max-w-xl text-base">
                The latest package downloads here. No account and no detour
                through a release screen.
              </Card.Description>
            </Card.Header>

            <Card.Content className="px-6 pb-6 sm:px-10 sm:pb-10 lg:px-12 lg:pb-12">
              <div className="grid gap-3 md:grid-cols-3">
                {platforms.map(({ id, name, format, detail, icon: Icon }) => (
                  <a
                    key={id}
                    href={`/download/${id}`}
                    className="block rounded-[calc(var(--radius)*1.5)] no-underline outline-none transition-transform hover:-translate-y-0.5 focus-visible:ring-2 focus-visible:ring-focus"
                  >
                    <Card className="h-full">
                      <Card.Header>
                        <div className="mb-8 flex items-start justify-between">
                          <span className="inline-flex size-10 items-center justify-center rounded-xl bg-default">
                            <HugeiconsIcon
                              icon={Icon}
                              size={18}
                              aria-hidden="true"
                            />
                          </span>
                          <HugeiconsIcon
                            icon={Download04Icon}
                            size={16}
                            aria-hidden="true"
                            className="text-muted"
                          />
                        </div>
                        <Card.Title>{name}</Card.Title>
                        <Card.Description>
                          {format} · {detail}
                        </Card.Description>
                      </Card.Header>
                    </Card>
                  </a>
                ))}
              </div>

              {download === "unavailable" ? (
                <Alert status="warning" className="mt-5">
                  <Alert.Indicator />
                  <Alert.Content>
                    <Alert.Title>Download unavailable</Alert.Title>
                    <Alert.Description>
                      The latest package could not be located. Please try again
                      in a moment.
                    </Alert.Description>
                  </Alert.Content>
                </Alert>
              ) : null}

              <Separator className="my-8" />

              <div className="flex flex-col gap-4 text-sm text-muted sm:flex-row sm:items-center sm:justify-between">
                <p>
                  Pre-1.0 software. Start read-only and keep a current backup.
                </p>
                <Link
                  href="https://github.com/HakimIno/plusplus/blob/main/docs/RELEASE_SIGNING.md"
                  target="_blank"
                  rel="noreferrer"
                >
                  Verify a release
                  <Link.Icon />
                </Link>
              </div>
            </Card.Content>
          </Card>

          <div className="mt-8 grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {databases.map(({ name, detail, icon }) => (
              <Card key={name} variant="secondary">
                <Card.Header className="flex-row items-center gap-3">
                  <Image src={icon} alt="" width={32} height={32} className="size-8" />
                  <div>
                    <Card.Title className="text-sm">{name}</Card.Title>
                    <Card.Description className="text-xs">{detail}</Card.Description>
                  </div>
                </Card.Header>
              </Card>
            ))}
          </div>
        </div>
      </section>

      <Separator />

      <footer className="bg-background">
        <div className="mx-auto flex max-w-6xl flex-col gap-8 px-5 py-12 sm:px-6 md:flex-row md:items-end md:justify-between">
          <div>
            <Brand />
            <p className="mt-3 max-w-sm text-sm text-muted">
              A production-safe native database client for macOS, Windows, and
              Linux.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-x-5 gap-y-2 text-sm text-muted">
            <Link href={sourceUrl} target="_blank" rel="noreferrer" className="no-underline">
              GitHub
            </Link>
            <Link
              href="https://github.com/HakimIno/plusplus/blob/main/ROADMAP.md"
              target="_blank"
              rel="noreferrer"
              className="no-underline"
            >
              Roadmap
            </Link>
            <Link
              href="https://github.com/HakimIno/plusplus/blob/main/CONTRIBUTING.md"
              target="_blank"
              rel="noreferrer"
              className="no-underline"
            >
              Contribute
            </Link>
            <span>MIT OR Apache-2.0</span>
          </div>
        </div>
      </footer>
    </main>
  );
}
