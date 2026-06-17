// All copy for the Airnote landing page lives in this file.
// Edit here to avoid touching component code.

export const downloads = {
  mac: {
    latestVersion: "2.3.8",
    latestDmg:
      "https://airnote.emiactech.com/releases/2.3.8/AirNote_2.3.8_aarch64.dmg",
  },
  windows: {
    latestVersion: "2.3.6",
    latestSetup:
      "https://airnote.emiactech.com/releases/2.3.6/AirNote_2.3.6_x64-setup.exe",
  },
};

export const nav = {
  links: [
    { label: "Home", href: "#top" },
    { label: "Pricing", href: "#pricing" },
    { label: "Changelog", href: "/changelog" },
  ],
  cta: { label: "Download", href: downloads.mac.latestDmg },
};

export const hero = {
  title: "Talk. Paste. Done.",
  subtitle:
    "Turn your voice into polished text. Works in any app on your Mac.",
  ctaMac: { label: "Download for Mac", href: downloads.mac.latestDmg },
  ctaWindows: { label: "Download for Windows", href: downloads.windows.latestSetup },
  iphoneLink: { label: "Also available for iPhone", href: "#mobile" },
};

export const shortcutDemo = {
  title: "One hotkey. Every app.",
  body:
    "Hold ⌥ Space, talk, release. Airnote drops cleaned text wherever your cursor is — Slack, Notion, your IDE, an email draft, a terminal prompt.",
  apps: ["Slack", "Notion", "Mail", "VS Code", "Chrome", "Messages", "Linear", "Figma"],
};

export const logoStrip = {
  eyebrow: "Trusted in production",
  title: "Used by those who move fast.",
  subtitle:
    "From two-person startups to engineering teams that ship daily — Airnote keeps up.",
  // Brand glyphs rendered via simple-icons (CC0) for nominative use. Swap to
  // real partner / customer marks when available.
  logos: [
    { slug: "vercel", name: "Vercel" },
    { slug: "linear", name: "Linear" },
    { slug: "loom", name: "Loom" },
    { slug: "notion", name: "Notion" },
    { slug: "figma", name: "Figma" },
    { slug: "raycast", name: "Raycast" },
  ],
};

export const toneSwitcher = {
  eyebrow: "Adaptability",
  title: "Communication isn't one-size-fits-all.",
  // 5 tone modes. Each carries its own colour palette so the WHOLE right
  // pane (outgoing bubble + active tab + glow) re-themes per tone. The
  // `messages` array drives a sequential typewriter — each message is
  // typed in, holds briefly, then the next one starts. Once all played,
  // the panel transitions to the next tone with a fresh palette.
  //
  // side === "out" → the user dictating via Airnote (right, coloured)
  // side === "in"  → Aarav replying (left, dark glass)
  modes: [
    {
      id: "formal",
      label: "Formal",
      color: "#1e8bff",          // blue — professional
      colorRgb: "30, 139, 255",
      messages: [
        { side: "out" as const, text: "Hello Aarav, hope you're doing well. Could you share the latest project status?" },
        { side: "in"  as const, text: "Hi! Yes — all milestones are tracking to plan. Final review is on Thursday." },
        { side: "out" as const, text: "Excellent. Would Tuesday at 10 AM work for a brief sync?" },
        { side: "in"  as const, text: "That works. I'll send a calendar invite shortly." },
      ],
    },
    {
      id: "casual",
      label: "Casual",
      color: "#06b6d4",          // cyan — friendly, modern
      colorRgb: "6, 182, 212",
      messages: [
        { side: "out" as const, text: "Hey Aarav, all good your end?" },
        { side: "in"  as const, text: "Yeah pretty good! How about you?" },
        { side: "out" as const, text: "Same here. Wanna grab a call next week?" },
        { side: "in"  as const, text: "Sure! Tuesday or Wednesday works 👍" },
      ],
    },
    {
      id: "legal",
      label: "Legal",
      color: "#7c3aed",          // violet — official, weighty
      colorRgb: "124, 58, 237",
      messages: [
        { side: "out" as const, text: "Aarav, please confirm the current status of all open deliverables." },
        { side: "in"  as const, text: "All items remain on schedule per the project plan dated 12 May." },
        { side: "out" as const, text: "Kindly forward the signed addendum at your earliest convenience." },
        { side: "in"  as const, text: "Will be transmitted within 24 hours." },
      ],
    },
    {
      id: "chat",
      label: "Chat",
      color: "#10b981",          // emerald — light, informal
      colorRgb: "16, 185, 129",
      messages: [
        { side: "out" as const, text: "yo aarav updates?" },
        { side: "in"  as const, text: "ya all good 👌" },
        { side: "out" as const, text: "call next wk?" },
        { side: "in"  as const, text: "wed afternoon work?" },
      ],
    },
    {
      id: "hinglish",
      label: "Hinglish",
      color: "#f59e0b",          // amber — warm, distinctive
      colorRgb: "245, 158, 11",
      messages: [
        { side: "out" as const, text: "Aarav, koi update hai bhai?" },
        { side: "in"  as const, text: "Haan yaar, sab on track 💪" },
        { side: "out" as const, text: "Next week ek call set kar lete hain?" },
        { side: "in"  as const, text: "Bilkul, Wednesday evening free hai mera" },
      ],
    },
  ],
};

