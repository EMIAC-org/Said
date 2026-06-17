import type { Metadata } from "next";
import { readFileSync } from "node:fs";
import { join } from "node:path";

export const metadata: Metadata = {
  title: "AirNote — User Guide",
  description:
    "AirNote shortcuts, Polish Mode, selected-text polish, status pill placement, bug reporting, learning, and macOS permissions.",
};

const guideHtml = readFileSync(join(process.cwd(), "public", "guide.html"), "utf8");

function extractHtmlPart(pattern: RegExp, label: string) {
  const match = guideHtml.match(pattern);

  if (!match?.[1]) {
    throw new Error(`Could not parse ${label} from public/guide.html`);
  }

  return match[1];
}

const guideStyles = extractHtmlPart(/<style>([\s\S]*?)<\/style>/, "styles");
const guideBody = extractHtmlPart(/<body>([\s\S]*?)<\/body>/, "body");

export default function GuidePage() {
  return (
    <>
      <style dangerouslySetInnerHTML={{ __html: guideStyles }} />
      <div dangerouslySetInnerHTML={{ __html: guideBody }} />
    </>
  );
}
