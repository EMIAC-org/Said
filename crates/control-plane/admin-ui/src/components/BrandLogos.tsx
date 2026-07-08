interface LogoProps {
  size?: number
  className?: string
}

export function LarkLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 48 48" fill="none" className={className}>
      <path d="M8.87 6.47c.3-.5.97-.67 1.47-.37l15.34 9.33c.3.18.49.5.49.86v14.42c0 .57-.62.93-1.12.65L8.87 22.2a.85.85 0 0 1-.44-.74V7.34c0-.3.15-.6.44-.87z" fill="#3370FF" />
      <path d="M39.13 6.47c-.3-.5-.97-.67-1.47-.37l-15.34 9.33a.98.98 0 0 0-.49.86v14.42c0 .57.62.93 1.12.65l16.18-9.16a.85.85 0 0 0 .44-.74V7.34a1 1 0 0 0-.44-.87z" fill="#00D6B9" />
      <path d="M24 32.95l-8.5 5.15c-.5.3-.5 1.03 0 1.34l8.06 4.86c.28.17.63.17.9 0l8.05-4.86c.5-.3.5-1.03 0-1.34L24 32.95z" fill="#3370FF" />
    </svg>
  )
}

export function OpenAILogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" className={className}>
      <path d="M22.28 9.37a6.05 6.05 0 0 0-.52-4.97 6.12 6.12 0 0 0-6.58-2.94A6.06 6.06 0 0 0 10.62.08a6.12 6.12 0 0 0-5.83 4.24 6.05 6.05 0 0 0-4.05 2.93 6.12 6.12 0 0 0 .75 7.18 6.05 6.05 0 0 0 .52 4.97 6.12 6.12 0 0 0 6.58 2.94 6.06 6.06 0 0 0 4.56 1.38 6.12 6.12 0 0 0 5.83-4.24 6.05 6.05 0 0 0 4.05-2.93 6.12 6.12 0 0 0-.75-7.18zM13.38 22.4a4.57 4.57 0 0 1-2.94-1.07l.15-.08 4.88-2.82a.8.8 0 0 0 .4-.69v-6.9l2.06 1.19a.07.07 0 0 1 .04.06v5.7a4.59 4.59 0 0 1-4.59 4.6zM3.76 18.3a4.56 4.56 0 0 1-.55-3.07l.15.09 4.88 2.82a.78.78 0 0 0 .78 0l5.96-3.44v2.38a.07.07 0 0 1-.03.06l-4.94 2.85a4.59 4.59 0 0 1-6.25-1.69zM2.54 7.97a4.56 4.56 0 0 1 2.38-2.01v5.8a.79.79 0 0 0 .4.69l5.96 3.44-2.07 1.19a.07.07 0 0 1-.07 0L4.2 14.23a4.59 4.59 0 0 1-1.67-6.26zm16.37 3.81L12.95 8.34l2.06-1.19a.07.07 0 0 1 .07 0l4.94 2.85a4.59 4.59 0 0 1-.71 8.28v-5.81a.79.79 0 0 0-.4-.69zm2.05-3.1l-.15-.1-4.88-2.81a.78.78 0 0 0-.78 0L9.19 9.22V6.84a.07.07 0 0 1 .03-.06l4.94-2.85a4.59 4.59 0 0 1 6.8 4.75zM8.1 12.9l-2.06-1.2a.07.07 0 0 1-.04-.05V5.95a4.59 4.59 0 0 1 7.53-3.53l-.15.08-4.88 2.82a.8.8 0 0 0-.4.7v6.88zm1.12-2.42l2.65-1.53 2.66 1.53v3.07l-2.66 1.53-2.65-1.53V10.48z" />
    </svg>
  )
}

export function GroqLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" className={className}>
      <rect x="2" y="2" width="20" height="20" rx="4" fill="#F55036" />
      <path d="M12 6a6 6 0 1 0 0 12 6 6 0 0 0 0-12zm0 2a4 4 0 1 1 0 8 4 4 0 0 1 0-8z" fill="#FFF" />
      <rect x="14" y="11" width="4" height="2" rx="1" fill="#FFF" />
    </svg>
  )
}

export function SlackLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" className={className}>
      <path d="M5.04 15.24a2.03 2.03 0 1 1-2.03-2.03h2.03v2.03zm1.02 0a2.03 2.03 0 1 1 4.05 0v5.08a2.03 2.03 0 1 1-4.05 0v-5.08z" fill="#E01E5A" />
      <path d="M8.73 5.04a2.03 2.03 0 1 1 2.03-2.03v2.03H8.73zm0 1.02a2.03 2.03 0 1 1 0 4.05H3.68a2.03 2.03 0 0 1 0-4.05h5.05z" fill="#36C5F0" />
      <path d="M18.96 8.73a2.03 2.03 0 1 1 2.03 2.03h-2.03V8.73zm-1.02 0a2.03 2.03 0 1 1-4.05 0V3.68a2.03 2.03 0 0 1 4.05 0v5.05z" fill="#2EB67D" />
      <path d="M15.27 18.96a2.03 2.03 0 1 1-2.03 2.03v-2.03h2.03zm0-1.02a2.03 2.03 0 0 1 0-4.05h5.05a2.03 2.03 0 0 1 0 4.05h-5.05z" fill="#ECB22E" />
    </svg>
  )
}

export function ZoomLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" className={className}>
      <rect x="2" y="2" width="20" height="20" rx="5" fill="#2D8CFF" />
      <path d="M5.5 9.5a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-5a2 2 0 0 1-2-2v-5z" fill="#FFF" />
      <path d="M15.5 10.2l2.5-1.5a.5.5 0 0 1 .75.43v5.74a.5.5 0 0 1-.75.43l-2.5-1.5V10.2z" fill="#FFF" />
    </svg>
  )
}