export const features = {
  eyebrow: "What's inside",
  title: "Powerful features, seamlessly integrated.",
  subtitle:
    "Airnote sits in the background. It only shows up when you press the key — and gets out of the way the moment you're done.",
  // Large showcase cards — each renders a distinctive CSS mock UI (or
  // photo-backed visual) at the top + an icon-beside-title row + a
  // 2-beat body (description + bolded white tagline).
  showcase: [
    {
      id: "offline",
      accent: "violet",
      icon: "WifiOff",
      visual: "WifiToggleVisual",
      title: "Works offline.",
      body: "Airnote keeps working without a connection.",
      tagline: "No Wi-Fi, no problem.",
    },
    {
      id: "vocab",
      accent: "emerald",
      icon: "BookOpen",
      visual: "VocabPanelVisual",
      title: "Use your own words.",
      body: "Enter names, abbreviations, and specialized terms once.",
      tagline: "Airnote remembers them forever.",
    },
    {
      id: "tones",
      accent: "rose",
      icon: "Sliders",
      visual: "ModePickerVisual",
      title: "Predefined modes.",
      body: "Each mode tunes tone, structure, and formatting.",
      tagline: "So your text is always right, right away.",
    },
    {
      id: "languages",
      accent: "sky",
      icon: "Globe",
      visual: "MultilingualVisual",
      title: "Multilingual support.",
      body: "Airnote handles 92+ languages and dialects.",
      tagline: "Speak Hinglish or switch mid-sentence — Airnote keeps up.",
    },
    {
      id: "clipboard",
      accent: "green",
      icon: "Clipboard",
      visual: "ClipboardChatVisual",
      title: "Clipboard integration.",
      body: "No copy-paste step in the middle of your flow.",
      tagline: "Cleaned text lands where your cursor is.",
    },
    {
      id: "meeting",
      accent: "cyan",
      icon: "Users",
      visual: "MeetingCallVisual",
      title: "Meeting assistant.",
      body: "Focus on the conversation while Airnote takes notes.",
      tagline: "Record and digest meetings effortlessly.",
    },
  ],
  // Small callout row beneath — icon + title + 2-line body + optional badge.
  callouts: [
    {
      icon: "FileText",
      title: "File transcription",
      body: "Upload audio or video. Get a polished transcript back, ready to share.",
    },
    {
      icon: "Hand",
      title: "Push to talk",
      body: "Hold a key from anywhere. Speak, release, and the cleaned text is waiting.",
      badge: "New",
    },
    {
      icon: "Command",
      title: "Shortcuts",
      body: "Bind any combo to launch, dictate, or jump between modes — global or per-app.",
    },
    {
      icon: "Sparkles",
      title: "Super Mode",
      body: "Long-form, multi-speaker, code-aware. Engage when accuracy matters most.",
    },
  ],
};

