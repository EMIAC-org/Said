// Brand icon registry backed by simple-icons (CC0-licensed brand SVG paths,
// intended for "works with X" / integration showcases). We import per slug so
// the bundle only carries the icons we actually use.
//
// simple-icons API: each entry is { title, slug, hex, path }.

import {
  siSlack,
  siNotion,
  siZedindustries,
  siXcode,
  siGooglechrome,
  siSafari,
  siLinear,
  siFigma,
  siDiscord,
  siObsidian,
  siRaycast,
  siTelegram,
  siWhatsapp,
  siGmail,
  siGithub,
  siVercel,
  siLoom,
} from "simple-icons";

export type Brand = {
  title: string;
  slug: string;
  hex: string;
  path: string;
};

// Note: Cursor and Microsoft VS Code aren't in simple-icons (both brands have
// requested exclusion). Substituting Zed and Xcode — both are Mac-native code
// editors, fit the same slot on the keyboard, and are licensed for nominative
// use through simple-icons (CC0).
const REGISTRY: Record<string, Brand> = {
  slack: siSlack,
  notion: siNotion,
  zed: siZedindustries,
  xcode: siXcode,
  googlechrome: siGooglechrome,
  safari: siSafari,
  linear: siLinear,
  figma: siFigma,
  discord: siDiscord,
  obsidian: siObsidian,
  raycast: siRaycast,
  telegram: siTelegram,
  whatsapp: siWhatsapp,
  gmail: siGmail,
  github: siGithub,
  vercel: siVercel,
  loom: siLoom,
};

export function getBrandIcon(slug: string): Brand | undefined {
  return REGISTRY[slug];
}

/** YIQ luminance test — true if the color is dark enough that we should invert
 *  it to white on a colored background instead of brand-color on white. */
export function isHexDark(hex: string): boolean {
  const r = parseInt(hex.slice(0, 2), 16);
  const g = parseInt(hex.slice(2, 4), 16);
  const b = parseInt(hex.slice(4, 6), 16);
  return (r * 299 + g * 587 + b * 114) / 1000 < 128;
}
