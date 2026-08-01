// Single owner of local speech-model download transport and progress.
//
// Every per-model difference (which Tauri command installs it, which command
// cancels it, which progress event carries it) lives in `localModelTransport`
// and `downloadOwner`. Components render models; they never branch on a model
// key. Adding a catalog model must not require editing a component.
//
// Progress is keyed by model, so a download of one model can never render as
// progress on another — the whole reason this module exists.

import { useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke, type LocalModelKey } from "./invoke";

/** Progress event emitted by the verified catalog installer. */
const CATALOG_EVENT = "local-model-download";
/** Progress event emitted by the legacy meeting downloader (Oriserve + VAD). */
const MEETING_EVENT = "meeting-model-download";
/** The only meeting-downloader artifact that is also a dictation model. */
const ORISERVE_ARTIFACT = "ggml-oriserve-hinglish-fp16.bin";

const ACTIVE_STATUSES = ["downloading", "verifying", "retrying"] as const;

export type LocalModelDownloadStatus =
  | (typeof ACTIVE_STATUSES)[number]
  | "done"
  | "cancelled"
  | "error";

/** Raw payload shape shared by both progress events. */
interface RawDownloadProgress {
  model?: LocalModelKey | null;
  name: string;
  received: number;
  total: number;
  status: LocalModelDownloadStatus | string;
  error: string | null;
}

export interface LocalModelDownload {
  model: LocalModelKey;
  status: LocalModelDownloadStatus | string;
  received: number;
  total: number;
  /** `null` until the server reports a content length. */
  percent: number | null;
  error: string | null;
}

export type LocalModelDownloads = Partial<Record<LocalModelKey, LocalModelDownload>>;

interface LocalModelTransport {
  download: { command: string; args?: Record<string, unknown> };
  cancel: { command: string; args?: Record<string, unknown> };
}

/**
 * Oriserve is still installed by the meeting downloader because Meetings own
 * that whisper artifact; catalog models use the checksum-verified installer.
 * This function is the only place that distinction is allowed to exist.
 */
export function localModelTransport(model: LocalModelKey): LocalModelTransport {
  if (model === "oriserve") {
    return {
      download: { command: "download_dictation_model" },
      cancel: { command: "meeting_cancel_model_download", args: { name: ORISERVE_ARTIFACT } },
    };
  }
  return {
    download: { command: "download_local_model", args: { model } },
    cancel: { command: "cancel_local_model_download", args: { model } },
  };
}

/**
 * The model a progress event belongs to, or `null` for artifacts that are not
 * selectable dictation models (the meeting downloader also reports Silero VAD).
 */
export function downloadOwner(progress: RawDownloadProgress): LocalModelKey | null {
  if (progress.model) return progress.model;
  return progress.name === ORISERVE_ARTIFACT ? "oriserve" : null;
}

/** Resolves when the model is installed and verified; rejects on cancel. */
export async function startLocalModelDownload(model: LocalModelKey): Promise<void> {
  const { download } = localModelTransport(model);
  await invoke(download.command, download.args);
}

/** Best effort: a cancel that loses its race is not worth an error banner. */
export async function cancelLocalModelDownload(model: LocalModelKey): Promise<void> {
  const { cancel } = localModelTransport(model);
  await invoke(cancel.command, cancel.args).catch(() => {});
}

/** A rejected download that the user cancelled themselves is not an error. */
export function isCancelledDownload(message: string): boolean {
  return /cancell?ed/i.test(message);
}

function toDownload(progress: RawDownloadProgress, model: LocalModelKey): LocalModelDownload {
  return {
    model,
    status: progress.status,
    received: progress.received,
    total: progress.total,
    percent:
      progress.total > 0
        ? Math.min(100, Math.round((progress.received / progress.total) * 100))
        : null,
    error: progress.error,
  };
}

/**
 * Live download progress for every local speech model, keyed by model.
 *
 * Handlers are read through a ref so subscribing happens once: callers can pass
 * inline closures without resubscribing (and dropping events) on every render.
 */
export function useLocalModelDownloads(handlers?: {
  onDone?: (model: LocalModelKey) => void;
  onError?: (model: LocalModelKey, message: string) => void;
}): LocalModelDownloads {
  const [downloads, setDownloads] = useState<LocalModelDownloads>({});
  const latest = useRef(handlers);
  latest.current = handlers;

  useEffect(() => {
    let disposed = false;
    const stops: UnlistenFn[] = [];

    const handle = (progress: RawDownloadProgress) => {
      const model = downloadOwner(progress);
      if (!model) return;
      const active = (ACTIVE_STATUSES as readonly string[]).includes(progress.status);
      setDownloads((current) => {
        if (active) return { ...current, [model]: toDownload(progress, model) };
        if (!current[model]) return current;
        const next = { ...current };
        delete next[model];
        return next;
      });
      if (progress.status === "done") latest.current?.onDone?.(model);
      if (progress.status === "error" && progress.error) {
        latest.current?.onError?.(model, progress.error);
      }
    };

    for (const event of [CATALOG_EVENT, MEETING_EVENT]) {
      void listen<RawDownloadProgress>(event, (received) => handle(received.payload))
        .then((stop) => {
          if (disposed) {
            stop();
            return;
          }
          stops.push(stop);
        })
        .catch((cause) => console.warn(`[local-models] ${event} subscribe failed`, cause));
    }

    return () => {
      disposed = true;
      for (const stop of stops) stop();
    };
  }, []);

  return downloads;
}