export const insights = {
  eyebrow: "Insights",
  title: "See your dictation, in numbers.",
  subtitle:
    "Track your speed, accuracy, and consistency over time — without ever leaving the app.",
  innerHeader: { title: "Insights" },
  // a11y label for the range tabs.
  rangeLabel: "Time range",
  // The range the dashboard opens on. "all" matches the original static
  // snapshot so first-paint stays identical to today; the interactivity
  // is a reward for clicking, not a different default state.
  defaultRange: "all" as const,
  // Heatmap labels + legend stay constant across ranges — only the
  // `filled` cell set changes per range.
  streakBase: {
    daysOfWeek: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    months: ["Mar", "Apr", "May"],
    // The bottom-right cell of the heatmap (day 6 / Sat, col 11)
    // represents this date — used to compute hover-tooltip dates.
    endDate: "2026-05-30",
    legend: { more: "More", less: "Less", today: "Today" },
    emptyLabel: "No dictation",
  },
  // Per-range snapshots. Clicking a range swaps these into the live view.
  // Hand-picked to tell a coherent story over time: Today is small,
  // 7 days is the fastest recent week, All time aggregates everything.
  ranges: [
    {
      id: "today" as const,
      label: "Today",
      subhead: "2 sessions",
      kpis: {
        wpm: { value: "142", gauge: 36, gaugeLabel: "Today", gaugeValue: "36%" },
        sessions: {
          value: "2",
          label: "Sessions polished",
          lines: [
            { text: "85 words total" },
            { text: "1 day streak" },
          ],
        },
        total: {
          value: "85",
          label: "Total words dictated",
          sub: "Desktop · macOS",
          lines: [
            { text: "85 polished words across 2 sessions" },
          ],
        },
      },
      accuracy: {
        title: "Accuracy",
        sessions: "2 sessions",
        percent: 100,
        centerLabel: "accepted",
        rows: [
          { label: "Accepted as-is", value: "2" },
          { label: "Edited after paste", value: "0" },
        ],
        last: { label: "Today", value: "100%" },
      },
      streak: {
        title: "1 day streak",
        best: "BEST · 1D",
        filled: [{ day: 6, col: 11, today: true, words: 42 }],
      },
    },
    {
      id: "7d" as const,
      label: "7 days",
      subhead: "8 sessions",
      kpis: {
        wpm: { value: "156", gauge: 42, gaugeLabel: "7-day top", gaugeValue: "42%" },
        sessions: {
          value: "8",
          label: "Sessions polished",
          lines: [
            { text: "412 words total" },
            { text: "2 day streak" },
          ],
        },
        total: {
          value: "412",
          label: "Total words dictated",
          sub: "Desktop · macOS",
          lines: [
            { text: "412 polished words across 8 sessions" },
          ],
        },
      },
      accuracy: {
        title: "Accuracy",
        sessions: "8 sessions",
        percent: 100,
        centerLabel: "accepted",
        rows: [
          { label: "Accepted as-is", value: "8" },
          { label: "Edited after paste", value: "0" },
        ],
        last: { label: "Last 7 days", value: "100%" },
      },
      streak: {
        title: "2 day streak",
        best: "BEST · 2D",
        filled: [
          { day: 6, col: 11, today: true, words: 42 },
          { day: 6, col: 10, words: 78 },
          { day: 5, col: 10, words: 36 },
          { day: 3, col: 10, words: 51 },
          { day: 1, col: 10, words: 22 },
        ],
      },
    },
    {
      id: "30d" as const,
      label: "30 days",
      subhead: "21 sessions",
      kpis: {
        wpm: { value: "145", gauge: 38, gaugeLabel: "30-day top", gaugeValue: "38%" },
        sessions: {
          value: "21",
          label: "Sessions polished",
          lines: [
            { text: "1,037 words total" },
            { text: "2 day streak" },
          ],
        },
        total: {
          value: "1,037",
          label: "Total words dictated",
          sub: "Desktop · macOS",
          lines: [
            { text: "1,037 polished words across 21 sessions" },
          ],
        },
      },
      accuracy: {
        title: "Accuracy",
        sessions: "21 sessions",
        percent: 99,
        centerLabel: "accepted",
        rows: [
          { label: "Accepted as-is", value: "20" },
          { label: "Edited after paste", value: "1" },
        ],
        last: { label: "Last 30 days", value: "99%" },
      },
      streak: {
        title: "2 day streak",
        best: "BEST · 2D",
        filled: [
          { day: 6, col: 11, today: true, words: 42 },
          { day: 6, col: 10, words: 78 },
          { day: 5, col: 10, words: 36 },
          { day: 3, col: 10, words: 51 },
          { day: 1, col: 10, words: 22 },
          { day: 4, col: 8, words: 67 },
          { day: 2, col: 8, words: 41 },
          { day: 5, col: 7, words: 95 },
          { day: 2, col: 7, words: 31 },
        ],
      },
    },
    {
      id: "all" as const,
      label: "All time",
      subhead: "21 sessions",
      kpis: {
        wpm: { value: "140", gauge: 35, gaugeLabel: "Top", gaugeValue: "35%" },
        sessions: {
          value: "21",
          label: "Sessions polished",
          lines: [
            { text: "1,037 words total" },
            { text: "2 day streak" },
          ],
        },
        total: {
          value: "1,037",
          label: "Total words dictated",
          sub: "Desktop · macOS",
          lines: [
            { text: "1,037 polished words across 21 sessions" },
          ],
        },
      },
      accuracy: {
        title: "Accuracy",
        sessions: "21 sessions",
        percent: 100,
        centerLabel: "accepted",
        rows: [
          { label: "Accepted as-is", value: "21" },
          { label: "Edited after paste", value: "0" },
        ],
        last: { label: "Last 7 days", value: "100%" },
      },
      streak: {
        title: "2 day streak",
        best: "BEST · 2D",
        filled: [
          { day: 6, col: 11, today: true, words: 42 },
          { day: 6, col: 10, words: 78 },
          { day: 0, col: 11, ghost: true, words: 0 },
        ],
      },
    },
  ],
};

