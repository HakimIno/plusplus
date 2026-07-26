import { NextResponse } from "next/server";

type ReleaseAsset = {
  name: string;
  browser_download_url: string;
};

type GitHubRelease = {
  assets: ReleaseAsset[];
};

const assetMatchers: Record<string, (name: string) => boolean> = {
  macos: (name) => name.toLowerCase().endsWith(".dmg"),
  windows: (name) => name.toLowerCase().endsWith(".zip"),
  linux: (name) => name.toLowerCase().endsWith(".appimage"),
};

export async function GET(
  request: Request,
  { params }: { params: Promise<{ platform: string }> },
) {
  const { platform } = await params;
  const matchesPlatform = assetMatchers[platform];

  if (!matchesPlatform) {
    return NextResponse.json({ error: "Unsupported platform" }, { status: 404 });
  }

  try {
    const response = await fetch(
      "https://api.github.com/repos/HakimIno/plusplus/releases/latest",
      {
        headers: {
          Accept: "application/vnd.github+json",
          "User-Agent": "plusplus-website",
        },
        next: { revalidate: 300 },
      },
    );

    if (!response.ok) {
      throw new Error(`GitHub API returned ${response.status}`);
    }

    const release = (await response.json()) as GitHubRelease;
    const asset = release.assets.find(({ name }) => matchesPlatform(name));

    if (!asset) {
      throw new Error(`No release asset found for ${platform}`);
    }

    return NextResponse.redirect(asset.browser_download_url, 307);
  } catch {
    return NextResponse.redirect(
      new URL("/?download=unavailable#download", request.url),
      307,
    );
  }
}
