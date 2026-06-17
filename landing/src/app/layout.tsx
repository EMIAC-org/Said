import type { Metadata } from "next";
import { Inter, Space_Grotesk } from "next/font/google";
import "./globals.css";

const inter = Inter({
  subsets: ["latin"],
  variable: "--font-inter",
  display: "swap",
});

const display = Space_Grotesk({
  subsets: ["latin"],
  variable: "--font-display",
  display: "swap",
});

export const metadata: Metadata = {
  title: "Airnote — Voice to text, everywhere on your Mac",
  description:
    "Airnote turns speech into clean, formatted text in any app. Press the hotkey, talk, paste. On-device when you want it. 90+ languages.",
  metadataBase: new URL("https://airnote.app"),
  openGraph: {
    title: "Airnote — Voice to text, everywhere on your Mac",
    description:
      "Airnote turns speech into clean, formatted text in any app. Press the hotkey, talk, paste.",
    type: "website",
  },
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  // Theme is pinned to "dusk" (the navy → indigo → warm purple hero sky).
  // The user-facing theme toggle was removed — there is no longer a need to
  // read localStorage or write data-theme at runtime.
  return (
    <html
      lang="en"
      data-theme="dusk"
      className={`${inter.variable} ${display.variable}`}
    >
      <body className="noise antialiased">{children}</body>
    </html>
  );
}