export const appPreview = {
  eyebrow: "Inside Airnote",
  title: "The real app, no mockups.",
  body:
    "Configurable models, enterprise SSO, and the version channel you trust. Everything you see here ships in the app today.",
  // Tabbed real-screenshot viewer. Each tab shows an actual Airnote v2.3.2
  // settings page; the caption below the heading updates per tab so the
  // section reads as a guided tour rather than a dump of images.
  tabs: [
    {
      id: "models",
      label: "Models",
      caption:
        "Pick Fast for instant hotkey dictation, or Smart for complex sentences. ChatGPT plugs in for shortcut transforms and repair / refine.",
    },
    {
      id: "enterprise",
      label: "Enterprise",
      caption:
        "Connect Airnote to your organization's AirNote Enterprise server. SSO, central billing, and admin-managed vocabularies — all without leaving the app.",
    },
    {
      id: "about",
      label: "About",
      caption:
        "Anonymous diagnostics (audio + content stay on your device), Stable / Beta channels for the cautious or curious, and a one-click software update check.",
    },
  ],
};

export const integrations = {
  title: "Works wherever you type.",
  body:
    "Airnote pastes straight into whichever app has focus. If you can type in it, you can talk into it.",
  apps: [
    "Slack", "Notion", "Mail", "VS Code", "Chrome", "Safari", "Messages", "Linear",
    "Figma", "Cursor", "iTerm", "Discord", "Obsidian", "Things", "Bear", "Raycast",
    "Telegram", "WhatsApp", "Gmail", "Outlook", "Sublime", "Xcode", "Zed", "Asana",
    "Jira", "GitHub", "Twitter", "Sheets", "Docs", "Reflect",
  ],
  /** Apps featured as icon keys on the 3D keyboard, mapped to simple-icons slugs. */
  featured: [
    { name: "Slack", slug: "slack" },
    { name: "Notion", slug: "notion" },
    { name: "Zed", slug: "zed" },
    { name: "Xcode", slug: "xcode" },
    { name: "Chrome", slug: "googlechrome" },
    { name: "Safari", slug: "safari" },
    { name: "Linear", slug: "linear" },
    { name: "Figma", slug: "figma" },
    { name: "Discord", slug: "discord" },
    { name: "Obsidian", slug: "obsidian" },
    { name: "Raycast", slug: "raycast" },
    { name: "Telegram", slug: "telegram" },
    { name: "WhatsApp", slug: "whatsapp" },
    { name: "Gmail", slug: "gmail" },
    { name: "GitHub", slug: "github" },
  ],
};

