import { useRef, useState, useCallback } from "react";

interface RecorderState {
  isRecording: boolean;
  duration: number;
  audioBlob: Blob | null;
  audioUrl: string | null;
  error: string | null;
}

export function useAudioRecorder() {
  const [state, setState] = useState<RecorderState>({
    isRecording: false,
    duration: 0,
    audioBlob: null,
    audioUrl: null,
    error: null,
  });

  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const timerRef = useRef<number | null>(null);
  const startTimeRef = useRef<number>(0);

  const startRecording = useCallback(async () => {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: { sampleRate: 16000, channelCount: 1, echoCancellation: true },
      });

      const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
        ? "audio/webm;codecs=opus"
        : MediaRecorder.isTypeSupported("audio/webm")
          ? "audio/webm"
          : "";

      const recorder = new MediaRecorder(stream, mimeType ? { mimeType } : {});
      mediaRecorderRef.current = recorder;
      chunksRef.current = [];

      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) chunksRef.current.push(e.data);
      };

      recorder.onstop = async () => {
        stream.getTracks().forEach((t) => t.stop());

        const webmBlob = new Blob(chunksRef.current, {
          type: recorder.mimeType || "audio/webm",
        });

        try {
          const wavBlob = await convertToWav(webmBlob);
          const url = URL.createObjectURL(wavBlob);
          setState((prev) => ({
            ...prev,
            isRecording: false,
            audioBlob: wavBlob,
            audioUrl: url,
          }));
        } catch {
          const url = URL.createObjectURL(webmBlob);
          setState((prev) => ({
            ...prev,
            isRecording: false,
            audioBlob: webmBlob,
            audioUrl: url,
          }));
        }
      };

      startTimeRef.current = Date.now();
      recorder.start(100);

      timerRef.current = window.setInterval(() => {
        setState((prev) => ({
          ...prev,
          duration: (Date.now() - startTimeRef.current) / 1000,
        }));
      }, 100);

      setState({
        isRecording: true,
        duration: 0,
        audioBlob: null,
        audioUrl: null,
        error: null,
      });
    } catch (err) {
      setState((prev) => ({
        ...prev,
        error:
          err instanceof Error ? err.message : "Failed to access microphone",
      }));
    }
  }, []);

  const stopRecording = useCallback(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
    if (
      mediaRecorderRef.current &&
      mediaRecorderRef.current.state !== "inactive"
    ) {
      mediaRecorderRef.current.stop();
    }
  }, []);

  return { ...state, startRecording, stopRecording };
}

async function convertToWav(blob: Blob): Promise<Blob> {
  const audioCtx = new AudioContext({ sampleRate: 16000 });
  const arrayBuf = await blob.arrayBuffer();
  const decoded = await audioCtx.decodeAudioData(arrayBuf);
  audioCtx.close();

  // Take first channel, resample to 16kHz
  const srcData = decoded.getChannelData(0);
  const srcRate = decoded.sampleRate;
  const ratio = 16000 / srcRate;
  const dstLen = Math.round(srcData.length * ratio);
  const dstData = new Float32Array(dstLen);

  for (let i = 0; i < dstLen; i++) {
    const srcIdx = i / ratio;
    const lo = Math.floor(srcIdx);
    const hi = Math.min(lo + 1, srcData.length - 1);
    const frac = srcIdx - lo;
    dstData[i] = srcData[lo] * (1 - frac) + srcData[hi] * frac;
  }

  // Float32 -> Int16
  const pcm = new Int16Array(dstLen);
  for (let i = 0; i < dstLen; i++) {
    const s = Math.max(-1, Math.min(1, dstData[i]));
    pcm[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
  }

  // Build WAV
  const dataSize = pcm.length * 2;
  const buf = new ArrayBuffer(44 + dataSize);
  const view = new DataView(buf);

  writeStr(view, 0, "RIFF");
  view.setUint32(4, 36 + dataSize, true);
  writeStr(view, 8, "WAVE");
  writeStr(view, 12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);           // PCM
  view.setUint16(22, 1, true);           // mono
  view.setUint32(24, 16000, true);       // sample rate
  view.setUint32(28, 16000 * 2, true);   // byte rate
  view.setUint16(32, 2, true);           // block align
  view.setUint16(34, 16, true);          // bits per sample
  writeStr(view, 36, "data");
  view.setUint32(40, dataSize, true);

  const pcmBytes = new Uint8Array(buf, 44);
  pcmBytes.set(new Uint8Array(pcm.buffer));

  return new Blob([buf], { type: "audio/wav" });
}

function writeStr(view: DataView, offset: number, str: string) {
  for (let i = 0; i < str.length; i++) {
    view.setUint8(offset + i, str.charCodeAt(i));
  }
}
