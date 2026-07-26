import type { Metadata } from "next";
import "./globals.css";

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

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