export const heroDemo = {
  prePromptTitle: "Ready to see how it works?",
  playLabel: "Play Demo",
  replayLabel: "Replay Demo",
  // Notes mock content
  notes: {
    today: "Today",
    items: [
      { title: "New note", time: "6:27AM", preview: "No additional…" },
      { title: "Dream where I fo…", time: "7:15PM", preview: "When I put it t…" },
      { title: "List of books", time: "7:12PM", preview: "Doctor Faustu…" },
    ],
    dateHeader: "May 26, 2026 at 6:27AM",
    body:
      "Today's to-do list:\n• Review pull requests\n• Finish API documentation\n• Call with design team at 3pm",
  },
  // Slack mock content
  slack: {
    teamLabel: "Slack",
    channelLabel: "#standup",
    welcomeTitle: "Welcome to the #standup channel",
    welcomeBody:
      "This channel is for everything #standup. Hold meetings, share docs, and make decisions together with your team.",
    todayDivider: "Today",
    messageAuthor: "Lisa",
    messageTime: "10:00 AM",
    messageBody: "Morning team, any updates from last week?",
    inputPlaceholder: "Message #standup",
  },
  // Cursor mock content
  cursor: {
    tabName: "mod.rs",
    breadcrumb: "src › listeners › mod.rs",
    chatFile: "mod.rs",
    chatFileNote: "Current File",
    promptText:
      "How could I make it easier to switch certificates in the transport listeners?",
    modelChip: "claude-4.5-opus",
    modeChip: "chat",
    contextChip: "codebase",
  },
  dock: {
    airnoteLabel: "Airnote",
    slackLabel: "Slack",
    notesLabel: "Notes",
  },
};

export const whisperFlow = {
  eyebrow: "Behind the scenes",
  title: "From sound to sentence.",
  body:
    "Your voice goes in, a polished line of text comes out — usually before you've taken a breath.",
  spoken:
    "uh reschedule the offsite to next week i guess most of us are back from leave",
  cleaned:
    "Let's reschedule the offsite to next week — most of the team is back from leave.",
  badge: "Pasted into Notion",
  cloudLabel: "Cleaning up",
};

export const mobile = {
  eyebrow: "iOS companion",
  title: "Capture on the move.",
  body:
    "Hold the lock-screen widget, talk, and the cleaned text shows up on your Mac the moment you sit down. Your phone is now your best dictation hardware.",
  cta: { label: "Get it on the App Store", href: "#download" },
};

