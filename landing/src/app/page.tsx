import { Nav } from "@/components/sections/Nav";
import { Hero } from "@/components/sections/Hero";
import { ShortcutDemo } from "@/components/sections/ShortcutDemo";
import { LogoStrip } from "@/components/sections/LogoStrip";
import { ToneSwitcher } from "@/components/sections/ToneSwitcher";
import { FeatureGrid } from "@/components/sections/FeatureGrid";
import { InsightsDashboard } from "@/components/sections/InsightsDashboard";
import { AppPreview } from "@/components/sections/SettingsPreview";
import { WhisperFlow } from "@/components/sections/WhisperFlow";
import { MobileSection } from "@/components/sections/MobileSection";
import { Pricing } from "@/components/sections/Pricing";
import { FAQ } from "@/components/sections/FAQ";
import { Footer } from "@/components/sections/Footer";

export default function Page() {
  return (
    <>
      {/* Skip-to-content link — hidden by default, visible on keyboard focus.
          Lets keyboard users jump past the fixed Nav straight into the main
          content. Standard a11y pattern. */}
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:fixed focus:left-4 focus:top-4 focus:z-[100] focus:rounded-full focus:bg-white focus:px-4 focus:py-2 focus:text-sm focus:font-medium focus:text-ink-900 focus:shadow-lg"
      >
        Skip to content
      </a>
      <Nav />
      <main id="main">
        <Hero />
        <LogoStrip />
        <ToneSwitcher />
        <FeatureGrid />
        <InsightsDashboard />
        <AppPreview />
        {/* ShortcutDemo (the "One hotkey. Every app." keyboard with the
            highlighted ⌥+Space + brand icons on the number row) sits at
            this deeper position so it acts as the page's "this is how
            it works" reveal after the user has seen the value props. The
            standalone Integrations section is folded into this one — its
            icon row already lives on the keyboard. */}
        <ShortcutDemo />
        <WhisperFlow />
        <MobileSection />
        <Pricing />
        <FAQ />
      </main>
      <Footer />
    </>
  );
}
