import Image from "next/image";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  AppleIcon,
  ArrowDown02Icon,
  ArrowRight01Icon,
  ArrowRight02Icon,
  ComputerTerminal01Icon,
  DatabaseIcon,
  Download04Icon,
  ExternalLinkIcon,
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

const databases = [
  {
    name: "PostgreSQL",
    detail: "Server database",
    protocol: "Native protocol",
    icon: "/databases/postgresql.svg",
    color: "#4169E1",
    iconBackground: "#E9EEFF",
  },
  {
    name: "MySQL / MariaDB",
    detail: "Shared connection flow",
    protocol: "MySQL protocol",
    icon: "/databases/mysql.svg",
    color: "#4479A1",
    iconBackground: "#E7F2F5",
  },
  {
    name: "SQL Server",
    detail: "Microsoft databases",
    protocol: "TDS protocol",
    icon: "/databases/microsoftsqlserver.svg",
    color: "#CC2927",
    iconBackground: "#FBE9E6",
  },
  {
    name: "SQLite",
    detail: "Open a local file",
    protocol: "Embedded",
    icon: "/databases/sqlite.svg",
    color: "#0F80CC",
    iconBackground: "#E5F3F8",
  },
];

const platforms = [
  {
    id: "macos",
    name: "macOS",
    format: "Universal DMG",
    detail: "Apple Silicon + Intel",
    icon: AppleIcon,
    color: "#D2F36A",
  },
  {
    id: "windows",
    name: "Windows",
    format: "Portable ZIP",
    detail: "Windows x86_64",
    icon: WindowsNewIcon,
    color: "#FF8F78",
  },
  {
    id: "linux",
    name: "Linux",
    format: "AppImage",
    detail: "Linux x86_64",
    icon: ComputerTerminal01Icon,
    color: "#B8A6FF",
  },
];

const features = [
  {
    icon: DatabaseIcon,
    label: "SCHEMA",
    title: "See the whole schema",
    text: "Tables, columns, keys, indexes, views, routines, and triggers stay within reach.",
    color: "#DFF4EF",
    accent: "#087F8C",
  },
  {
    icon: ComputerTerminal01Icon,
    label: "SQL",
    title: "Write SQL without friction",
    text: "One focused editor and the same keyboard shortcuts across every connection.",
    color: "#E7E1FF",
    accent: "#624CF2",
  },
  {
    icon: GridTableIcon,
    label: "DATA",
    title: "Change data deliberately",
    text: "Stage cell edits, inserted rows, and deletions before saving them together.",
    color: "#FFE1D9",
    accent: "#C94933",
  },
  {
    icon: FileExportIcon,
    label: "EXPORT",
    title: "Export every row",
    text: "Stream full tables to CSV or JSON without holding the whole dataset in memory.",
    color: "#F0F7C7",
    accent: "#597116",
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
    title: "Credentials stay on your device",
    text: "Passwords live in the OS keychain. Query history and optional audit logs remain local.",
  },
];

