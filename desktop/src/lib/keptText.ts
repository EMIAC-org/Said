import type { Recording } from "@/types";

type DictationText = Pick<Recording, "polished" | "final_text">;

/** The text a dictation ended up as: the user's edit when they changed what
 *  AirNote typed, otherwise AirNote's text. Every place that shows or copies a
 *  dictation uses this, so History and the dashboards agree. */
export function keptText(r: DictationText): string {
  return (r.final_text ?? "").trim() || (r.polished ?? "").trim();
}

/** The user changed AirNote's text after it was typed. */
export function wasEdited(r: DictationText): boolean {
  const kept = (r.final_text ?? "").trim();
  return kept.length > 0 && kept !== (r.polished ?? "").trim();
}
