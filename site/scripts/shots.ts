import { mkdirSync } from "node:fs";
import { join } from "node:path";
import sharp from "sharp";
import {
  APPEARANCES,
  FEATURE_DISPLAY_WIDTH,
  HERO_DISPLAY_WIDTH,
  HERO_SCREEN,
  PIXEL_RATIO,
  shotName,
  shotPath,
  STEPS,
} from "../src/content";

const WEBP_QUALITY = 82;
const WEBP_EFFORT = 6;

const site = join(import.meta.dir, "..");
const rendered = join(site, "..", "target", "shots");
const published = join(site, "public");

const screens = [
  { screen: HERO_SCREEN, width: HERO_DISPLAY_WIDTH * PIXEL_RATIO },
  ...STEPS.map((step) => ({
    screen: step.screen,
    width: FEATURE_DISPLAY_WIDTH * PIXEL_RATIO,
  })),
];

mkdirSync(join(published, "shots"), { recursive: true });

for (const { screen, width } of screens) {
  for (const appearance of APPEARANCES) {
    const group = screen.group === undefined ? [] : ["--group", String(screen.group)];
    const args = ["mise", "run", "shot", "--", screen.name, appearance, ...group];
    const render = Bun.spawnSync(args, {
      stdout: "inherit",
      stderr: "inherit",
    });
    if (!render.success) {
      throw new Error(`${args.join(" ")} failed`);
    }
    const source = join(rendered, `${shotName(screen)}-${appearance}.png`);
    const target = join(published, shotPath(screen, appearance));
    await sharp(source).resize({ width }).webp({ quality: WEBP_QUALITY, effort: WEBP_EFFORT }).toFile(target);
    console.log(target);
  }
}
