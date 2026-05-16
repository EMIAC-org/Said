interface Props {
  backendOk: boolean;
}

export function Sidebar({ backendOk }: Props) {
  return (
    <nav style={s.sidebar}>
      <div style={s.brand}>
        <div style={s.logo}>S</div>
        <div>
          <div style={s.brandTitle}>Learning Lab</div>
          <div style={s.brandVer}>Said v2.1.7</div>
        </div>
      </div>

      <div style={s.section}>Pipeline</div>
      <NavItem icon="◉" label="Live Trace" active />
      <NavItem icon="⚙" label="Batch Test" />
      <NavItem icon="☷" label="Variant Tracker" />

      <div style={s.section}>Learning</div>
      <NavItem icon="★" label="Vocabulary" />
      <NavItem icon="⇄" label="STT Rules" />
      <NavItem icon="✎" label="Corrections" />

      <div style={s.section}>Analysis</div>
      <NavItem icon="▤" label="Quality Metrics" />
      <NavItem icon="⚖" label="Coverage Map" />

      <div style={{ flex: 1 }} />

      <div style={{
        ...s.status,
        ...(backendOk ? s.statusOk : s.statusErr),
      }}>
        <span style={{
          ...s.dot,
          background: backendOk ? "var(--green)" : "var(--red)",
        }} />
        {backendOk ? "Backend connected" : "Disconnected"} &middot; :48484
      </div>
    </nav>
  );
}

function NavItem({ icon, label, active }: { icon: string; label: string; active?: boolean }) {
  return (
    <div style={{ ...s.item, ...(active ? s.itemActive : {}) }}>
      <span style={s.ico}>{icon}</span>
      {label}
    </div>
  );
}

const s: Record<string, React.CSSProperties> = {
  sidebar: {
    gridRow: "1 / -1",
    background: "var(--bg-card)",
    borderRight: "1px solid var(--border-subtle)",
    padding: "20px 0",
    display: "flex",
    flexDirection: "column",
    overflow: "hidden",
  },
  brand: {
    padding: "0 18px 22px",
    display: "flex",
    alignItems: "center",
    gap: 10,
  },
  logo: {
    width: 32, height: 32, borderRadius: 8,
    background: "var(--accent-bg)",
    border: "1px solid rgba(129,140,248,0.15)",
    display: "flex", alignItems: "center", justifyContent: "center",
    color: "var(--accent)", fontSize: 15, fontWeight: 800,
  },
  brandTitle: { fontSize: 15, fontWeight: 700, letterSpacing: "-0.01em" },
  brandVer: { fontSize: 10, color: "var(--text-faint)" },
  section: {
    padding: "16px 18px 6px",
    fontSize: 10, fontWeight: 600,
    color: "var(--text-faint)",
    textTransform: "uppercase",
    letterSpacing: "0.08em",
  },
  item: {
    display: "flex", alignItems: "center", gap: 9,
    padding: "7px 18px",
    fontSize: 13, fontWeight: 500,
    color: "var(--text-sec)",
    cursor: "pointer",
    borderLeft: "2px solid transparent",
    transition: "all 0.1s",
  },
  itemActive: {
    color: "var(--accent)",
    borderLeftColor: "var(--accent)",
    background: "var(--accent-bg)",
  },
  ico: { width: 16, textAlign: "center", fontSize: 13, opacity: 0.6 },
  status: {
    margin: "0 12px",
    padding: "10px 12px",
    borderRadius: "var(--radius-sm)",
    display: "flex", alignItems: "center", gap: 8,
    fontSize: 11, fontWeight: 500,
  },
  statusOk: {
    background: "var(--green-bg)",
    border: "1px solid var(--green-border)",
    color: "var(--green)",
  },
  statusErr: {
    background: "var(--red-bg)",
    border: "1px solid var(--red-border)",
    color: "var(--red)",
  },
  dot: { width: 6, height: 6, borderRadius: "50%", flexShrink: 0 },
};
