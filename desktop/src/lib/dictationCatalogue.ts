import type { DictationRoute, SttSetupPolicy } from "./invoke";

/** A route AirNote can configure today. Keep this list limited to working paths. */
export interface DictationRouteOption {
  id: DictationRoute;
  kind: "local" | "cloud";
  label: string;
  provider: string;
  description: string;
  detail: string;
  badge?: string;
}

/**
 * The device policy decides whether local speech can run here; this catalogue
 * supplies the same clear, non-marketing route descriptions to setup and
 * Settings. It deliberately does not list planned providers or BYOK flows.
 */
export function dictationRouteOptions(policy: SttSetupPolicy): DictationRouteOption[] {
  const cloudOptions: DictationRouteOption[] = [
    {
      id: "cloud-deepinfra-whisper-v3-turbo",
      kind: "cloud",
      label: "Whisper Large V3 Turbo",
      provider: "DeepInfra",
      description: "Hosted transcription after you release the dictation key.",
      detail: "Internet required · recording is sent to DeepInfra",
    },
    {
      id: "cloud-openai-gpt-4o-mini-transcribe",
      kind: "cloud",
      label: "GPT-4o mini Transcribe",
      provider: "OpenAI",
      description: "Hosted transcription after you release the dictation key.",
      detail: "Internet required · recording is sent to OpenAI",
    },
  ];

  if (policy.setup_kind === "cloud_locked") return cloudOptions;

  return [
    {
      id: "local",
      kind: "local",
      label: policy.local_model_name ?? "Local speech recognition",
      provider: "On this Mac",
      description: "Speech recognition runs on this device before AirNote polishes the text.",
      detail: `${policy.local_model_size_hint ?? "Model download required"} · no cloud speech provider`,
      badge: "Recommended",
    },
    ...cloudOptions,
  ];
}

export function dictationRouteOption(
  policy: SttSetupPolicy,
  route: DictationRoute,
): DictationRouteOption {
  const options = dictationRouteOptions(policy);
  const selected = options.find((option) => option.id === route);
  if (selected) return selected;
  return options[0]!;
}
