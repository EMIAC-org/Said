import { useEffect, useState, type CSSProperties } from "react";
import { Monitor } from "lucide-react";
import { getAppIdentity } from "@/lib/invoke";
import type { AppIdentity } from "@/types";

const identityCache = new Map<string, AppIdentity | null>();
const identityInflight = new Map<string, Promise<AppIdentity | null>>();

const KNOWN_APP_NAMES: Record<string, string> = {
  "com.apple.Safari": "Safari",
  "com.apple.Notes": "Notes",
  "com.apple.mail": "Mail",
  "com.apple.Messages": "Messages",
  "com.apple.TextEdit": "TextEdit",
  "com.google.Chrome": "Chrome",
  "org.chromium.Chromium": "Chromium",
  "com.microsoft.VSCode": "VS Code",
  "com.microsoft.Word": "Word",
  "com.microsoft.Outlook": "Outlook",
  "com.microsoft.PowerPoint": "PowerPoint",
  "com.microsoft.Excel": "Excel",
  "com.slack.Slack": "Slack",
  "com.tinyspeck.slackmacgap": "Slack",
  "com.notion.id": "Notion",
  "com.figma.Desktop": "Figma",
  "company.thebrowser.Browser": "Arc",
  "md.obsidian": "Obsidian",
  "com.apple.dt.Xcode": "Xcode",
  "com.googlecode.iterm2": "iTerm2",
  "com.apple.Terminal": "Terminal",
  "com.spotify.client": "Spotify",
  "com.linear.app": "Linear",
  "com.discord.Discord": "Discord",
  "com.zoom.us": "Zoom",
};

function resolveIdentity(appKey: string): Promise<AppIdentity | null> {
  if (identityCache.has(appKey)) return Promise.resolve(identityCache.get(appKey) ?? null);
  const existing = identityInflight.get(appKey);
  if (existing) return existing;

  const request = getAppIdentity(appKey).then((identity) => {
    identityCache.set(appKey, identity);
    identityInflight.delete(appKey);
    return identity;
  });
  identityInflight.set(appKey, request);
  return request;
}

export function fallbackAppName(appKey: string | null | undefined): string {
  const key = appKey?.trim();
  if (!key) return "Unknown app";
  if (KNOWN_APP_NAMES[key]) return KNOWN_APP_NAMES[key];
  const pathName = key.split(/[\\/]/).filter(Boolean).pop();
  const base = pathName || key.split(".").filter(Boolean).pop() || key;
  return base.replace(/\.(app|exe)$/i, "").replace(/[-_]/g, " ");
}

export function appDisplayName(appKey: string | null | undefined, identity?: AppIdentity | null): string {
  return identity?.name?.trim() || fallbackAppName(appKey);
}

export function useAppIdentity(appKey: string | null | undefined): AppIdentity | null {
  const key = appKey?.trim() || "";
  const [identity, setIdentity] = useState<AppIdentity | null>(() =>
    key ? identityCache.get(key) ?? null : null,
  );

  useEffect(() => {
    if (!key) {
      setIdentity(null);
      return;
    }
    let alive = true;
    void resolveIdentity(key).then((next) => {
      if (alive) setIdentity(next);
    });
    return () => {
      alive = false;
    };
  }, [key]);

  return identity;
}

export function AppIcon({
  appKey,
  label,
  size = 24,
  radius = 6,
  fallbackSize = 13,
  className,
  style,
}: {
  appKey: string | null | undefined;
  label?: string;
  size?: number;
  radius?: number;
  fallbackSize?: number;
  className?: string;
  style?: CSSProperties;
}) {
  const identity = useAppIdentity(appKey);
  const name = label || appDisplayName(appKey, identity);

  return (
    <span
      className={className}
      title={name}
      style={{
        width: size,
        height: size,
        borderRadius: radius,
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        overflow: "hidden",
        flexShrink: 0,
        background: "hsl(var(--surface-4))",
        color: "hsl(var(--muted-foreground))",
        boxShadow: "inset 0 0 0 1px hsl(var(--border) / 0.55)",
        ...style,
      }}
    >
      {identity?.icon ? (
        <img
          src={identity.icon}
          alt={name}
          draggable={false}
          style={{ width: "100%", height: "100%", objectFit: "contain" }}
        />
      ) : (
        <Monitor size={fallbackSize} />
      )}
    </span>
  );
}
