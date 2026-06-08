#!/usr/bin/env node

import fs from "node:fs";
import crypto from "node:crypto";

const DEFAULT_TIMEOUT_MS = 45_000;
const DEFAULT_FRAME_MS = 100;

function usage() {
  console.log(`Usage:
node tools/runtime-gateway/ws-smoke.mjs --url https://airnote.emiactech.com --token <token> --wav sample.wav [options]

Options:
  --url <url>          Server base URL or full WS URL.
  --token <token>      AirNote auth WS token.
  --wav <path>         PCM WAV file to stream.
  --model <fast|smart> Selected model metadata. Default: fast.
  --language <name>    Output language metadata. Default: hinglish.
  --frame-ms <n>       Audio frame size in ms. Default: 100.
  --timeout-ms <n>     Total timeout. Default: 45000.
  --no-realtime        Send frames as fast as possible.
  --help               Show this help.
`);
}

function parseArgs(argv) {
  const args = {
    model: "fast",
    language: "hinglish",
    frameMs: DEFAULT_FRAME_MS,
    timeoutMs: DEFAULT_TIMEOUT_MS,
    realtime: true,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    switch (arg) {
      case "--help":
      case "-h":
        args.help = true;
        break;
      case "--url":
        args.url = argv[++i];
        break;
      case "--token":
        args.token = argv[++i];
        break;
      case "--wav":
        args.wav = argv[++i];
        break;
      case "--model":
        args.model = argv[++i];
        break;
      case "--language":
        args.language = argv[++i];
        break;
      case "--frame-ms":
        args.frameMs = Number(argv[++i]);
        break;
      case "--timeout-ms":
        args.timeoutMs = Number(argv[++i]);
        break;
      case "--no-realtime":
        args.realtime = false;
        break;
      default:
        throw new Error(`unknown argument: ${arg}`);
    }
  }

  return args;
}

function readFourCc(buffer, offset) {
  return buffer.toString("ascii", offset, offset + 4);
}

function parseWav(file) {
  const bytes = fs.readFileSync(file);
  if (bytes.length < 44 || readFourCc(bytes, 0) !== "RIFF" || readFourCc(bytes, 8) !== "WAVE") {
    throw new Error("input is not a RIFF/WAVE file");
  }

  let offset = 12;
  let fmt = null;
  let data = null;

  while (offset + 8 <= bytes.length) {
    const chunkId = readFourCc(bytes, offset);
    const chunkSize = bytes.readUInt32LE(offset + 4);
    const start = offset + 8;
    const end = start + chunkSize;

    if (end > bytes.length) {
      throw new Error(`invalid WAV chunk ${chunkId}: extends past EOF`);
    }

    if (chunkId === "fmt ") {
      if (chunkSize < 16) {
        throw new Error("invalid fmt chunk");
      }
      fmt = {
        audioFormat: bytes.readUInt16LE(start),
        channels: bytes.readUInt16LE(start + 2),
        sampleRate: bytes.readUInt32LE(start + 4),
        byteRate: bytes.readUInt32LE(start + 8),
        blockAlign: bytes.readUInt16LE(start + 12),
        bitsPerSample: bytes.readUInt16LE(start + 14),
      };
    } else if (chunkId === "data") {
      data = bytes.subarray(start, end);
    }

    offset = end + (chunkSize % 2);
  }

  if (!fmt) throw new Error("WAV missing fmt chunk");
  if (!data) throw new Error("WAV missing data chunk");
  if (fmt.audioFormat !== 1) throw new Error(`only PCM WAV is supported, got format ${fmt.audioFormat}`);
  if (fmt.bitsPerSample !== 16) throw new Error(`only 16-bit PCM WAV is supported, got ${fmt.bitsPerSample}`);
  if (fmt.channels !== 1 && fmt.channels !== 2) {
    throw new Error(`only mono/stereo WAV is supported, got ${fmt.channels} channels`);
  }

  let pcm = data;
  if (fmt.channels === 2) {
    pcm = downmixStereo16ToMono(data);
    fmt.channels = 1;
    fmt.blockAlign = 2;
    fmt.byteRate = fmt.sampleRate * 2;
  }

  return { pcm, sampleRate: fmt.sampleRate, channels: fmt.channels };
}

