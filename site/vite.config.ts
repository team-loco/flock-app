import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";
import { TAGLINE, TITLE } from "./src/content";

const REPO = "team-loco/flock-app";
const REPO_URL = `https://github.com/${REPO}`;
const RELEASES_URL = `${REPO_URL}/releases`;
const LATEST_RELEASE_URL = `${RELEASES_URL}/latest`;
const LATEST_RELEASE_API = `https://api.github.com/repos/${REPO}/releases/latest`;
const ASSET_PREFIX = "Flock-";
const ASSET_SUFFIX = "-arm64.zip";
const LOCO_MARKER = "-loco-";

type Release = { assets?: { name?: string; browser_download_url?: string }[] };

async function downloadUrl(): Promise<string> {
  const token = process.env.GITHUB_TOKEN;
  const headers: Record<string, string> = { Accept: "application/vnd.github+json" };
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }
  try {
    const response = await fetch(LATEST_RELEASE_API, { headers });
    if (!response.ok) {
      console.warn(`latest release lookup failed: ${response.status}`);
      return LATEST_RELEASE_URL;
    }
    const release = (await response.json()) as Release;
    const asset = release.assets?.find(
      ({ name }) =>
        name !== undefined &&
        name.startsWith(ASSET_PREFIX) &&
        name.endsWith(ASSET_SUFFIX) &&
        !name.includes(LOCO_MARKER),
    );
    return asset?.browser_download_url ?? LATEST_RELEASE_URL;
  } catch (error) {
    console.warn(`latest release lookup failed: ${error}`);
    return LATEST_RELEASE_URL;
  }
}

const HTML_VALUES: Record<string, string> = {
  "%FLOCK_TITLE%": TITLE,
  "%FLOCK_TAGLINE%": TAGLINE,
};

const htmlValues = (): Plugin => ({
  name: "flock-html-values",
  transformIndexHtml: (html) =>
    Object.entries(HTML_VALUES).reduce(
      (page, [token, value]) => page.replaceAll(token, value),
      html,
    ),
});

export default defineConfig(async () => ({
  plugins: [react(), tailwindcss(), htmlValues()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  define: {
    __DOWNLOAD_URL__: JSON.stringify(await downloadUrl()),
    __RELEASES_URL__: JSON.stringify(RELEASES_URL),
    __REPO_URL__: JSON.stringify(REPO_URL),
  },
}));
