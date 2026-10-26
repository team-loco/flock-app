import { join } from "node:path";
import sharp from "sharp";
import { FOOTER_ICON_ASSET_SIZE, ICON_ASSET_SIZE, iconPath } from "../src/content";

const FAVICON_SIZE = 32;
const WEBP_QUALITY = 90;

const site = join(import.meta.dir, "..");
const source = join(site, "..", "assets", "flock-logo.png");
const published = join(site, "public");

const icons = [
  { size: ICON_ASSET_SIZE, format: "webp" },
  { size: ICON_ASSET_SIZE, format: "png" },
  { size: FOOTER_ICON_ASSET_SIZE, format: "png" },
] as const;

for (const { size, format } of icons) {
  const image = sharp(source).resize(size);
  const encoded = format === "webp" ? image.webp({ quality: WEBP_QUALITY }) : image.png();
  await encoded.toFile(join(published, iconPath(size, format)));
}
await sharp(source)
  .resize(FAVICON_SIZE)
  .png()
  .toFile(join(published, `favicon-${FAVICON_SIZE}.png`));
