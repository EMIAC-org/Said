#!/usr/bin/env python3
"""Run Sortformer diarization and reconcile speakers onto ASR segments.

The final transcript keeps ASR text as the source of truth. Diarization is used
only to assign speaker labels by time overlap, so missed diarization regions do
not delete transcript lines.
"""

from __future__ import annotations

import argparse
import itertools
import json
import os
import re
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple


DEFAULT_SORTFORMER_REPO = "nvidia/diar_streaming_sortformer_4spk-v2.1"
DEFAULT_SORTFORMER_FILE = "diar_streaming_sortformer_4spk-v2.1.nemo"


@dataclass
class TranscriptSegment:
    source: str
    speaker_id: str
    speaker_name: str
    start_ms: int
    end_ms: int
    text: str


@dataclass
class DiarizationSegment:
    speaker_id: str
    speaker_name: str
    start_ms: int
    end_ms: int
    confidence: float = 0.72


@dataclass
class SourceActivitySegment:
    source: str
    start_ms: int
    end_ms: int
    mic_rms: float = 0.0
    system_rms: float = 0.0


def now_ms() -> int:
    return int(time.time() * 1000)


def read_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as fh:
        return json.load(fh)


def write_json(path: Path, obj: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def clean_text(text: Any) -> str:
    return " ".join(str(text or "").strip().split())


def ms_from_value(value: Any) -> int:
    if value is None:
        return 0
    if isinstance(value, str):
        value = value.strip()
        if not value:
            return 0
        if ":" in value:
            return ms_from_timestamp(value)
        value = float(value)
    if isinstance(value, float) and value < 10_000:
        return int(round(value * 1000))
    return int(round(float(value)))


def ms_from_timestamp(value: str) -> int:
    match = re.match(r"(?:(\d+):)?(\d+):(\d+)[,.](\d+)", value.strip())
    if not match:
        return 0
    hours = int(match.group(1) or 0)
    minutes = int(match.group(2))
    seconds = int(match.group(3))
    millis = int((match.group(4) + "000")[:3])
    return ((hours * 60 + minutes) * 60 + seconds) * 1000 + millis


def load_transcript_segments(path: Path) -> Tuple[List[TranscriptSegment], Dict[str, Any]]:
    data = read_json(path)
    segments: List[TranscriptSegment] = []

    if isinstance(data, dict) and isinstance(data.get("segments"), list):
        for item in data["segments"]:
            text = clean_text(item.get("text"))
            if not text:
                continue
            start_ms = ms_from_value(item.get("start_ms", item.get("start")))
            end_ms = max(ms_from_value(item.get("end_ms", item.get("end"))), start_ms)
            segments.append(
                TranscriptSegment(
                    source=str(item.get("source") or "asr"),
                    speaker_id=str(item.get("speaker_id") or "unknown_speaker"),
                    speaker_name=str(item.get("speaker_name") or "Unknown Speaker"),
                    start_ms=start_ms,
                    end_ms=end_ms,
                    text=text,
                )
            )
        return segments, data

    if isinstance(data, dict) and isinstance(data.get("transcription"), list):
        for item in data["transcription"]:
            text = clean_text(item.get("text"))
            if not text:
                continue
            offsets = item.get("offsets") or {}
            start_ms = ms_from_value(offsets.get("from"))
            end_ms = max(ms_from_value(offsets.get("to")), start_ms)
            segments.append(
                TranscriptSegment(
                    source="asr",
                    speaker_id="unknown_speaker",
                    speaker_name="Unknown Speaker",
                    start_ms=start_ms,
                    end_ms=end_ms,
                    text=text,
                )
            )
        return segments, data

    raise ValueError(f"unsupported transcript JSON format: {path}")


def parse_sortformer_line(line: str) -> Optional[DiarizationSegment]:
    parts = line.strip().split()
    if len(parts) < 3:
        return None
    try:
        start_ms = int(round(float(parts[0]) * 1000))
        end_ms = int(round(float(parts[1]) * 1000))
    except ValueError:
        return None
    raw_speaker = parts[2]
    return DiarizationSegment(
        speaker_id=raw_speaker,
        speaker_name=raw_speaker,
        start_ms=start_ms,
        end_ms=max(end_ms, start_ms),
    )


def flatten_sortformer_output(raw: Any) -> List[str]:
    if isinstance(raw, str):
        return [line for line in raw.splitlines() if line.strip()]
    if isinstance(raw, list):
        lines: List[str] = []
        for item in raw:
            lines.extend(flatten_sortformer_output(item))
        return lines
    return []


def load_diarization_segments(path: Path) -> Tuple[List[DiarizationSegment], Dict[str, Any]]:
    data = read_json(path)
    segments: List[DiarizationSegment] = []

    if isinstance(data, dict) and isinstance(data.get("segments"), list):
        for item in data["segments"]:
            start_ms = ms_from_value(item.get("start_ms", item.get("start")))
            end_ms = max(ms_from_value(item.get("end_ms", item.get("end"))), start_ms)
            speaker_id = str(item.get("speaker_id") or item.get("speaker") or "speaker_0")
            segments.append(
                DiarizationSegment(
                    speaker_id=speaker_id,
                    speaker_name=str(item.get("speaker_name") or speaker_id),
                    start_ms=start_ms,
                    end_ms=end_ms,
                    confidence=float(item.get("confidence") or 0.72),
                )
            )
        return normalize_diarization_speakers(segments), data

    lines = flatten_sortformer_output(data)
    for line in lines:
        parsed = parse_sortformer_line(line)
        if parsed is not None:
            segments.append(parsed)
    return normalize_diarization_speakers(segments), data


def run_sortformer(audio: Path, args: argparse.Namespace) -> Tuple[List[DiarizationSegment], Dict[str, Any]]:
    try:
        import torch  # type: ignore
        from huggingface_hub import hf_hub_download  # type: ignore
        from nemo.collections.asr.models import SortformerEncLabelModel  # type: ignore
    except ImportError as exc:
        raise RuntimeError(
            "NeMo Sortformer dependencies are not installed in this Python environment"
        ) from exc

    model_path = args.model_path
    if model_path:
        model_file = Path(model_path).expanduser()
    else:
        env_model = os.environ.get("AIRNOTE_NEMO_SORTFORMER_MODEL_PATH", "").strip()
        if env_model:
            model_file = Path(env_model).expanduser()
        else:
            model_cache = Path(
                args.model_cache
                or os.environ.get("AIRNOTE_NEMO_MODEL_CACHE", "")
                or "~/Library/Application Support/VoicePolish/models/nemo"
            ).expanduser()
            local_candidate = model_cache / DEFAULT_SORTFORMER_FILE
            if local_candidate.is_file():
                model_file = local_candidate
            else:
                model_file = Path(
                    hf_hub_download(
                        repo_id=args.model_repo,
                        filename=args.model_filename,
                        cache_dir=str(model_cache),
                    )
                )

    if not model_file.is_file():
        raise RuntimeError(f"Sortformer model file not found: {model_file}")

    map_location = torch.device(args.device) if args.device else None
    model = SortformerEncLabelModel.restore_from(str(model_file), map_location=map_location)
    started = time.perf_counter()
    raw = model.diarize(
        audio=str(audio),
        batch_size=args.batch_size,
        num_workers=0,
        verbose=False,
    )
    elapsed_s = time.perf_counter() - started
    segments = [
        segment
        for line in flatten_sortformer_output(raw)
        if (segment := parse_sortformer_line(line)) is not None
    ]
    meta = {
        "provider": "nemo_sortformer",
        "repo": args.model_repo,
        "model_file": str(model_file),
        "audio": str(audio),
        "elapsed_s": elapsed_s,
        "raw_type": str(type(raw)),
    }
    return normalize_diarization_speakers(segments), meta


def normalize_diarization_speakers(
    segments: Sequence[DiarizationSegment],
) -> List[DiarizationSegment]:
    mapping: Dict[str, Tuple[str, str]] = {}
    normalized: List[DiarizationSegment] = []
    for segment in sorted(segments, key=lambda item: (item.start_ms, item.end_ms)):
        if segment.speaker_id not in mapping:
            idx = len(mapping) + 1
            mapping[segment.speaker_id] = (f"speaker_{idx}", f"Speaker {idx}")
        speaker_id, speaker_name = mapping[segment.speaker_id]
        normalized.append(
            DiarizationSegment(
                speaker_id=speaker_id,
                speaker_name=speaker_name,
                start_ms=segment.start_ms,
                end_ms=segment.end_ms,
                confidence=segment.confidence,
            )
        )
    return normalized


def overlap_ms(start_a: int, end_a: int, start_b: int, end_b: int) -> int:
    return max(0, min(end_a, end_b) - max(start_a, start_b))


def nearest_gap_ms(segment: TranscriptSegment, diar: DiarizationSegment) -> int:
    if overlap_ms(segment.start_ms, segment.end_ms, diar.start_ms, diar.end_ms) > 0:
        return 0
    if segment.end_ms <= diar.start_ms:
        return diar.start_ms - segment.end_ms
    return segment.start_ms - diar.end_ms


def assign_speakers(
    transcript_segments: Sequence[TranscriptSegment],
    diarization_segments: Sequence[DiarizationSegment],
    max_nearest_gap_ms: int,
) -> Tuple[List[Dict[str, Any]], Dict[str, int]]:
    final_segments: List[Dict[str, Any]] = []
    stats = {"assigned_by_overlap": 0, "assigned_by_nearest": 0, "unassigned": 0}

    for segment in transcript_segments:
        overlaps: Dict[str, Tuple[int, DiarizationSegment]] = {}
        for diar in diarization_segments:
            amount = overlap_ms(segment.start_ms, segment.end_ms, diar.start_ms, diar.end_ms)
            if amount <= 0:
                continue
            previous = overlaps.get(diar.speaker_id)
            if previous is None or amount > previous[0]:
                overlaps[diar.speaker_id] = (amount, diar)

        status = "unassigned"
        assigned: Optional[DiarizationSegment] = None
        overlap_total = 0
        if overlaps:
            overlap_total, assigned = max(overlaps.values(), key=lambda item: item[0])
            status = "assigned_by_overlap"
            stats["assigned_by_overlap"] += 1
        elif max_nearest_gap_ms >= 0 and diarization_segments:
            nearest = min(
                diarization_segments,
                key=lambda diar: nearest_gap_ms(segment, diar),
            )
            gap = nearest_gap_ms(segment, nearest)
            if gap <= max_nearest_gap_ms:
                assigned = nearest
                status = "assigned_by_nearest"
                stats["assigned_by_nearest"] += 1

        if assigned is None:
            speaker_id = (
                segment.speaker_id
                if segment.speaker_id and segment.speaker_id != "unknown_speaker"
                else "unknown_speaker"
            )
            speaker_name = (
                segment.speaker_name
                if segment.speaker_name and segment.speaker_name != "Unknown Speaker"
                else "Unknown Speaker"
            )
            stats["unassigned"] += 1
        else:
            speaker_id = assigned.speaker_id
            speaker_name = assigned.speaker_name

        speech_start_ms = assigned.start_ms if assigned is not None else segment.start_ms
        speech_end_ms = assigned.end_ms if assigned is not None else segment.end_ms

        final_segments.append(
            {
                "source": segment.source,
                "speaker_id": speaker_id,
                "speaker_name": speaker_name,
                "start_ms": segment.start_ms,
                "end_ms": segment.end_ms,
                "transcript_start_ms": segment.start_ms,
                "transcript_end_ms": segment.end_ms,
                "speech_start_ms": speech_start_ms,
                "speech_end_ms": speech_end_ms,
                "display_start_ms": speech_start_ms,
                "display_end_ms": speech_end_ms,
                "text": segment.text,
                "diarization_status": status,
                "diarization_overlap_ms": overlap_total,
            }
        )

    return final_segments, stats


def speakers_from_diarization(segments: Sequence[DiarizationSegment]) -> List[Dict[str, Any]]:
    seen: Dict[str, str] = {}
    for segment in segments:
        seen.setdefault(segment.speaker_id, segment.speaker_name)
    return [
        {
            "speaker_id": speaker_id,
            "speaker_name": speaker_name,
            "source": "final_diarization",
            "role": "meeting_participant",
        }
        for speaker_id, speaker_name in seen.items()
    ]


def source_for_wav(path: Path) -> str:
    name = path.name.lower()
    if name == "mic.wav" or name.startswith("mic."):
        return "mic"
    if name == "system.wav" or name.startswith("system."):
        return "system"
    return path.stem.lower().replace(" ", "_") or "track"


def source_sort_key(source: str) -> Tuple[int, str]:
    if source == "mic":
        return (0, source)
    if source == "system":
        return (1, source)
    return (2, source)


def resolve_artifact_path(raw_path: Any, base_dir: Path) -> Optional[Path]:
    if not raw_path:
        return None
    path = Path(str(raw_path)).expanduser()
    if path.is_absolute():
        return path
    return (base_dir / path).resolve()


def source_wavs_from_transcript(source_transcript: Dict[str, Any], base_dir: Path) -> Dict[str, Path]:
    out: Dict[str, Path] = {}
    for raw_path in source_transcript.get("source_wavs") or []:
        path = resolve_artifact_path(raw_path, base_dir)
        if path is None:
            continue
        out[source_for_wav(path)] = path
    return out


def load_source_activity(
    source_transcript: Dict[str, Any],
    base_dir: Path,
) -> Tuple[List[SourceActivitySegment], Optional[str]]:
    candidates: List[Path] = []
    for key in ("source_activity_path", "audio_manifest_path"):
        path = resolve_artifact_path(source_transcript.get(key), base_dir)
        if path is not None:
            candidates.append(path)
    candidates.extend(
        [
            base_dir / "meeting.source-activity.json",
            base_dir / "meeting.audio.json",
        ]
    )

    seen: set[str] = set()
    for path in candidates:
        key = str(path)
        if key in seen or not path.is_file():
            continue
        seen.add(key)
        try:
            data = read_json(path)
        except Exception:
            continue
        raw_segments = data.get("source_activity_segments") if isinstance(data, dict) else None
        if not isinstance(raw_segments, list):
            continue
        segments: List[SourceActivitySegment] = []
        for item in raw_segments:
            if not isinstance(item, dict):
                continue
            start_ms = ms_from_value(item.get("start_ms", item.get("start")))
            end_ms = max(ms_from_value(item.get("end_ms", item.get("end"))), start_ms)
            if end_ms <= start_ms:
                continue
            segments.append(
                SourceActivitySegment(
                    source=str(item.get("source") or "unknown"),
                    start_ms=start_ms,
                    end_ms=end_ms,
                    mic_rms=float(item.get("mic_rms") or 0.0),
                    system_rms=float(item.get("system_rms") or 0.0),
                )
            )
        if segments:
            return segments, str(path)
    return [], None


def segment_display_window(segment: Dict[str, Any]) -> Tuple[int, int]:
    start_ms = int(segment.get("display_start_ms") or segment.get("start_ms") or 0)
    end_ms = int(segment.get("display_end_ms") or segment.get("end_ms") or start_ms)
    return start_ms, max(end_ms, start_ms)


def gap_ms(start_a: int, end_a: int, start_b: int, end_b: int) -> int:
    if overlap_ms(start_a, end_a, start_b, end_b) > 0:
        return 0
    if end_a <= start_b:
        return start_b - end_a
    return start_a - end_b


def system_activity_metrics(
    activities: Sequence[SourceActivitySegment],
    start_ms: int,
    end_ms: int,
) -> Dict[str, float]:
    window_ms = max(end_ms - start_ms, 1)
    system_activity_ms = 0
    weighted_mic = 0.0
    weighted_system = 0.0
    total_activity_ms = 0

    for item in activities:
        amount = overlap_ms(start_ms, end_ms, item.start_ms, item.end_ms)
        if amount <= 0:
            continue
        total_activity_ms += amount
        weighted_mic += item.mic_rms * amount
        weighted_system += item.system_rms * amount
        if item.source == "system_audio":
            system_activity_ms += amount

    mic_rms = weighted_mic / max(total_activity_ms, 1)
    system_rms = weighted_system / max(total_activity_ms, 1)
    return {
        "system_activity_ratio": system_activity_ms / window_ms,
        "mic_rms": mic_rms,
        "system_rms": system_rms,
        "system_to_mic_rms_ratio": system_rms / max(mic_rms, 1e-9),
    }


def track_silence_metrics(
    activities: Sequence[SourceActivitySegment],
    start_ms: int,
    end_ms: int,
    source: str,
) -> Dict[str, float]:
    window_ms = max(end_ms - start_ms, 1)
    total_activity_ms = 0
    silence_ms = 0
    weighted_track = 0.0

    for item in activities:
        amount = overlap_ms(start_ms, end_ms, item.start_ms, item.end_ms)
        if amount <= 0:
            continue
        total_activity_ms += amount
        if item.source == "silence":
            silence_ms += amount
        track_rms = item.mic_rms if source == "mic" else item.system_rms
        weighted_track += track_rms * amount

    return {
        "covered_ratio": total_activity_ms / window_ms,
        "silence_ratio": silence_ms / window_ms,
        "track_rms": weighted_track / max(total_activity_ms, 1),
    }


def normalized_tokens(text: str) -> set[str]:
    return {
        token
        for token in re.findall(r"[a-z0-9]+", clean_text(text).lower())
        if token
    }


def normalized_token_list(text: str) -> List[str]:
    return [
        token
        for token in re.findall(r"[a-z0-9]+", clean_text(text).lower())
        if token
    ]


def text_overlap_metrics(left: str, right: str) -> Dict[str, float]:
    left_tokens = set(normalized_token_list(left))
    right_tokens = set(normalized_token_list(right))
    if not left_tokens and not right_tokens:
        return {"jaccard": 1.0, "containment": 1.0, "token_overlap": 0.0}
    if not left_tokens or not right_tokens:
        return {"jaccard": 0.0, "containment": 0.0, "token_overlap": 0.0}
    overlap = len(left_tokens & right_tokens)
    return {
        "jaccard": overlap / len(left_tokens | right_tokens),
        "containment": overlap / min(len(left_tokens), len(right_tokens)),
        "token_overlap": float(overlap),
    }


def text_similarity(left: str, right: str) -> float:
    return text_overlap_metrics(left, right)["jaccard"]


def timestamp_drift_ms(segment: Dict[str, Any]) -> int:
    display_start, _ = segment_display_window(segment)
    raw_start = segment.get("transcript_start_ms")
    if raw_start is None:
        raw_start = segment.get("start_ms")
    transcript_start = int(display_start if raw_start is None else raw_start)
    return abs(transcript_start - display_start)


def choose_duplicate_text(
    system_segment: Dict[str, Any],
    mic_echo_segment: Dict[str, Any],
    *,
    similarity_threshold: float,
    drift_margin_ms: int,
    allow_mic_echo_text: bool,
) -> Tuple[str, str, Dict[str, Any]]:
    system_text = clean_text(system_segment.get("text"))
    mic_text = clean_text(mic_echo_segment.get("text"))
    similarity = text_similarity(system_text, mic_text)
    system_drift = timestamp_drift_ms(system_segment)
    mic_drift = timestamp_drift_ms(mic_echo_segment)

    chosen_text = system_text
    chosen_source = "system"
    reason = "canonical_system_track"

    if not system_text and mic_text:
        chosen_text = mic_text
        chosen_source = "mic_echo"
        reason = "system_text_empty"
    elif (
        allow_mic_echo_text
        and mic_text
        and similarity < similarity_threshold
        and system_drift >= mic_drift + drift_margin_ms
    ):
        chosen_text = mic_text
        chosen_source = "mic_echo"
        reason = "lower_timestamp_drift"

    return chosen_text, chosen_source, {
        "text_similarity": round(similarity, 4),
        "system_timestamp_drift_ms": system_drift,
        "mic_echo_timestamp_drift_ms": mic_drift,
        "chosen_text_source": chosen_source,
        "chosen_text_reason": reason,
    }


def find_remote_system_match(
    mic_segment: Dict[str, Any],
    final_segments: Sequence[Dict[str, Any]],
    max_gap_ms: int,
) -> Optional[Dict[str, Any]]:
    candidates = find_remote_system_candidates(mic_segment, final_segments, max_gap_ms)
    if not candidates:
        return None
    return candidates[0][2]


def find_remote_system_candidates(
    mic_segment: Dict[str, Any],
    final_segments: Sequence[Dict[str, Any]],
    max_gap_ms: int,
) -> List[Tuple[int, int, Dict[str, Any]]]:
    mic_start, mic_end = segment_display_window(mic_segment)
    candidates: List[Tuple[int, int, Dict[str, Any]]] = []
    for segment in final_segments:
        if segment is mic_segment or segment.get("source") != "system":
            continue
        system_start, system_end = segment_display_window(segment)
        amount = overlap_ms(mic_start, mic_end, system_start, system_end)
        gap = gap_ms(mic_start, mic_end, system_start, system_end)
        if amount <= 0 and gap > max_gap_ms:
            continue
        candidates.append((amount, -gap, segment))
    return sorted(candidates, key=lambda item: (item[0], item[1]), reverse=True)


def is_text_duplicate_echo(
    system_segment: Dict[str, Any],
    mic_segment: Dict[str, Any],
    args: argparse.Namespace,
) -> Tuple[bool, Dict[str, Any]]:
    metrics = text_overlap_metrics(
        clean_text(system_segment.get("text")),
        clean_text(mic_segment.get("text")),
    )
    is_duplicate = (
        metrics["token_overlap"] >= args.echo_min_token_overlap
        and (
            metrics["jaccard"] >= args.echo_text_similarity_threshold
            or metrics["containment"] >= args.echo_text_containment_threshold
        )
    )
    return is_duplicate, {
        "text_jaccard": round(metrics["jaccard"], 4),
        "text_containment": round(metrics["containment"], 4),
        "token_overlap": int(metrics["token_overlap"]),
        "text_duplicate": is_duplicate,
    }


def suppress_trackwise_echo(
    final_segments: Sequence[Dict[str, Any]],
    source_activity: Sequence[SourceActivitySegment],
    args: argparse.Namespace,
) -> Tuple[List[Dict[str, Any]], Dict[str, Any]]:
    stats: Dict[str, Any] = {
        "status": "disabled" if args.disable_echo_suppression else "completed",
        "source_activity_path": None,
        "suppressed_segments": 0,
        "merged_segments": 0,
        "candidates": [],
    }
    if args.disable_echo_suppression:
        return list(final_segments), stats

    if not source_activity:
        stats["status"] = "completed_text_only"
    suppressed_ids: set[int] = set()
    replacements: Dict[int, Dict[str, Any]] = {}

    for mic_segment in final_segments:
        if mic_segment.get("source") != "mic":
            continue
        mic_start, mic_end = segment_display_window(mic_segment)
        system_candidates = find_remote_system_candidates(
            mic_segment,
            final_segments,
            args.echo_max_pair_gap_ms,
        )
        if not system_candidates:
            continue
        activity = system_activity_metrics(source_activity, mic_start, mic_end)
        activity_duplicate = (
            bool(source_activity)
            and activity["system_activity_ratio"] >= args.echo_activity_overlap_ratio
            and activity["system_to_mic_rms_ratio"] >= args.echo_system_rms_ratio
        )
        best_match: Optional[Tuple[Tuple[float, float, float, int, int], Dict[str, Any], Dict[str, Any], bool]] = None
        for overlap_amount, negative_gap, candidate in system_candidates:
            text_duplicate, candidate_text_metrics = is_text_duplicate_echo(candidate, mic_segment, args)
            if not activity_duplicate and not text_duplicate:
                continue
            score = (
                1.0 if text_duplicate else 0.0,
                float(candidate_text_metrics["text_containment"]),
                float(candidate_text_metrics["text_jaccard"]),
                int(candidate_text_metrics["token_overlap"]),
                overlap_amount + negative_gap,
            )
            if best_match is None or score > best_match[0]:
                best_match = (score, candidate, candidate_text_metrics, text_duplicate)
        if best_match is None:
            continue

        _score, system_segment, text_metrics, text_duplicate = best_match
        if not activity_duplicate and not text_duplicate:
            continue

        chosen_text, chosen_source, text_decision = choose_duplicate_text(
            system_segment,
            mic_segment,
            similarity_threshold=args.echo_text_similarity_threshold,
            drift_margin_ms=args.echo_drift_margin_ms,
            allow_mic_echo_text=args.echo_allow_mic_text,
        )
        merged = dict(system_segment)
        merged["text"] = chosen_text
        merged["echo_suppression"] = {
            "status": "merged_remote_echo",
            "suppressed_source": "mic",
            "suppressed_speaker_id": mic_segment.get("speaker_id"),
            "suppressed_speaker_name": mic_segment.get("speaker_name"),
            "activity": activity,
            "activity_duplicate": activity_duplicate,
            **text_metrics,
            **text_decision,
        }
        merged["alternate_texts"] = [
            {
                "source": "system",
                "speaker_name": system_segment.get("speaker_name"),
                "text": system_segment.get("text"),
            },
            {
                "source": "mic_echo",
                "speaker_name": mic_segment.get("speaker_name"),
                "text": mic_segment.get("text"),
            },
        ]

        suppressed_ids.add(id(mic_segment))
        replacements[id(system_segment)] = merged
        stats["suppressed_segments"] += 1
        stats["merged_segments"] += 1
        stats["candidates"].append(
            {
                "system_speaker": system_segment.get("speaker_name"),
                "mic_echo_speaker": mic_segment.get("speaker_name"),
                "system_text": system_segment.get("text"),
                "mic_echo_text": mic_segment.get("text"),
                "chosen_text_source": chosen_source,
                "activity": activity,
                "activity_duplicate": activity_duplicate,
                **text_metrics,
                **text_decision,
            }
        )

    merged_segments: List[Dict[str, Any]] = []
    for segment in final_segments:
        if id(segment) in suppressed_ids:
            continue
        merged_segments.append(replacements.get(id(segment), segment))
    return merged_segments, stats


def audio_duration_ms_from_transcript(source_transcript: Dict[str, Any]) -> int:
    for key in ("audio_duration_ms", "duration_ms"):
        value = source_transcript.get(key)
        if value is not None:
            try:
                return max(0, int(round(float(value))))
            except (TypeError, ValueError):
                pass
    return 0


def suppress_out_of_bounds_unassigned_segments(
    final_segments: Sequence[Dict[str, Any]],
    source_transcript: Dict[str, Any],
    args: argparse.Namespace,
) -> Tuple[List[Dict[str, Any]], Dict[str, Any]]:
    duration_ms = audio_duration_ms_from_transcript(source_transcript)
    stats: Dict[str, Any] = {
        "status": "skipped_no_duration" if duration_ms <= 0 else "completed",
        "audio_duration_ms": duration_ms,
        "suppressed_segments": 0,
        "candidates": [],
    }
    if duration_ms <= 0:
        return list(final_segments), stats

    kept: List[Dict[str, Any]] = []
    margin_ms = args.unassigned_out_of_bounds_margin_ms
    for segment in final_segments:
        end_ms = int(segment.get("end_ms") or segment.get("transcript_end_ms") or 0)
        if (
            segment.get("diarization_status") == "unassigned"
            and int(segment.get("diarization_overlap_ms") or 0) <= 0
            and end_ms > duration_ms + margin_ms
        ):
            stats["suppressed_segments"] += 1
            stats["candidates"].append(
                {
                    "source": segment.get("source"),
                    "speaker_name": segment.get("speaker_name"),
                    "start_ms": segment.get("start_ms"),
                    "end_ms": segment.get("end_ms"),
                    "text": segment.get("text"),
                    "reason": "unassigned_segment_extends_beyond_audio_duration",
                }
            )
            continue
        kept.append(segment)
    return kept, stats


def suppress_silent_unassigned_tail_segments(
    final_segments: Sequence[Dict[str, Any]],
    source_transcript: Dict[str, Any],
    source_activity: Sequence[SourceActivitySegment],
    args: argparse.Namespace,
) -> Tuple[List[Dict[str, Any]], Dict[str, Any]]:
    duration_ms = audio_duration_ms_from_transcript(source_transcript)
    stats: Dict[str, Any] = {
        "status": "skipped_no_duration"
        if duration_ms <= 0
        else "skipped_no_source_activity"
        if not source_activity
        else "completed",
        "audio_duration_ms": duration_ms,
        "suppressed_segments": 0,
        "candidates": [],
    }
    if duration_ms <= 0 or not source_activity:
        return list(final_segments), stats

    kept: List[Dict[str, Any]] = []
    tail_start_ms = max(0, duration_ms - args.unassigned_silence_tail_margin_ms)
    for segment in final_segments:
        start_ms, end_ms = segment_display_window(segment)
        source = str(segment.get("source") or "")
        is_unassigned = (
            segment.get("diarization_status") == "unassigned"
            and int(segment.get("diarization_overlap_ms") or 0) <= 0
        )
        if not is_unassigned or source not in {"mic", "system"} or start_ms < tail_start_ms:
            kept.append(segment)
            continue

        metrics = track_silence_metrics(source_activity, start_ms, end_ms, source)
        if (
            metrics["covered_ratio"] >= args.unassigned_silence_min_covered_ratio
            and metrics["silence_ratio"] >= args.unassigned_silence_ratio
            and metrics["track_rms"] <= args.unassigned_silence_rms_threshold
        ):
            stats["suppressed_segments"] += 1
            stats["candidates"].append(
                {
                    "source": segment.get("source"),
                    "speaker_name": segment.get("speaker_name"),
                    "start_ms": segment.get("start_ms"),
                    "end_ms": segment.get("end_ms"),
                    "text": segment.get("text"),
                    "reason": "unassigned_tail_segment_on_silent_source_activity",
                    **{key: round(value, 6) for key, value in metrics.items()},
                }
            )
            continue
        kept.append(segment)
    return kept, stats


def track_label_parts(source: str) -> Tuple[str, str, str]:
    if source == "mic":
        return "local_speaker", "Local Speaker", "local_participant"
    if source == "system":
        return "remote_speaker", "Remote Speaker", "remote_participant"
    safe = re.sub(r"[^a-z0-9]+", "_", source.lower()).strip("_") or "track"
    return f"{safe}_speaker", f"{source.title()} Speaker", "meeting_participant"


def relabel_diarization_segments(
    segments: Sequence[DiarizationSegment],
    source: str,
) -> Tuple[List[DiarizationSegment], List[Dict[str, Any]]]:
    speaker_id_prefix, speaker_name_prefix, role = track_label_parts(source)
    mapping: Dict[str, Tuple[str, str]] = {}
    relabeled: List[DiarizationSegment] = []
    speakers: List[Dict[str, Any]] = []

    for segment in sorted(segments, key=lambda item: (item.start_ms, item.end_ms)):
        if segment.speaker_id not in mapping:
            idx = len(mapping) + 1
            speaker_id = f"{speaker_id_prefix}_{idx}"
            speaker_name = f"{speaker_name_prefix} {idx}"
            mapping[segment.speaker_id] = (speaker_id, speaker_name)
            speakers.append(
                {
                    "speaker_id": speaker_id,
                    "speaker_name": speaker_name,
                    "source": source,
                    "role": role,
                }
            )
        speaker_id, speaker_name = mapping[segment.speaker_id]
        relabeled.append(
            DiarizationSegment(
                speaker_id=speaker_id,
                speaker_name=speaker_name,
                start_ms=segment.start_ms,
                end_ms=segment.end_ms,
                confidence=segment.confidence,
            )
        )

    return relabeled, speakers


def parse_existing_track_diarization_args(values: Sequence[str]) -> Dict[str, Path]:
    out: Dict[str, Path] = {}
    for value in values:
        if "=" not in value:
            raise ValueError(
                "--existing-track-diarization-json must be SOURCE=PATH, for example mic=/tmp/mic.json"
            )
        source, raw_path = value.split("=", 1)
        source = source.strip()
        path = Path(raw_path.strip()).expanduser()
        if not source or not path.is_file():
            raise ValueError(f"invalid existing track diarization mapping: {value}")
        out[source] = path
    return out


def should_use_trackwise(
    args: argparse.Namespace,
    source_transcript: Dict[str, Any],
    transcript_segments: Sequence[TranscriptSegment],
) -> bool:
    if args.disable_trackwise:
        return False
    if args.force_trackwise:
        return True
    if not isinstance(source_transcript.get("source_wavs"), list):
        return False
    source_wavs = source_wavs_from_transcript(source_transcript, args.transcript_json.parent)
    segment_sources = {
        segment.source
        for segment in transcript_segments
        if segment.source in source_wavs
    }
    return len(segment_sources) >= 2


def aggregate_assignment_stats(track_stats: Iterable[Dict[str, int]]) -> Dict[str, int]:
    totals = {"assigned_by_overlap": 0, "assigned_by_nearest": 0, "unassigned": 0}
    for stats in track_stats:
        for key in totals:
            totals[key] += int(stats.get(key, 0))
    return totals


def write_trackwise_artifacts(
    *,
    args: argparse.Namespace,
    source_transcript: Dict[str, Any],
    transcript_segments: Sequence[TranscriptSegment],
    final_segments: Sequence[Dict[str, Any]],
    diarization_segments: Sequence[Dict[str, Any]],
    speakers: Sequence[Dict[str, Any]],
    tracks: Sequence[Dict[str, Any]],
    assignment_stats: Dict[str, int],
    echo_suppression: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    generated_at = now_ms()
    transcript_text = format_transcript(final_segments)
    text_out = args.transcript_text_out
    if text_out is None and args.transcript_out.name.endswith(".json"):
        text_out = args.transcript_out.with_suffix(".txt")
    if text_out is not None:
        text_out.parent.mkdir(parents=True, exist_ok=True)
        text_out.write_text(transcript_text + "\n", encoding="utf-8")

    diarization_artifact = {
        "schema_version": 1,
        "status": "completed",
        "mode": "trackwise",
        "method": f"{args.provider}_trackwise",
        "source": "trackwise_sortformer",
        "speakers": list(speakers),
        "segments": list(diarization_segments),
        "tracks": list(tracks),
        "generated_at_ms": generated_at,
        "error": None,
    }
    write_json(args.diarization_out, diarization_artifact)

    transcript_artifact = {
        "schema_version": 1,
        "provider": "whisper.cpp+trackwise-sortformer",
        "status": "completed",
        "diarization_mode": "trackwise",
        "diarization_provider": args.provider,
        "source_transcript_json": str(args.transcript_json),
        "diarization_json_path": str(args.diarization_out),
        "transcript_text_path": str(text_out) if text_out is not None else None,
        "transcript": transcript_text,
        "segments": list(final_segments),
        "raw_segment_count": len(transcript_segments),
        "final_segment_count": len(final_segments),
        "text_preserved": len(transcript_segments) == len(final_segments),
        "assignment_stats": assignment_stats,
        "echo_suppression": echo_suppression,
        "source_transcript_status": source_transcript.get("status"),
        "tracks": list(tracks),
        "track_stats": list(tracks),
        "generated_at_ms": generated_at,
        "error": None,
    }
    write_json(args.transcript_out, transcript_artifact)

    return {
        "status": "completed",
        "mode": "trackwise",
        "raw_segments": len(transcript_segments),
        "final_segments": len(final_segments),
        "text_preserved": len(transcript_segments) == len(final_segments),
        "diarization_segments": len(diarization_segments),
        "assignment_stats": assignment_stats,
        "echo_suppression": echo_suppression,
        "tracks": list(tracks),
        "diarization_out": str(args.diarization_out),
        "transcript_out": str(args.transcript_out),
    }


def run_trackwise_finalization(
    args: argparse.Namespace,
    transcript_segments: Sequence[TranscriptSegment],
    source_transcript: Dict[str, Any],
) -> Dict[str, Any]:
    source_wavs = source_wavs_from_transcript(source_transcript, args.transcript_json.parent)
    existing_track_diarization = parse_existing_track_diarization_args(
        args.existing_track_diarization_json or []
    )
    segments_by_source: Dict[str, List[TranscriptSegment]] = {}
    for segment in transcript_segments:
        if segment.source in source_wavs:
            segments_by_source.setdefault(segment.source, []).append(segment)

    if args.force_trackwise:
        for source in source_wavs:
            segments_by_source.setdefault(source, [])
    if len(segments_by_source) < 2:
        raise RuntimeError("trackwise finalization requires at least two transcript sources")

    all_final_segments: List[Dict[str, Any]] = []
    all_diarization_segments: List[Dict[str, Any]] = []
    all_speakers: List[Dict[str, Any]] = []
    all_stats: List[Dict[str, int]] = []
    tracks: List[Dict[str, Any]] = []

    for source in sorted(segments_by_source, key=source_sort_key):
        audio_path = source_wavs.get(source)
        if audio_path is None or not audio_path.is_file():
            raise FileNotFoundError(f"source WAV for {source!r} not found: {audio_path}")
        track_segments = sorted(
            segments_by_source[source],
            key=lambda item: (item.start_ms, item.end_ms),
        )
        if source in existing_track_diarization:
            diarization_segments, diarization_meta = load_diarization_segments(
                existing_track_diarization[source]
            )
            diarization_meta = {
                "provider": "existing_track_diarization_json",
                "source": str(existing_track_diarization[source]),
                "loaded_meta": diarization_meta.get("meta") if isinstance(diarization_meta, dict) else None,
            }
        else:
            diarization_segments, diarization_meta = run_sortformer(audio_path, args)

        relabeled_diarization, speakers = relabel_diarization_segments(
            diarization_segments,
            source,
        )
        final_segments, stats = assign_speakers(
            track_segments,
            relabeled_diarization,
            max_nearest_gap_ms=args.max_nearest_gap_ms,
        )
        all_final_segments.extend(final_segments)
        all_stats.append(stats)
        all_speakers.extend(speakers)
        all_diarization_segments.extend(
            {
                "speaker_id": segment.speaker_id,
                "speaker_name": segment.speaker_name,
                "source": source,
                "start_ms": segment.start_ms,
                "end_ms": segment.end_ms,
                "start": round(segment.start_ms / 1000.0, 3),
                "end": round(segment.end_ms / 1000.0, 3),
                "confidence": segment.confidence,
                "method": f"{args.provider}_trackwise",
            }
            for segment in relabeled_diarization
        )
        tracks.append(
            {
                "source": source,
                "audio": str(audio_path),
                "transcript_segments": len(track_segments),
                "diarization_segments": len(relabeled_diarization),
                "assignment_stats": stats,
                "meta": diarization_meta,
            }
        )

    all_final_segments.sort(
        key=lambda item: (
            int(item.get("display_start_ms") or item["start_ms"]),
            source_sort_key(str(item.get("source") or "")),
            int(item.get("display_end_ms") or item["end_ms"]),
        )
    )
    source_activity, source_activity_path = load_source_activity(
        source_transcript,
        args.transcript_json.parent,
    )
    all_final_segments, echo_suppression = suppress_trackwise_echo(
        all_final_segments,
        source_activity,
        args,
    )
    all_final_segments, out_of_bounds_suppression = suppress_out_of_bounds_unassigned_segments(
        all_final_segments,
        source_transcript,
        args,
    )
    all_final_segments, silent_tail_suppression = suppress_silent_unassigned_tail_segments(
        all_final_segments,
        source_transcript,
        source_activity,
        args,
    )
    echo_suppression["source_activity_path"] = source_activity_path
    echo_suppression["out_of_bounds_suppression"] = out_of_bounds_suppression
    echo_suppression["silent_tail_suppression"] = silent_tail_suppression
    assignment_stats = aggregate_assignment_stats(all_stats)
    return write_trackwise_artifacts(
        args=args,
        source_transcript=source_transcript,
        transcript_segments=transcript_segments,
        final_segments=all_final_segments,
        diarization_segments=all_diarization_segments,
        speakers=all_speakers,
        tracks=tracks,
        assignment_stats=assignment_stats,
        echo_suppression=echo_suppression,
    )


def format_timestamp_ms(ms: int) -> str:
    total_seconds = ms // 1000
    minutes = total_seconds // 60
    seconds = total_seconds % 60
    return f"{minutes:02d}:{seconds:02d}"


def format_transcript(segments: Sequence[Dict[str, Any]]) -> str:
    lines = []
    for segment in segments:
        timestamp_ms = int(segment.get("display_start_ms") or segment["start_ms"])
        lines.append(
            f"[{format_timestamp_ms(timestamp_ms)} {segment['speaker_name']}] {segment['text']}"
        )
    return "\n".join(lines)


def write_artifacts(
    *,
    args: argparse.Namespace,
    transcript_segments: Sequence[TranscriptSegment],
    diarization_segments: Sequence[DiarizationSegment],
    final_segments: Sequence[Dict[str, Any]],
    assignment_stats: Dict[str, int],
    diarization_meta: Dict[str, Any],
    source_transcript: Dict[str, Any],
) -> None:
    generated_at = now_ms()
    diarization_artifact = {
        "schema_version": 1,
        "status": "completed",
        "mode": "single_track",
        "method": args.provider,
        "source": "sortformer",
        "speakers": speakers_from_diarization(diarization_segments),
        "segments": [
            {
                "speaker_id": segment.speaker_id,
                "speaker_name": segment.speaker_name,
                "source": "final_diarization",
                "start_ms": segment.start_ms,
                "end_ms": segment.end_ms,
                "start": round(segment.start_ms / 1000.0, 3),
                "end": round(segment.end_ms / 1000.0, 3),
                "confidence": segment.confidence,
                "method": args.provider,
            }
            for segment in diarization_segments
        ],
        "generated_at_ms": generated_at,
        "meta": diarization_meta,
        "error": None,
    }
    write_json(args.diarization_out, diarization_artifact)

    transcript_text = format_transcript(final_segments)
    text_out = args.transcript_text_out
    if text_out is None and args.transcript_out.name.endswith(".json"):
        text_out = args.transcript_out.with_suffix(".txt")
    if text_out is not None:
        text_out.parent.mkdir(parents=True, exist_ok=True)
        text_out.write_text(transcript_text + "\n", encoding="utf-8")

    transcript_artifact = {
        "schema_version": 1,
        "provider": "whisper.cpp+sortformer",
        "status": "completed",
        "diarization_mode": "single_track",
        "diarization_provider": args.provider,
        "source_transcript_json": str(args.transcript_json),
        "diarization_json_path": str(args.diarization_out),
        "transcript_text_path": str(text_out) if text_out is not None else None,
        "transcript": transcript_text,
        "segments": list(final_segments),
        "raw_segment_count": len(transcript_segments),
        "final_segment_count": len(final_segments),
        "text_preserved": len(transcript_segments) == len(final_segments),
        "assignment_stats": assignment_stats,
        "source_transcript_status": source_transcript.get("status"),
        "generated_at_ms": generated_at,
        "error": None,
    }
    write_json(args.transcript_out, transcript_artifact)


def active_label_at(segments: Sequence[Tuple[int, int, str]], start_ms: int, end_ms: int) -> Optional[str]:
    best_label = None
    best_overlap = 0
    for seg_start, seg_end, label in segments:
        amount = overlap_ms(start_ms, end_ms, seg_start, seg_end)
        if amount > best_overlap:
            best_overlap = amount
            best_label = label
    return best_label


def load_reference_segments(path: Path) -> List[Tuple[int, int, str]]:
    data = read_json(path)
    out: List[Tuple[int, int, str]] = []
    if not isinstance(data, list):
        raise ValueError(f"reference segments must be a list: {path}")
    for item in data:
        label = str(item.get("speaker") or item.get("speaker_id") or "")
        if not label:
            continue
        start_ms = ms_from_value(item.get("start_ms", item.get("start")))
        end_ms = max(ms_from_value(item.get("end_ms", item.get("end"))), start_ms)
        out.append((start_ms, end_ms, label))
    return out


def score_speaker_assignment(
    reference: Sequence[Tuple[int, int, str]],
    predicted: Sequence[Dict[str, Any]],
    frame_ms: int,
) -> Dict[str, Any]:
    pred_segments = [
        (int(item["start_ms"]), int(item["end_ms"]), str(item["speaker_id"]))
        for item in predicted
        if item.get("speaker_id") != "unknown_speaker"
    ]
    max_end = max(
        [end for _start, end, _label in reference]
        + [end for _start, end, _label in pred_segments]
        + [0]
    )

    ref_labels = sorted({label for _start, _end, label in reference})
    pred_labels = sorted({label for _start, _end, label in pred_segments})
    overlap_counts: Dict[Tuple[str, str], int] = {
        (ref, pred): 0 for ref in ref_labels for pred in pred_labels
    }

    ref_frames = 0
    pred_frames = 0
    miss = 0
    false_alarm = 0
    ref_with_pred = 0
    for start in range(0, max_end + frame_ms, frame_ms):
        end = start + frame_ms
        ref = active_label_at(reference, start, end)
        pred = active_label_at(pred_segments, start, end)
        if ref is not None:
            ref_frames += 1
            if pred is None:
                miss += 1
            else:
                ref_with_pred += 1
                overlap_counts[(ref, pred)] = overlap_counts.get((ref, pred), 0) + 1
        elif pred is not None:
            false_alarm += 1
            pred_frames += 1
        if pred is not None and ref is not None:
            pred_frames += 1

    best_correct = 0
    best_mapping: Dict[str, str] = {}
    if ref_labels and pred_labels:
        if len(pred_labels) <= 8 and len(ref_labels) <= 8:
            for perm in itertools.permutations(ref_labels, min(len(pred_labels), len(ref_labels))):
                mapping = dict(zip(pred_labels, perm))
                correct = sum(overlap_counts.get((ref, pred), 0) for pred, ref in mapping.items())
                if correct > best_correct:
                    best_correct = correct
                    best_mapping = mapping
        else:
            remaining_refs = set(ref_labels)
            for pred in pred_labels:
                ref = max(
                    remaining_refs or ref_labels,
                    key=lambda label: overlap_counts.get((label, pred), 0),
                )
                best_mapping[pred] = ref
                remaining_refs.discard(ref)
            best_correct = sum(overlap_counts.get((ref, pred), 0) for pred, ref in best_mapping.items())

    confusion = max(0, ref_with_pred - best_correct)
    ref_den = max(ref_frames, 1)
    return {
        "frame_ms": frame_ms,
        "reference_speech_frames": ref_frames,
        "predicted_speech_frames": pred_frames,
        "matched_correct_frames": best_correct,
        "miss_frames": miss,
        "confusion_frames": confusion,
        "false_alarm_frames": false_alarm,
        "speaker_accuracy_ref": round(best_correct / ref_den, 4),
        "coverage_ref": round(ref_with_pred / ref_den, 4),
        "miss_ref": round(miss / ref_den, 4),
        "confusion_ref": round(confusion / ref_den, 4),
        "false_alarm_ref": round(false_alarm / ref_den, 4),
        "der_like_ref": round((miss + confusion + false_alarm) / ref_den, 4),
        "speaker_mapping": best_mapping,
        "reference_speakers": ref_labels,
        "predicted_speakers": pred_labels,
    }


def parse_args(argv: Optional[Sequence[str]] = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audio", type=Path, required=True)
    parser.add_argument("--transcript-json", type=Path, required=True)
    parser.add_argument("--diarization-out", type=Path, required=True)
    parser.add_argument("--transcript-out", type=Path, required=True)
    parser.add_argument("--transcript-text-out", type=Path)
    parser.add_argument("--provider", default="nemo_sortformer_v2.1")
    parser.add_argument("--existing-diarization-json", type=Path)
    parser.add_argument(
        "--existing-track-diarization-json",
        action="append",
        default=[],
        help="Test hook for track-wise mode, format SOURCE=PATH. Repeat for mic/system.",
    )
    parser.add_argument("--force-trackwise", action="store_true")
    parser.add_argument("--disable-trackwise", action="store_true")
    parser.add_argument("--disable-echo-suppression", action="store_true")
    parser.add_argument(
        "--echo-activity-overlap-ratio",
        type=float,
        default=float(os.environ.get("AIRNOTE_FINAL_ECHO_ACTIVITY_OVERLAP_RATIO", "0.45")),
    )
    parser.add_argument(
        "--echo-system-rms-ratio",
        type=float,
        default=float(os.environ.get("AIRNOTE_FINAL_ECHO_SYSTEM_RMS_RATIO", "4.0")),
    )
    parser.add_argument(
        "--echo-max-pair-gap-ms",
        type=int,
        default=int(os.environ.get("AIRNOTE_FINAL_ECHO_MAX_PAIR_GAP_MS", "400")),
    )
    parser.add_argument(
        "--echo-text-similarity-threshold",
        type=float,
        default=float(os.environ.get("AIRNOTE_FINAL_ECHO_TEXT_SIMILARITY_THRESHOLD", "0.90")),
    )
    parser.add_argument(
        "--echo-text-containment-threshold",
        type=float,
        default=float(os.environ.get("AIRNOTE_FINAL_ECHO_TEXT_CONTAINMENT_THRESHOLD", "0.60")),
    )
    parser.add_argument(
        "--echo-min-token-overlap",
        type=int,
        default=int(os.environ.get("AIRNOTE_FINAL_ECHO_MIN_TOKEN_OVERLAP", "3")),
    )
    parser.add_argument(
        "--echo-drift-margin-ms",
        type=int,
        default=int(os.environ.get("AIRNOTE_FINAL_ECHO_DRIFT_MARGIN_MS", "1500")),
    )
    parser.add_argument(
        "--echo-allow-mic-text",
        action="store_true",
        default=os.environ.get("AIRNOTE_FINAL_ECHO_ALLOW_MIC_TEXT", "").strip().lower()
        in {"1", "true", "yes", "on"},
    )
    parser.add_argument(
        "--unassigned-out-of-bounds-margin-ms",
        type=int,
        default=int(os.environ.get("AIRNOTE_FINAL_UNASSIGNED_OOB_MARGIN_MS", "2000")),
    )
    parser.add_argument(
        "--unassigned-silence-tail-margin-ms",
        type=int,
        default=int(os.environ.get("AIRNOTE_FINAL_UNASSIGNED_SILENCE_TAIL_MARGIN_MS", "2500")),
    )
    parser.add_argument(
        "--unassigned-silence-min-covered-ratio",
        type=float,
        default=float(os.environ.get("AIRNOTE_FINAL_UNASSIGNED_SILENCE_MIN_COVERED_RATIO", "0.90")),
    )
    parser.add_argument(
        "--unassigned-silence-ratio",
        type=float,
        default=float(os.environ.get("AIRNOTE_FINAL_UNASSIGNED_SILENCE_RATIO", "0.80")),
    )
    parser.add_argument(
        "--unassigned-silence-rms-threshold",
        type=float,
        default=float(os.environ.get("AIRNOTE_FINAL_UNASSIGNED_SILENCE_RMS_THRESHOLD", "0.01")),
    )
    parser.add_argument("--model-path")
    parser.add_argument("--model-cache")
    parser.add_argument(
        "--model-repo",
        default=os.environ.get("AIRNOTE_NEMO_SORTFORMER_REPO", DEFAULT_SORTFORMER_REPO),
    )
    parser.add_argument(
        "--model-filename",
        default=os.environ.get("AIRNOTE_NEMO_SORTFORMER_FILENAME", DEFAULT_SORTFORMER_FILE),
    )
    parser.add_argument("--device", default=os.environ.get("AIRNOTE_NEMO_DEVICE", ""))
    parser.add_argument("--batch-size", type=int, default=1)
    parser.add_argument(
        "--max-nearest-gap-ms",
        type=int,
        default=int(os.environ.get("AIRNOTE_FINAL_DIARIZATION_MAX_NEAREST_GAP_MS", "750")),
    )
    parser.add_argument("--reference-segments", type=Path)
    parser.add_argument("--metrics-out", type=Path)
    parser.add_argument("--score-frame-ms", type=int, default=100)
    return parser.parse_args(argv)


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = parse_args(argv)
    if not args.audio.is_file():
        raise FileNotFoundError(f"audio file not found: {args.audio}")
    if not args.transcript_json.is_file():
        raise FileNotFoundError(f"transcript JSON not found: {args.transcript_json}")

    transcript_segments, source_transcript = load_transcript_segments(args.transcript_json)
    if should_use_trackwise(args, source_transcript, transcript_segments):
        result = run_trackwise_finalization(args, transcript_segments, source_transcript)
        if args.reference_segments:
            reference = load_reference_segments(args.reference_segments)
            metrics = score_speaker_assignment(
                reference,
                read_json(args.transcript_out).get("segments", []),
                args.score_frame_ms,
            )
            result["metrics"] = metrics
            if args.metrics_out:
                write_json(args.metrics_out, {**result, "generated_at_ms": now_ms()})
        print(json.dumps(result, indent=2, ensure_ascii=False))
        return 0

    if args.existing_diarization_json:
        diarization_segments, diarization_meta = load_diarization_segments(args.existing_diarization_json)
        diarization_meta = {
            "provider": "existing_diarization_json",
            "source": str(args.existing_diarization_json),
            "loaded_meta": diarization_meta.get("meta") if isinstance(diarization_meta, dict) else None,
        }
    else:
        diarization_segments, diarization_meta = run_sortformer(args.audio, args)

    single_track_source = source_for_wav(args.audio)
    if single_track_source in {"mic", "system"}:
        diarization_segments, _speakers = relabel_diarization_segments(
            diarization_segments,
            single_track_source,
        )
        diarization_meta = {
            **diarization_meta,
            "single_track_source": single_track_source,
            "speaker_label_source": single_track_source,
        }

    final_segments, assignment_stats = assign_speakers(
        transcript_segments,
        diarization_segments,
        max_nearest_gap_ms=args.max_nearest_gap_ms,
    )
    write_artifacts(
        args=args,
        transcript_segments=transcript_segments,
        diarization_segments=diarization_segments,
        final_segments=final_segments,
        assignment_stats=assignment_stats,
        diarization_meta=diarization_meta,
        source_transcript=source_transcript if isinstance(source_transcript, dict) else {},
    )

    result: Dict[str, Any] = {
        "status": "completed",
        "raw_segments": len(transcript_segments),
        "final_segments": len(final_segments),
        "text_preserved": len(transcript_segments) == len(final_segments),
        "diarization_segments": len(diarization_segments),
        "assignment_stats": assignment_stats,
        "diarization_out": str(args.diarization_out),
        "transcript_out": str(args.transcript_out),
    }
    if args.reference_segments:
        reference = load_reference_segments(args.reference_segments)
        metrics = score_speaker_assignment(reference, final_segments, args.score_frame_ms)
        result["metrics"] = metrics
        if args.metrics_out:
            write_json(args.metrics_out, {**result, "generated_at_ms": now_ms()})

    print(json.dumps(result, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    try:
        exit_code = main()
        sys.stdout.flush()
        sys.stderr.flush()
        os._exit(exit_code)
    except Exception as exc:
        print(f"sortformer_diarize failed: {exc}", file=sys.stderr)
        sys.stderr.flush()
        os._exit(1)