export const testimonials = {
  title: "What people actually do with it.",
  // PLACEHOLDER — do NOT ship without replacing with real, permissioned quotes.
  items: [
    {
      quote:
        "I wrote a whole code review out loud while walking my dog. Airnote handled the variable names without complaining.",
      name: "Engineer, Series B startup",
      role: "Placeholder — replace with real quote",
    },
    {
      quote:
        "It cut my email backlog by an hour a day. The 'email' tone genuinely sounds like me on a good day.",
      name: "Head of Sales",
      role: "Placeholder — replace with real quote",
    },
    {
      quote:
        "First dictation tool that doesn't make me edit afterward. I trust the punctuation.",
      name: "Journalist",
      role: "Placeholder — replace with real quote",
    },
  ],
};

export const pricing = {
  title: "Pay once, or not at all.",
  subtitle: "Start free. Upgrade when you'd miss it.",
  tiers: [
    {
      name: "Free",
      price: "$0",
      cadence: "forever",
      desc: "Everything you need to dictate daily.",
      features: [
        "Unlimited local transcription",
        "92 languages",
        "Hotkey in any app",
        "5 cloud minutes / day",
      ],
      cta: "Download",
      highlight: false,
    },
    {
      name: "Pro",
      price: "$8",
      cadence: "per month",
      desc: "For people who live in dictation.",
      features: [
        "Everything in Free",
        "Unlimited cloud minutes",
        "Custom tones & vocabularies",
        "iOS companion",
        "Priority models",
      ],
      cta: "Start Pro trial",
      highlight: true,
    },
    {
      name: "Lifetime",
      price: "$199",
      cadence: "one time",
      desc: "Own it. No subscription.",
      features: [
        "Everything in Pro",
        "All future major versions",
        "Team vocab sharing (5 seats)",
        "Founder support",
      ],
      cta: "Buy lifetime",
      highlight: false,
    },
  ],
};

export const faq = {
  title: "Things people ask.",
  items: [
    {
      q: "Does Airnote work offline?",
      a: "Yes. Switch to Airnote Local and a model runs entirely on your Mac. Nothing is sent over the network, and you can keep dictating on a flight or in a tunnel.",
    },
    {
      q: "Which Macs are supported?",
      a: "Any Mac running macOS 13 or later. Apple Silicon is required for the local on-device model. Intel Macs can still use the cloud model.",
    },
    {
      q: "What languages does it understand?",
      a: "92 languages out of the box, including code-switching mid-sentence. The cleaned-up output stays in whichever language you spoke in unless you ask for a translate mode.",
    },
    {
      q: "Is my audio stored?",
      a: "On the cloud model, audio is processed and discarded within seconds. On the local model, nothing leaves your Mac in the first place. You can opt out of all telemetry in settings.",
    },
    {
      q: "Can I use my own hotkey?",
      a: "Yes — rebind to any modifier combo, or use a side button on a mouse. You can also set per-app hotkeys.",
    },
    {
      q: "Is there a refund policy?",
      a: "Yes. 30 days, no questions, on Pro and Lifetime. Email and you'll get it back within a day.",
    },
  ],
};

export const footer = {
  tagline: "Voice to text, everywhere on your Mac.",
  columns: [
    {
      heading: "Product",
      links: [
        { label: "Features", href: "#features" },
        { label: "Pricing", href: "#pricing" },
        { label: "Download", href: downloads.mac.latestDmg },
        { label: "Changelog", href: "/changelog" },
      ],
    },
    {
      heading: "Resources",
      links: [
        { label: "Shortcuts", href: "#" },
        { label: "API", href: "#" },
        { label: "Status", href: "#" },
      ],
    },
    {
      heading: "Company",
      links: [
        { label: "About", href: "#" },
        { label: "Contact", href: "#" },
        { label: "Press kit", href: "#" },
        { label: "Careers", href: "#" },
      ],
    },
    {
      heading: "Legal",
      links: [
        { label: "Privacy", href: "#" },
        { label: "Terms", href: "#" },
        { label: "Security", href: "#" },
      ],
    },
  ],
  social: [
    { label: "X", href: "#" },
    { label: "GitHub", href: "#" },
    { label: "YouTube", href: "#" },
  ],
  copyright: "© 2026 Airnote, Inc.",
};