export function GoogleMeetLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" className={className}>
      <path d="M12.5 5H6.5A2.5 2.5 0 0 0 4 7.5v9A2.5 2.5 0 0 0 6.5 19h6A2.5 2.5 0 0 0 15 16.5V7.5A2.5 2.5 0 0 0 12.5 5z" fill="#00897B" />
      <path d="M15 10l4.3-3a.5.5 0 0 1 .77.42v9.16a.5.5 0 0 1-.77.42L15 14V10z" fill="#00897B" />
      <path d="M6.5 5A2.5 2.5 0 0 0 4 7.5V12h5.5V5H6.5z" fill="#00BFA5" />
      <path d="M9.5 12H4v4.5A2.5 2.5 0 0 0 6.5 19H15v-7H9.5z" fill="#00897B" />
    </svg>
  )
}

export function TeamsLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" className={className}>
      <path d="M16.5 5.5a2 2 0 1 0 0-4 2 2 0 0 0 0 4z" fill="#5059C9" />
      <path d="M20 7h-7a1 1 0 0 0-1 1v6.5a3.5 3.5 0 0 0 3.5 3.5H17a3 3 0 0 0 3-3V7z" fill="#5059C9" />
      <path d="M10.5 5.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z" fill="#7B83EB" />
      <path d="M14 7H4a1 1 0 0 0-1 1v7a4 4 0 0 0 4 4h3a4 4 0 0 0 4-4V7z" fill="#7B83EB" />
    </svg>
  )
}

export function NotionLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" className={className}>
      <path d="M4.46 2.46c.52.43.72.4 1.7.33l9.2-.55c.98-.06 1.23-.03 1.23.03s.03.24-.24.24l-1.53.18c-.36.06-.47.18-.47.18v12.6c0 .67.37.98.61.98.36 0 .55-.24.55-.24l.18-.12V4.62c0-.24.18-.24.18-.24h3.37c.24 0 .3.18.3.3v11.94c0 .73-.55 1.28-1.53 1.34l-2.69.18c-1.1.06-1.35-.3-1.35-.67V6.34L10.6 6.7v11.57c0 .61-.49.85-.49.85l-2.26.3c-.98.13-1.1-.3-1.1-.3L4.28 14.5c-.24-.43-.24-.85 0-.85l.73-.12V4.38l-.73.06c-.36.03-.55-.24-.55-.55 0-.3.18-.55.73-1.43z" fillRule="evenodd" />
    </svg>
  )
}

export function LinearLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" className={className}>
      <path d="M3.36 6.88a10 10 0 0 0-.28 1.39L13.73 18.92a10.07 10.07 0 0 0 1.39-.28L3.36 6.88zm-1.02 3.64A9.99 9.99 0 0 0 2 12c0 2.83 1.18 5.39 3.07 7.21l8.14-8.14L2.34 10.52zM4.79 19.21A9.96 9.96 0 0 0 12 22a9.99 9.99 0 0 0 1.48-.11l-8.69-8.69v.01zM3.86 5.58L18.42 20.14a10.01 10.01 0 0 0 1.82-1.58L4.08 2.4a9.96 9.96 0 0 0-1.58 1.82L3.86 5.58zM5.79 3.44L20.56 18.21A9.98 9.98 0 0 0 22 12c0-5.52-4.48-10-10-10a9.96 9.96 0 0 0-6.21 2.16v-.01z" fill="#5E6AD2" />
    </svg>
  )
}

export function PostgresLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" className={className}>
      <path d="M17.13 2.23c-1.42-.3-2.48-.2-3.33.08-.65-.35-1.65-.58-2.8-.53-1.5.06-2.8.57-3.62 1.3-.6-.1-1.32-.1-2.1.09C3.28 3.6 2.18 5.1 2.03 7.2c-.08 1.05.06 2.4.41 4.1.65 3.1 2.08 6.84 3.92 7.67.73.33 1.42.16 1.92-.27.06-.05.12-.1.17-.17a5.1 5.1 0 0 0 2.51.66c.62 0 1.19-.11 1.69-.29.35.53.72.87 1.13 1.01.25.08.5.12.76.12.5 0 1.02-.2 1.53-.57.87-.64 1.43-1.7 1.43-1.7s.35.06.8.02c1.56-.13 2.62-.84 3.26-1.78 1.33-1.97.85-4.66.55-5.96-.14-.62-.55-1.56-1.18-2.52.14-.72.23-1.62.14-2.58-.14-1.61-.87-3.19-2.54-3.71z" fill="#336791" />
      <path d="M16.54 5.67c.5.24.85.78.93 1.73.07.81-.02 1.55-.14 2.17-.29-.4-.63-.8-1.02-1.2-.95-.96-2.03-1.5-2.03-1.5s1.26-.7 2.26-.82v-.38z" fill="#FFF" opacity="0.4" />
    </svg>
  )
}

export function AirNoteLogo({ size = 20, className }: LogoProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" className={className}>
      <rect x="3" y="8.5" width="3" height="7" rx="1.5" fill="currentColor" />
      <rect x="8" y="4.5" width="3" height="15" rx="1.5" fill="currentColor" />
      <rect x="13" y="2.5" width="3" height="19" rx="1.5" fill="currentColor" />
      <rect x="18" y="6.5" width="3" height="11" rx="1.5" fill="currentColor" />
    </svg>
  )
}