const themes = [
  {
    name: "Tidal Ledger",
    mode: "LIGHT",
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
    mode: "DARK",
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
    mode: "DARK",
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
    mode: "LIGHT",
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

function Brand({ inverse = false }: { inverse?: boolean }) {
  return (
    <a
      href="#top"
      className={`inline-flex items-center gap-2.5 ${inverse ? "text-[#F7F1E8]" : ""}`}
      aria-label="plusplus home"
    >
      <Image
        src="/app-icon.png"
        alt=""
        width={34}
        height={34}
        className="rounded-[9px]"
        priority
      />
      <span className="text-[18px] font-semibold tracking-[-0.035em]">
        plusplus
      </span>
    </a>
  );
}

function AppFrame({
  src,
  alt,
  width,
  height,
  priority = false,
  className = "",
}: {
  src: string;
  alt: string;
  width: number;
  height: number;
  priority?: boolean;
  className?: string;
}) {
  return (
    <div
      className={`overflow-hidden rounded-[18px] border-2 border-[#16232A] bg-[#0E1115] p-1.5 shadow-[0_24px_70px_rgba(22,35,42,0.22)] sm:rounded-[24px] sm:p-2 ${className}`}
    >
      <div className="flex h-8 items-center gap-1.5 px-3 sm:h-10">
        <span className="h-2.5 w-2.5 rounded-full bg-[#FF7C65]" />
        <span className="h-2.5 w-2.5 rounded-full bg-[#F5CF54]" />
        <span className="h-2.5 w-2.5 rounded-full bg-[#D2F36A]" />
      </div>
      <Image
        src={src}
        alt={alt}
        width={width}
        height={height}
        className="h-auto w-full rounded-[10px] border border-white/10 sm:rounded-[14px]"
        priority={priority}
      />
    </div>
  );
}

function QueryTrail({ className = "" }: { className?: string }) {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 360 100"
      className={className}
      fill="none"
    >
      <path
        d="M14 56C72 2 102 96 165 50C228 4 267 90 345 34"
        stroke="currentColor"
        strokeWidth="2"
        strokeDasharray="7 8"
        strokeLinecap="round"
      />
      <circle cx="14" cy="56" r="7" fill="#FF8F78" stroke="#16232A" strokeWidth="2" />
      <circle cx="165" cy="50" r="7" fill="#D2F36A" stroke="#16232A" strokeWidth="2" />
      <circle cx="345" cy="34" r="7" fill="#B8A6FF" stroke="#16232A" strokeWidth="2" />
    </svg>
  );
}

function ThemePreview({ theme }: { theme: Theme }) {
  return (
    <article className="group min-w-[270px] flex-1 sm:min-w-[310px]">
      <div
        className="overflow-hidden rounded-[20px] border border-white/20 p-2 transition-transform duration-300 group-hover:-translate-y-2"
        style={{ backgroundColor: theme.base, color: theme.text }}
      >
        <div
          className="flex h-8 items-center justify-between rounded-t-[13px] px-3"
          style={{ backgroundColor: theme.panel }}
        >
          <span className="font-mono text-[8px] opacity-70">one | shop</span>
          <span className="h-2 w-2 rounded-full" style={{ backgroundColor: theme.accent }} />
        </div>
        <div className="grid h-[172px] grid-cols-[34%_1fr]">
          <div className="space-y-2 p-3" style={{ backgroundColor: theme.panel }}>
            <span
              className="block h-5 rounded"
              style={{ backgroundColor: theme.surface }}
            />
            {[70, 86, 58, 76].map((width) => (
              <span
                key={width}
                className="block h-1.5 rounded-full opacity-60"
                style={{ width: `${width}%`, backgroundColor: theme.weak }}
              />
            ))}
          </div>
          <div className="p-3" style={{ backgroundColor: theme.code }}>
            <div className="flex items-center gap-1.5">
              <span className="h-1.5 w-12 rounded-full" style={{ backgroundColor: theme.accent }} />
              <span className="h-1.5 w-8 rounded-full opacity-50" style={{ backgroundColor: theme.weak }} />
            </div>
            <div className="mt-7 grid grid-cols-3 gap-2">
              {[0, 1, 2].map((item) => (
                <span
                  key={item}
                  className="h-14 rounded-md border opacity-90"
                  style={{ backgroundColor: theme.surface, borderColor: theme.weak }}
                />
              ))}
            </div>
            <div className="mt-3 h-7 rounded-md" style={{ backgroundColor: theme.surface }} />
          </div>
        </div>
      </div>
      <div className="mt-4 flex items-center justify-between">
        <h3 className="text-[15px] font-semibold text-[#F7F1E8]">{theme.name}</h3>
        <span className="font-mono text-[10px] tracking-[0.12em] text-[#909A9E]">
          {theme.mode}
        </span>
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
    <main id="top" className="overflow-hidden bg-[#F7F1E8] text-[#16232A]">
      <header className="sticky top-0 z-50 border-b border-[#16232A]/15 bg-[#F7F1E8]/92 backdrop-blur-md">
        <nav
          aria-label="Primary navigation"
          className="mx-auto flex h-[72px] max-w-[1280px] items-center justify-between px-5 sm:px-8"
        >
          <Brand />
          <div className="hidden items-center gap-8 text-[13px] font-semibold md:flex">
            <a href="#product" className="transition-opacity hover:opacity-55">Product</a>
            <a href="#themes" className="transition-opacity hover:opacity-55">Themes</a>
            <a href="#safety" className="transition-opacity hover:opacity-55">Safety</a>
          </div>
          <a
            href="#download"
            className="inline-flex h-10 items-center gap-2 rounded-full border-2 border-[#16232A] bg-[#16232A] px-4 text-[13px] font-semibold text-white transition-transform hover:-translate-y-0.5"
          >
            Download
            <HugeiconsIcon icon={ArrowDown02Icon} size={14} aria-hidden="true" />
          </a>
        </nav>
      </header>

      <section className="px-3 pt-3 sm:px-5 sm:pt-5">
        <div className="mx-auto max-w-[1440px] overflow-hidden rounded-[28px] border-2 border-[#16232A] bg-[#F8D85E]">
          <div className="flex min-h-9 items-center justify-between border-b-2 border-[#16232A] px-5 py-2 font-mono text-[9px] tracking-[0.12em] sm:px-8 sm:text-[10px]">
            <span>LOCAL-FIRST DATABASE WORKSPACE</span>
            <span className="hidden sm:inline">MACOS · WINDOWS · LINUX</span>
          </div>

          <div className="grid lg:grid-cols-[0.92fr_1.08fr]">
            <div className="relative flex flex-col justify-center px-6 py-16 sm:px-10 sm:py-20 lg:min-h-[690px] lg:px-16">
              <div className="absolute top-8 right-10 hidden h-5 w-5 rotate-12 border-2 border-[#16232A] bg-[#FF8F78] sm:block" />
              <p className="flex items-center gap-2 text-[12px] font-semibold">
                <span className="h-2.5 w-2.5 rounded-full bg-[#4968F2]" />
                Native, open source, and quietly fast
              </p>
              <h1 className="mt-7 max-w-[700px] text-[48px] leading-[0.92] font-semibold tracking-[-0.065em] sm:text-[76px] lg:text-[84px]">
                Move fast
                <br />
                in data.
                <span className="mt-2 block w-fit -rotate-1 rounded-[16px] border-2 border-[#16232A] bg-[#FF8F78] px-3 pb-2 sm:px-5">
                  Stay careful.
                </span>
              </h1>
              <p className="mt-8 max-w-[580px] text-[17px] leading-7 sm:text-[19px] sm:leading-8">
                Explore schemas, run SQL, stage edits, and export complete datasets
                without sending your work to a third party.
              </p>
              <div className="mt-9 flex flex-col gap-3 sm:flex-row">
                <a
                  href="#download"
                  className="inline-flex h-12 items-center justify-center gap-2 rounded-full bg-[#16232A] px-6 text-[14px] font-semibold text-white transition-transform hover:-translate-y-0.5"
                >
                  Download plusplus
                  <HugeiconsIcon icon={Download04Icon} size={16} aria-hidden="true" />
                </a>
                <a
                  href={sourceUrl}
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex h-12 items-center justify-center gap-2 rounded-full border-2 border-[#16232A] bg-[#F7F1E8] px-6 text-[14px] font-semibold transition-transform hover:-translate-y-0.5"
                >
                  <HugeiconsIcon icon={GithubIcon} size={16} aria-hidden="true" />
                  View source
                  <HugeiconsIcon icon={ExternalLinkIcon} size={14} aria-hidden="true" />
                </a>
              </div>
              <div className="mt-8 flex flex-wrap gap-x-5 gap-y-2 text-[11px] font-semibold">
                {["No account", "No Electron", "No telemetry"].map((item) => (
                  <span key={item} className="flex items-center gap-1.5">
                    <HugeiconsIcon icon={Tick02Icon} size={14} aria-hidden="true" />
                    {item}
                  </span>
                ))}
              </div>
            </div>

            <div className="relative min-h-[520px] overflow-hidden border-t-2 border-[#16232A] bg-[#4968F2] p-5 sm:min-h-[620px] sm:p-10 lg:min-h-[690px] lg:border-t-0 lg:border-l-2 lg:p-12">
              <QueryTrail className="absolute -top-1 -right-10 h-auto w-[75%] text-white/75" />
              <span className="absolute top-24 right-7 rotate-6 rounded-full border-2 border-[#16232A] bg-[#D2F36A] px-4 py-2 font-mono text-[10px] font-semibold">
                READ-ONLY ON
              </span>
              <AppFrame
                src="/screenshots/erd.png"
                alt="plusplus showing a visual entity relationship diagram"
                width={2720}
                height={1700}
                priority
                className="hero-window relative mt-20 w-[112%] max-w-none -rotate-2 sm:mt-24"
              />
              <div className="absolute right-5 bottom-6 max-w-[220px] rotate-2 rounded-[16px] border-2 border-[#16232A] bg-[#F7F1E8] p-4 shadow-[5px_5px_0_#16232A] sm:right-10 sm:bottom-10">
                <p className="font-mono text-[9px] tracking-[0.12em] text-[#657276]">BEFORE EXECUTION</p>
                <p className="mt-2 text-[13px] leading-5 font-semibold">
                  Destructive queries pause for a second look.
                </p>
              </div>
            </div>
          </div>
        </div>
      </section>

      <section
        aria-labelledby="databases-heading"
        className="database-stage border-y-2 border-[#16232A] bg-[#16232A] text-[#F7F1E8]"
      >
        <div className="relative mx-auto max-w-[1240px] px-5 py-20 sm:px-8 sm:py-24">
          <div className="grid gap-7 lg:grid-cols-[0.95fr_1.05fr] lg:items-end">
            <div>
              <p className="font-mono text-[10px] tracking-[0.12em] text-[#B8A6FF]">
                SUPPORTED CONNECTIONS
              </p>
              <h2
                id="databases-heading"
                className="mt-4 max-w-[680px] text-[38px] leading-[1.02] font-semibold tracking-[-0.05em] sm:text-[54px]"
              >
                Four engines.
                <br />
                One familiar workflow.
              </h2>
            </div>
            <p className="max-w-[570px] text-[15px] leading-7 text-[#B6C0C3] lg:justify-self-end">
              Move between production servers and local files without relearning
              the schema browser, query editor, or result grid.
            </p>
          </div>

          <div className="mt-12 grid grid-cols-2 gap-3 sm:gap-4 lg:grid-cols-4">
            {databases.map(
              ({ name, detail, protocol, icon, color, iconBackground }) => (
                <article
                  key={name}
                  className="group relative flex min-h-[238px] flex-col overflow-hidden rounded-[20px] border-2 border-[#F7F1E8]/70 border-t-[5px] bg-[#F7F1E8] p-4 text-[#16232A] transition-[transform,box-shadow] duration-300 hover:-translate-y-1.5 hover:shadow-[0_8px_0_rgba(247,241,232,0.16)] sm:min-h-[270px] sm:p-6"
                  style={{ borderTopColor: color }}
                >
                  <div className="flex items-start justify-between gap-3">
                    <span
                      className="flex h-[70px] w-[70px] items-center justify-center rounded-[16px] border border-[#16232A]/15 sm:h-[84px] sm:w-[84px]"
                      style={{ backgroundColor: iconBackground }}
                    >
                      <Image
                        src={icon}
                        alt=""
                        width={58}
                        height={58}
                        className="h-[46px] w-[46px] object-contain sm:h-[58px] sm:w-[58px]"
                      />
                    </span>
                    <span className="mt-1 hidden items-center gap-1.5 font-mono text-[8px] tracking-[0.1em] text-[#526166] sm:flex">
                      <span className="h-2 w-2 rounded-full bg-[#5F8E2F]" />
                      SUPPORTED
                    </span>
                  </div>

                  <div className="mt-auto pt-8">
                    <h3 className="text-[16px] leading-tight font-semibold tracking-[-0.02em] sm:text-[19px]">
                      {name}
                    </h3>
                    <p className="mt-2 text-[11px] leading-5 text-[#526166] sm:text-[13px]">
                      {detail}
                    </p>
                    <p className="mt-3 border-t border-[#16232A]/15 pt-3 font-mono text-[8px] tracking-[0.08em] text-[#657276] sm:text-[9px]">
                      {protocol}
                    </p>
                  </div>
                </article>
              ),
            )}
          </div>
        </div>
      </section>

      <section id="product" className="bg-[#FFFCF7]">
        <div className="mx-auto max-w-[1240px] px-5 py-24 sm:px-8 sm:py-32">
          <div className="grid gap-8 lg:grid-cols-[0.8fr_1.2fr] lg:gap-24">
            <div>
              <p className="font-mono text-[10px] tracking-[0.12em] text-[#657276]">YOUR DAILY WORKBENCH</p>
              <h2 className="mt-5 text-[44px] leading-[1] font-semibold tracking-[-0.055em] sm:text-[64px]">
                Less tool.
                <br />
                More flow.
              </h2>
            </div>
            <div className="flex items-end">
              <p className="max-w-[680px] text-[18px] leading-8 text-[#526166]">
                The schema browser, query editor, result grid, and shortcuts stay
                familiar. The interface steps back until you need it.
              </p>
            </div>
          </div>

          <div className="mt-16 grid gap-4 sm:grid-cols-2">
            {features.map(({ icon: Icon, label, title, text, color, accent }, index) => (
              <article
                key={title}
                className={`relative min-h-[270px] overflow-hidden rounded-[24px] border-2 border-[#16232A] p-7 sm:p-9 ${
                  index === 0 ? "sm:col-span-2 lg:col-span-1 lg:row-span-2 lg:min-h-[556px]" : ""
                }`}
                style={{ backgroundColor: color }}
              >
                <div className="flex items-start justify-between">
                  <span
                    className="flex h-11 w-11 items-center justify-center rounded-full text-white"
                    style={{ backgroundColor: accent }}
                  >
                    <HugeiconsIcon icon={Icon} size={20} aria-hidden="true" />
                  </span>
                  <span className="font-mono text-[9px] tracking-[0.12em] opacity-55">{label}</span>
                </div>
                <div className={index === 0 ? "mt-24 sm:mt-36" : "mt-14"}>
                  <h3 className={index === 0 ? "text-[32px] font-semibold tracking-[-0.04em]" : "text-[22px] font-semibold tracking-[-0.03em]"}>
                    {title}
                  </h3>
                  <p className="mt-3 max-w-[430px] text-[14px] leading-6 text-[#526166]">
                    {text}
                  </p>
                </div>
              </article>
            ))}
          </div>
        </div>
      </section>

      <section id="themes" className="bg-[#16232A] text-[#F7F1E8]">
        <div className="mx-auto max-w-[1440px] px-5 py-24 sm:px-8 sm:py-32">
          <div className="mx-auto max-w-[1240px]">
            <div className="grid gap-10 lg:grid-cols-[1fr_0.8fr] lg:items-end">
              <div>
                <p className="flex items-center gap-2 font-mono text-[10px] tracking-[0.12em] text-[#AAB4B7]">
                  <HugeiconsIcon
                    icon={PaintBrush01Icon}
                    size={16}
                    aria-hidden="true"
                    className="text-[#B8A6FF]"
                  />
                  WORK YOUR WAY
                </p>
                <h2 className="mt-5 max-w-[760px] text-[45px] leading-[0.98] font-semibold tracking-[-0.055em] sm:text-[66px]">
                  Seven built-ins.
                  <br />
                  Infinite custom moods.
                </h2>
              </div>
              <p className="max-w-[550px] text-[16px] leading-7 text-[#B6C0C3]">
                Switch from bright and airy to deep-focus dark. Or drop in a small
                JSON file and make every surface feel like yours.
              </p>
            </div>

            <div className="theme-track mt-16 flex gap-5 overflow-x-auto pb-6">
              {themes.map((theme) => (
                <ThemePreview key={theme.name} theme={theme} />
              ))}
            </div>

            <div className="mt-8 flex flex-col gap-5 border-t border-white/15 pt-7 text-[12px] text-[#AAB4B7] sm:flex-row sm:items-center sm:justify-between">
              <p>Changes apply immediately · Custom themes need no recompile</p>
              <a
                href="https://github.com/HakimIno/plusplus/blob/main/docs/THEMES.md"
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-2 font-semibold text-white"
              >
                Create a custom theme
                <HugeiconsIcon icon={ArrowRight02Icon} size={16} aria-hidden="true" />
              </a>
            </div>
          </div>
        </div>
      </section>

      <section id="safety" className="bg-[#D2F36A]">
        <div className="mx-auto grid max-w-[1240px] gap-14 px-5 py-24 sm:px-8 sm:py-32 lg:grid-cols-[0.82fr_1.18fr] lg:gap-20">
          <div>
            <p className="font-mono text-[10px] tracking-[0.12em] text-[#526166]">PRODUCTION SAFETY</p>
            <h2 className="mt-5 text-[44px] leading-[1] font-semibold tracking-[-0.055em] sm:text-[62px]">
              A seatbelt for your SQL.
            </h2>
            <p className="mt-6 max-w-[520px] text-[16px] leading-7 text-[#455257]">
              Warnings stay visible. Read-only mode is enforced. Sensitive
              connection details never need to leave your device.
            </p>
            <a
              href="https://github.com/HakimIno/plusplus/blob/main/SECURITY.md"
              target="_blank"
              rel="noreferrer"
              className="mt-8 inline-flex items-center gap-2 text-[14px] font-semibold underline decoration-[#70823A] underline-offset-6"
            >
              Read the security model
              <HugeiconsIcon icon={ArrowRight02Icon} size={16} aria-hidden="true" />
            </a>
          </div>

          <div className="overflow-hidden rounded-[24px] border-2 border-[#16232A] bg-[#F7F1E8]">
            {safeguards.map(({ icon: Icon, title, text }, index) => (
              <article
                key={title}
                className={`grid grid-cols-[46px_1fr] gap-5 p-6 sm:p-8 ${
                  index > 0 ? "border-t-2 border-[#16232A]" : ""
                }`}
              >
                <span className="flex h-11 w-11 items-center justify-center rounded-full bg-[#16232A] text-[#D2F36A]">
                  <HugeiconsIcon icon={Icon} size={20} aria-hidden="true" />
                </span>
                <div>
                  <h3 className="text-[17px] font-semibold">{title}</h3>
                  <p className="mt-2 max-w-[560px] text-[14px] leading-6 text-[#526166]">
                    {text}
                  </p>
                </div>
              </article>
            ))}
          </div>

          <div className="relative lg:col-span-2">
            <div className="absolute -top-4 left-6 z-10 rotate-[-2deg] rounded-full border-2 border-[#16232A] bg-[#FF8F78] px-4 py-2 font-mono text-[9px] font-semibold tracking-[0.1em]">
              STAGED CHANGES
            </div>
            <AppFrame
              src="/screenshots/table-editor.png"
              alt="plusplus table editor showing foreign key controls"
              width={1180}
              height={760}
            />
          </div>
        </div>
      </section>

      <section className="bg-[#FFFCF7]">
        <div className="mx-auto grid max-w-[1240px] items-center gap-14 px-5 py-24 sm:px-8 sm:py-32 lg:grid-cols-[1.05fr_0.95fr] lg:gap-20">
          <div className="relative">
            <div className="absolute -inset-3 rotate-2 rounded-[26px] border-2 border-[#16232A] bg-[#B8A6FF]" />
            <div className="relative overflow-hidden rounded-[22px] border-2 border-[#16232A] bg-[#F7F1E8] p-2">
              <Image
                src="/dmg-background.svg"
                alt="The plusplus macOS installer artwork"
                width={660}
                height={400}
                className="h-auto w-full rounded-[14px]"
              />
            </div>
          </div>
          <div>
            <p className="font-mono text-[10px] tracking-[0.12em] text-[#657276]">NATIVE FROM THE FIRST CLICK</p>
            <h2 className="mt-5 text-[41px] leading-[1.02] font-semibold tracking-[-0.05em] sm:text-[57px]">
              Built for your machine, not a browser tab.
            </h2>
            <p className="mt-6 text-[16px] leading-7 text-[#526166]">
              Built in Rust with no Electron, cloud account, or telemetry.
              Queries, counts, and exports run away from the UI thread.
            </p>
            <div className="mt-8 flex flex-wrap gap-x-6 gap-y-3 text-[13px] font-semibold">
              {["OS keychain", "Server paging", "Local history"].map((item) => (
                <span key={item} className="flex items-center gap-2">
                  <span className="flex h-5 w-5 items-center justify-center rounded-full bg-[#D2F36A]">
                    <HugeiconsIcon icon={Tick02Icon} size={12} aria-hidden="true" />
                  </span>
                  {item}
                </span>
              ))}
            </div>
          </div>
        </div>
      </section>

      <section id="download" className="bg-[#FFFCF7] px-3 pb-3 sm:px-5 sm:pb-5">
        <div className="mx-auto max-w-[1440px] overflow-hidden rounded-[28px] border-2 border-[#16232A] bg-[#B8A6FF] px-6 py-16 sm:px-10 sm:py-20 lg:px-16">
          <div className="grid items-end gap-10 lg:grid-cols-[1fr_0.7fr]">
            <div>
              <p className="font-mono text-[10px] tracking-[0.12em]">READY WHEN YOU ARE</p>
              <h2 className="mt-5 text-[46px] leading-[0.98] font-semibold tracking-[-0.055em] sm:text-[66px]">
                Pick your platform.
                <br />
                Keep your data.
              </h2>
              <p className="mt-5 max-w-[620px] text-[16px] leading-7">
                The latest package starts downloading here. No account and no
                detour through a release screen.
              </p>
            </div>
            <QueryTrail className="hidden h-auto w-full max-w-[420px] lg:block" />
          </div>

          <div className="mt-12 grid gap-3 md:grid-cols-3">
            {platforms.map(({ id, name, format, detail, icon: Icon, color }) => (
              <a
                key={id}
                href={`/download/${id}`}
                className="group flex min-h-[190px] flex-col justify-between rounded-[20px] border-2 border-[#16232A] bg-[#F7F1E8] p-6 transition-transform hover:-translate-y-1"
              >
                <div className="flex items-start justify-between">
                  <span
                    className="flex h-11 w-11 items-center justify-center rounded-full border border-[#16232A]"
                    style={{ backgroundColor: color }}
                  >
                    <HugeiconsIcon icon={Icon} size={18} aria-hidden="true" />
                  </span>
                  <HugeiconsIcon
                    icon={Download04Icon}
                    size={16}
                    aria-hidden="true"
                    className="transition-transform group-hover:translate-y-1"
                  />
                </div>
                <div>
                  <h3 className="text-[19px] font-semibold">{name}</h3>
                  <p className="mt-1 text-[13px] text-[#526166]">
                    {format} · {detail}
                  </p>
                </div>
              </a>
            ))}
          </div>

          {download === "unavailable" && (
            <div className="mt-5 flex items-start gap-3 rounded-xl border-2 border-[#16232A] bg-[#F7F1E8] p-4 text-[13px]">
              <HugeiconsIcon
                icon={AlertCircleIcon}
                size={16}
                aria-hidden="true"
                className="mt-0.5 shrink-0"
              />
              The latest package could not be located. Please try again in a moment.
            </div>
          )}

          <div className="mt-7 flex flex-col items-center justify-between gap-5 border-t border-[#16232A]/30 pt-7 text-[12px] sm:flex-row">
            <p>Pre-1.0 software. Start read-only and keep a current backup.</p>
            <a
              href="https://github.com/HakimIno/plusplus/blob/main/docs/RELEASE_SIGNING.md"
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-1.5 font-semibold underline underline-offset-4"
            >
              Verify a release
              <HugeiconsIcon icon={ArrowRight01Icon} size={14} aria-hidden="true" />
            </a>
          </div>
        </div>
      </section>

      <footer className="bg-[#16232A] text-[#F7F1E8]">
        <div className="mx-auto flex max-w-[1240px] flex-col gap-8 px-7 py-12 sm:px-10 md:flex-row md:items-end md:justify-between">
          <div>
            <Brand inverse />
            <p className="mt-3 text-[12px] text-[#AAB4B7]">
              A production-safe native database client.
            </p>
          </div>
          <div className="flex flex-wrap gap-x-6 gap-y-2 text-[12px] font-medium text-[#C9D0D2]">
            <a href={sourceUrl} target="_blank" rel="noreferrer" className="hover:text-white">GitHub</a>
            <a href="https://github.com/HakimIno/plusplus/blob/main/ROADMAP.md" target="_blank" rel="noreferrer" className="hover:text-white">Roadmap</a>
            <a href="https://github.com/HakimIno/plusplus/blob/main/CONTRIBUTING.md" target="_blank" rel="noreferrer" className="hover:text-white">Contribute</a>
            <span>MIT OR Apache-2.0</span>
          </div>
        </div>
      </footer>
    </main>
  );
}
