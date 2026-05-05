import type { Metadata } from 'next';
import { Inter_Tight } from 'next/font/google';
import './globals.css';

const interTight = Inter_Tight({
  subsets: ['latin'],
  weight: ['400', '500', '600'],
  variable: '--font-sans',
  display: 'swap',
});

export const metadata: Metadata = {
  title: 'Said — Speak. Don’t type.',
  description:
    'Said turns your voice into clear text in real time, for everything from AI prompts to essays.',
  metadataBase: new URL('https://said.app'),
  openGraph: {
    title: 'Said — Speak. Don’t type.',
    description:
      'Said turns your voice into clear text in real time, for everything from AI prompts to essays.',
    type: 'website',
  },
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className={interTight.variable}>
      <body className="font-sans antialiased">{children}</body>
    </html>
  );
}
