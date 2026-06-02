// Compact GitHub-flavored-markdown renderer for Divo answers.
//
// Deliberately dependency-free (the desktop bundle ships no markdown lib): it
// covers the subset Divo emits — headings, bold/italic/inline-code, links,
// ordered/unordered lists, fenced code blocks, tables, blockquotes, hr and
// paragraphs. Styling lives in styles.css under `.divo-md`.

import { type ReactNode } from "react";

let keySeq = 0;
const k = () => `md-${keySeq++}`;

// ── Inline (bold / italic / code / links) ─────────────────────────────────────
function renderInline(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  // Order matters: code first (its contents are literal), then links, bold, italic.
  const re =
    /(`[^`]+`)|(\[[^\]]+\]\([^)]+\))|(\*\*[^*]+\*\*)|(__[^_]+__)|(\*[^*]+\*)|(_[^_]+_)/;
  let rest = text;
  let guard = 0;
  while (rest.length && guard++ < 5000) {
    const m = re.exec(rest);
    if (!m || m.index === undefined) {
      out.push(rest);
      break;
    }
    if (m.index > 0) out.push(rest.slice(0, m.index));
    const tok = m[0];
    if (tok.startsWith("`")) {
      out.push(<code key={k()}>{tok.slice(1, -1)}</code>);
    } else if (tok.startsWith("[")) {
      const mm = /\[([^\]]+)\]\(([^)]+)\)/.exec(tok);
      if (mm) {
        out.push(
          <a key={k()} href={mm[2]} target="_blank" rel="noreferrer">
            {mm[1]}
          </a>,
        );
      } else out.push(tok);
    } else if (tok.startsWith("**") || tok.startsWith("__")) {
      out.push(<strong key={k()}>{renderInline(tok.slice(2, -2))}</strong>);
    } else {
      out.push(<em key={k()}>{renderInline(tok.slice(1, -1))}</em>);
    }
    rest = rest.slice(m.index + tok.length);
  }
  return out;
}

function splitRow(line: string): string[] {
  return line
    .replace(/^\s*\|/, "")
    .replace(/\|\s*$/, "")
    .split("|")
    .map((c) => c.trim());
}

// ── Block parser ──────────────────────────────────────────────────────────────
export function Markdown({ content }: { content: string }) {
  const lines = (content || "").replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    // blank
    if (!line.trim()) {
      i++;
      continue;
    }

    // fenced code
    if (line.trim().startsWith("```")) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith("```")) {
        body.push(lines[i]);
        i++;
      }
      i++; // closing fence
      blocks.push(
        <pre key={k()}>
          <code>{body.join("\n")}</code>
        </pre>,
      );
      continue;
    }

    // heading
    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    if (h) {
      const level = h[1].length;
      const Tag = (`h${Math.min(level, 4)}`) as "h1" | "h2" | "h3" | "h4";
      blocks.push(<Tag key={k()}>{renderInline(h[2])}</Tag>);
      i++;
      continue;
    }

    // horizontal rule
    if (/^(\s*[-*_]){3,}\s*$/.test(line)) {
      blocks.push(<hr key={k()} />);
      i++;
      continue;
    }

    // table (header row + separator row of ---)
    if (line.includes("|") && i + 1 < lines.length && /^\s*\|?[\s:-]+\|[\s:|-]*$/.test(lines[i + 1])) {
      const header = splitRow(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].includes("|") && lines[i].trim()) {
        rows.push(splitRow(lines[i]));
        i++;
      }
      blocks.push(
        <table key={k()}>
          <thead>
            <tr>
              {header.map((c) => (
                <th key={k()}>{renderInline(c)}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={k()}>
                {r.map((c) => (
                  <td key={k()}>{renderInline(c)}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>,
      );
      continue;
    }

    // blockquote
    if (/^\s*>\s?/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        body.push(lines[i].replace(/^\s*>\s?/, ""));
        i++;
      }
      blocks.push(<blockquote key={k()}>{renderInline(body.join(" "))}</blockquote>);
      continue;
    }

    // unordered list
    if (/^\s*[-*+]\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*[-*+]\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*[-*+]\s+/, ""));
        i++;
      }
      blocks.push(
        <ul key={k()}>
          {items.map((it) => (
            <li key={k()}>{renderInline(it)}</li>
          ))}
        </ul>,
      );
      continue;
    }

    // ordered list
    if (/^\s*\d+\.\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*\d+\.\s+/, ""));
        i++;
      }
      blocks.push(
        <ol key={k()}>
          {items.map((it) => (
            <li key={k()}>{renderInline(it)}</li>
          ))}
        </ol>,
      );
      continue;
    }

    // paragraph (gather consecutive non-blank, non-structural lines)
    const para: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() &&
      !lines[i].trim().startsWith("```") &&
      !/^(#{1,6})\s+/.test(lines[i]) &&
      !/^\s*[-*+]\s+/.test(lines[i]) &&
      !/^\s*\d+\.\s+/.test(lines[i]) &&
      !/^\s*>\s?/.test(lines[i])
    ) {
      para.push(lines[i]);
      i++;
    }
    if (para.length) blocks.push(<p key={k()}>{renderInline(para.join(" "))}</p>);
  }

  return <div className="divo-md">{blocks}</div>;
}
