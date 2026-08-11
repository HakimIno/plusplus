import type { Metadata, Viewport } from "next";
import { Inter, Sora } from "next/font/google";
import "./globals.css";

const display = Sora({
  subsets: ["latin"],
  variable: "--font-display",
  display: "swap",
});

const body = Inter({
  subsets: ["latin"],
  variable: "--font-body",
  display: "swap",
});

export const metadata: Metadata = {
  metadataBase: new URL("https://plusplus.dev"),
  title: "plusplus — A production-safe native database client",
  description:
    "Browse schemas, run SQL, edit data, and export complete datasets with a fast native database client built for safer production work.",
  openGraph: {
    title: "plusplus — Query fast. Change carefully.",
    description:
      "A fast, native SQL client that makes production mistakes harder.",
    type: "website",
    images: ["/screenshots/erd.png"],
  },
};

export const viewport: Viewport = {
  themeColor: "#171717",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="en"
      className={`dark ${display.variable} ${body.variable}`}
      data-theme="dark"
      suppressHydrationWarning
    >
      <body className="bg-background text-foreground antialiased">
        {children}
      </body>
    </html>
  );
}
