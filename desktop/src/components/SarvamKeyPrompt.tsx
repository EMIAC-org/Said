import { useCallback, useState } from "react";
import { ExternalLink, Mic, X } from "lucide-react";
import { openExternal, patchPreferences, syncCredentialVault } from "@/lib/invoke";

const SARVAM_API_KEYS_URL = "https://dashboard.sarvam.ai/key-management";

interface Props {
  onDismiss: () => void;
  onSaved: () => void;
}

export function SarvamKeyPrompt({ onDismiss, onSaved }: Props) {
  const [key, setKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [vaultWarning, setVaultWarning] = useState("");

  const handleSave = useCallback(async () => {
    const trimmed = key.trim();
    if (!trimmed) {
      setError("Enter your Sarvam API key.");
      return;
    }
    setSaving(true);
    setError("");
    setVaultWarning("");
    try {
      const updated = await patchPreferences({
        sarvam_api_key: trimmed,
        stt_provider: "sarvam",
      });
      if (!updated) throw new Error("Failed to save preferences.");
      try {
        const vault = await syncCredentialVault();
        if (vault?.failed) {
          const firstErr = vault.results?.find((r) => r.error)?.error;
          setVaultWarning(
            firstErr ??
              "Key saved on this Mac, but server vault sync failed — will retry on next connect.",
          );
        }
      } catch {
        setVaultWarning(
          "Key saved on this Mac, but server vault sync failed — will retry on next connect.",
        );
      }
      onSaved();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to save key.");
    } finally {
      setSaving(false);
    }
  }, [key, onSaved]);

  return (
    <div
      className="fixed inset-0 z-[200] flex items-center justify-center p-6"
      style={{ background: "hsl(0 0% 0% / 0.55)" }}
      role="dialog"
      aria-modal="true"
      aria-labelledby="sarvam-prompt-title"
    >
      <div
        className="relative w-full max-w-md rounded-2xl p-6 shadow-2xl"
        style={{
          background: "hsl(var(--surface-1))",
          border: "1px solid hsl(var(--border))",
        }}
      >
        <button
          type="button"
          onClick={onDismiss}
          className="absolute right-4 top-4 p-1 rounded-md transition-colors"
          style={{ color: "hsl(var(--muted-foreground))" }}
          aria-label="Skip for now"
        >
          <X size={16} />
        </button>

        <div
          className="w-10 h-10 rounded-xl flex items-center justify-center mb-4"
          style={{ background: "hsl(var(--surface-4))", color: "hsl(var(--primary))" }}
        >
          <Mic size={18} />
        </div>

        <p className="text-[11px] font-semibold uppercase tracking-wide mb-1" style={{ color: "hsl(var(--muted-foreground))" }}>
          New model available
        </p>
        <h2 id="sarvam-prompt-title" className="text-[18px] font-semibold text-foreground leading-snug">
          Sarvam (Saaras v3)
        </h2>
        <p className="text-[13px] mt-2 leading-relaxed" style={{ color: "hsl(var(--muted-foreground))" }}>
          Stronger Hinglish transcription with codemix mode — batch on release. Add your free Sarvam API key to enable it; dictation keeps working on Deepgram until then.
        </p>

        <button
          type="button"
          onClick={() => void openExternal(SARVAM_API_KEYS_URL)}
          className="mt-3 inline-flex items-center gap-1.5 text-[12px] font-medium transition-colors"
          style={{ color: "hsl(var(--primary))" }}
        >
          Get a key from Sarvam dashboard
          <ExternalLink size={12} />
        </button>

        <div className="mt-5">
          <label className="text-[12px] font-medium text-foreground" htmlFor="sarvam-key-input">
            Sarvam API key
          </label>
          <input
            id="sarvam-key-input"
            type="password"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder="sk_…"
            className="input mt-1.5 w-full"
            style={{ fontFamily: "ui-monospace, SF Mono, Menlo, monospace" }}
            autoComplete="off"
          />
        </div>

        {error && (
          <p className="mt-2 text-[12px]" style={{ color: "hsl(var(--destructive))" }}>
            {error}
          </p>
        )}
        {vaultWarning && (
          <p className="mt-2 text-[12px]" style={{ color: "hsl(var(--chip-amber-fg))" }}>
            {vaultWarning}
          </p>
        )}

        <div className="mt-6 flex flex-col gap-2">
          <button
            type="button"
            onClick={() => void handleSave()}
            disabled={saving}
            className="btn-primary btn-lg w-full"
          >
            {saving ? "Saving…" : "Save & enable Sarvam"}
          </button>
          <button
            type="button"
            onClick={onDismiss}
            disabled={saving}
            className="btn-ghost btn-lg w-full"
          >
            Skip for now
          </button>
        </div>
      </div>
    </div>
  );
}