function downmixStereo16ToMono(data) {
  if (data.length % 4 !== 0) {
    throw new Error("invalid stereo 16-bit PCM byte length");
  }
  const out = Buffer.alloc(data.length / 2);
  for (let i = 0, j = 0; i < data.length; i += 4, j += 2) {
    const left = data.readInt16LE(i);
    const right = data.readInt16LE(i + 2);
    const mixed = Math.max(-32768, Math.min(32767, Math.round((left + right) / 2)));
    out.writeInt16LE(mixed, j);
  }
  return out;
}

function buildWsUrl(rawUrl, token) {
  let url = rawUrl;
  if (!url.startsWith("ws://") && !url.startsWith("wss://")) {
    url = url.replace(/^http:\/\//, "ws://").replace(/^https:\/\//, "wss://");
    url = `${url.replace(/\/$/, "")}/v1/runtime/voice/ws`;
  }
  const parsed = new URL(url);
  parsed.searchParams.set("token", token);
  return parsed.toString();
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function sendJson(ws, payload) {
  ws.send(JSON.stringify(payload));
}

async function streamAudio(ws, pcm, sampleRate, frameMs, realtime) {
  const bytesPerSample = 2;
  const frameBytes = Math.max(bytesPerSample, Math.floor(sampleRate * bytesPerSample * frameMs / 1000));
  for (let offset = 0; offset < pcm.length; offset += frameBytes) {
    const chunk = pcm.subarray(offset, Math.min(offset + frameBytes, pcm.length));
    ws.send(chunk);
    if (realtime) {
      await sleep(frameMs);
    }
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    usage();
    return;
  }
  if (!args.url || !args.token || !args.wav) {
    usage();
    process.exitCode = 2;
    return;
  }
  if (!Number.isFinite(args.frameMs) || args.frameMs <= 0) {
    throw new Error("--frame-ms must be a positive number");
  }
  if (!Number.isFinite(args.timeoutMs) || args.timeoutMs <= 0) {
    throw new Error("--timeout-ms must be a positive number");
  }

  const { pcm, sampleRate, channels } = parseWav(args.wav);
  const wsUrl = buildWsUrl(args.url, args.token);
  const runId = crypto.randomUUID();
  const startedAt = Date.now();

  console.error(`[runtime-ws-smoke] connecting ${wsUrl.replace(/token=[^&]+/, "token=<redacted>")}`);
  console.error(`[runtime-ws-smoke] wav=${args.wav} bytes=${pcm.length} sample_rate=${sampleRate} channels=${channels}`);

  const ws = new WebSocket(wsUrl);
  let done = false;
  let errorSeen = false;

  const timeout = setTimeout(() => {
    if (!done) {
      errorSeen = true;
      console.error(`[runtime-ws-smoke] timeout after ${args.timeoutMs}ms`);
      ws.close();
    }
  }, args.timeoutMs);

  await new Promise((resolve, reject) => {
    ws.addEventListener("open", async () => {
      try {
        sendJson(ws, {
          type: "voice.start",
          run_id: runId,
          mode: "normal_voice",
          selected_model: args.model,
          output_language: args.language,
          source: "ws_smoke",
          platform: process.platform,
          app_version: "ws-smoke",
          audio: {
            encoding: "linear16",
            sample_rate: sampleRate,
            channels,
          },
        });
        await streamAudio(ws, pcm, sampleRate, args.frameMs, args.realtime);
        sendJson(ws, { type: "audio.end", run_id: runId });
      } catch (err) {
        reject(err);
      }
    });

    ws.addEventListener("message", (event) => {
      const text = typeof event.data === "string" ? event.data : Buffer.from(event.data).toString("utf8");
      console.log(text);
      try {
        const msg = JSON.parse(text);
        if (msg.type === "runtime.error") {
          errorSeen = true;
        }
        if (msg.type === "runtime.done") {
          done = true;
          ws.close();
        }
      } catch {
        // Keep printing non-JSON frames for debugging.
      }
    });

    ws.addEventListener("error", (event) => {
      errorSeen = true;
      reject(new Error(`websocket error: ${event.message || "unknown"}`));
    });

    ws.addEventListener("close", () => {
      clearTimeout(timeout);
      resolve();
    });
  });

  const elapsedMs = Date.now() - startedAt;
  console.error(`[runtime-ws-smoke] finished elapsed_ms=${elapsedMs} done=${done} error=${errorSeen}`);
  if (!done || errorSeen) {
    process.exitCode = 1;
  }
}

main().catch((err) => {
  console.error(`[runtime-ws-smoke] ${err.message}`);
  process.exitCode = 1;
});
