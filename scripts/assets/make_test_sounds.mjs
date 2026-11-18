// Writes the sandbox's test sounds: tiny sine tones as 8 kHz 16-bit mono WAV files.
// They are generated, so they are public domain (CC0); see sandbox/assets/CREDITS.md.
// Run from Retreat-Engine-3D: node scripts/assets/make_test_sounds.mjs
import { mkdirSync, writeFileSync } from "node:fs";

const SAMPLE_RATE = 8000;
const FADE_SECONDS = 0.01;

function wavBytes(frequencyHz, seconds, amplitude) {
  const sampleCount = Math.round(seconds * SAMPLE_RATE);
  const dataBytes = sampleCount * 2;
  const buffer = Buffer.alloc(44 + dataBytes);
  buffer.write("RIFF", 0);
  buffer.writeUInt32LE(36 + dataBytes, 4);
  buffer.write("WAVEfmt ", 8);
  buffer.writeUInt32LE(16, 16); // PCM header size
  buffer.writeUInt16LE(1, 20); // PCM
  buffer.writeUInt16LE(1, 22); // mono
  buffer.writeUInt32LE(SAMPLE_RATE, 24);
  buffer.writeUInt32LE(SAMPLE_RATE * 2, 28); // bytes per second
  buffer.writeUInt16LE(2, 32); // bytes per frame
  buffer.writeUInt16LE(16, 34); // bits per sample
  buffer.write("data", 36);
  buffer.writeUInt32LE(dataBytes, 40);
  for (let i = 0; i < sampleCount; i++) {
    const t = i / SAMPLE_RATE;
    const fade = Math.min(1, t / FADE_SECONDS, (seconds - t) / FADE_SECONDS);
    const sample = Math.sin(2 * Math.PI * frequencyHz * t) * amplitude * fade;
    buffer.writeInt16LE(Math.round(sample * 32767), 44 + i * 2);
  }
  return buffer;
}

mkdirSync("sandbox/assets/sounds", { recursive: true });
writeFileSync("sandbox/assets/sounds/beep.wav", wavBytes(880, 0.25, 0.3)); // 0.25 s effect
writeFileSync("sandbox/assets/sounds/tone.wav", wavBytes(220, 1.0, 0.3)); // 1 s loopable track
console.log("wrote sandbox/assets/sounds/beep.wav and tone.wav");
