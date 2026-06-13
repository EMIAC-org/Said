import { useCallback, useEffect, useRef, useState } from "react";
import { getRecordingAudioBytes } from "@/lib/invoke";

/**
 * Shared audio-playback hook for recording rows. Returns the currently-playing
 * recording id (null when idle), a `play(id, audioId)` toggler that pauses
 * if the same id is already playing, and a `stop()` for hard-stops.
 */
export function useAudioPlayer() {
  const audioRef                  = useRef<HTMLAudioElement | null>(null);
  const blobUrlRef                = useRef<string | null>(null);
  const [playingId, setPlayingId] = useState<string | null>(null);

  // Cleanup on unmount
  useEffect(() => () => {
    audioRef.current?.pause();
    audioRef.current = null;
    if (blobUrlRef.current) {
      URL.revokeObjectURL(blobUrlRef.current);
      blobUrlRef.current = null;
    }
  }, []);

  const play = useCallback(async (recordingId: string, _audioId?: string | null) => {
    // Toggle off if the same row is already playing
    if (playingId === recordingId) {
      audioRef.current?.pause();
      audioRef.current = null;
      setPlayingId(null);
      if (blobUrlRef.current) {
        URL.revokeObjectURL(blobUrlRef.current);
        blobUrlRef.current = null;
      }
      return;
    }

    // Stop any other in-progress playback
    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current = null;
    }
    if (blobUrlRef.current) {
      URL.revokeObjectURL(blobUrlRef.current);
      blobUrlRef.current = null;
    }

    try {
      const bytes = await getRecordingAudioBytes(recordingId);
      if (!bytes) return;

      // TS 5.7+ tightened Uint8Array generics; cast to BlobPart since the
      // runtime value is already a valid blob source.
      const blob  = new Blob([bytes as BlobPart], { type: "audio/wav" });
      const url   = URL.createObjectURL(blob);
      const audio = new Audio(url);
      audioRef.current   = audio;
      blobUrlRef.current = url;
      audio.onended = () => {
        setPlayingId(null);
        if (blobUrlRef.current) {
          URL.revokeObjectURL(blobUrlRef.current);
          blobUrlRef.current = null;
        }
        audioRef.current = null;
      };
      await audio.play();
      setPlayingId(recordingId);
    } catch {
      setPlayingId(null);
      audioRef.current = null;
      if (blobUrlRef.current) {
        URL.revokeObjectURL(blobUrlRef.current);
        blobUrlRef.current = null;
      }
    }
  }, [playingId]);

  const stop = useCallback(() => {
    audioRef.current?.pause();
    audioRef.current = null;
    if (blobUrlRef.current) {
      URL.revokeObjectURL(blobUrlRef.current);
      blobUrlRef.current = null;
    }
    setPlayingId(null);
  }, []);

  return { playingId, play, stop };
}
