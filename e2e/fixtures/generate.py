#!/usr/bin/env python3
"""Generate the pt-BR WAV fixtures used by E2E (T-009) and the text pipeline
(T-035).

Pure stdlib (wave/struct/math/random) so it runs anywhere:

    python e2e/fixtures/generate.py            # writes into ./pt-br
    python e2e/fixtures/generate.py --out DIR  # custom output dir

The output is **synthetic speech-like audio**, not real speech: each fixture is
a sequence of voiced syllable-like bursts (a harmonic stack shaped by two
formant-ish resonances under a syllable envelope), word gaps, sentence pauses,
and — for the "muletas" fixture — hesitation drones ("ahn/ééé"-like sustained
voiced segments) and long pauses. It exercises timing, VAD gating, the capture
ring, and the transcription pipeline end-to-end, but produces no meaningful
transcript. When recorded pt-BR speech is available, drop the real WAVs in
under the same filenames (16 kHz mono PCM16) and update the README.

Deterministic: fixed seeds, so regenerating reproduces identical files.
"""

import argparse
import math
import random
import struct
import wave
from pathlib import Path

SAMPLE_RATE = 16_000
AMPLITUDE = 0.45  # peak, comfortably below clipping


def _syllable(rng: random.Random, duration_ms: float) -> list[float]:
    """One voiced syllable: f0 with slight vibrato, ~8 harmonics weighted by two
    random formant bumps, attack/decay envelope."""
    n = int(SAMPLE_RATE * duration_ms / 1000)
    f0 = rng.uniform(95.0, 145.0)
    vibrato_hz = rng.uniform(4.0, 6.5)
    vibrato_depth = rng.uniform(0.005, 0.02)
    # Formant-ish resonance peaks (Hz) and their widths.
    f1, f2 = rng.uniform(400, 900), rng.uniform(1100, 2600)
    w1, w2 = rng.uniform(120, 300), rng.uniform(200, 500)
    harmonics = []
    for h in range(1, 9):
        f = f0 * h
        gain = math.exp(-((f - f1) ** 2) / (2 * w1 * w1)) + 0.6 * math.exp(
            -((f - f2) ** 2) / (2 * w2 * w2)
        )
        gain += 0.15 / h  # keep a little body even off-resonance
        harmonics.append(gain)
    attack = max(1, int(0.02 * SAMPLE_RATE))
    release = max(1, int(0.04 * SAMPLE_RATE))
    out = []
    for i in range(n):
        t = i / SAMPLE_RATE
        env = min(i / attack, 1.0, (n - i) / release)
        f_inst = f0 * (1.0 + vibrato_depth * math.sin(2 * math.pi * vibrato_hz * t))
        s = sum(g * math.sin(2 * math.pi * f_inst * h * t) for h, g in enumerate(harmonics, 1))
        # a breath of noise so consonant-like transients exist
        s += rng.uniform(-0.05, 0.05)
        out.append(AMPLITUDE * env * s)
    # normalize the burst
    peak = max(1e-9, max(abs(v) for v in out))
    return [v / peak * AMPLITUDE for v in out]


def _hesitation_drone(rng: random.Random, duration_ms: float) -> list[float]:
    """A muleta: sustained voiced 'ahn/ééé'-like drone — steady f0, gentle
    tremolo, narrow resonance around ~600 Hz."""
    n = int(SAMPLE_RATE * duration_ms / 1000)
    f0 = rng.uniform(90.0, 120.0)
    out = []
    attack = max(1, int(0.05 * SAMPLE_RATE))
    for i in range(n):
        t = i / SAMPLE_RATE
        env = min(i / attack, 1.0) * (0.8 + 0.2 * math.sin(2 * math.pi * 5.5 * t))
        s = 0.0
        for h in range(1, 6):
            f = f0 * h
            gain = math.exp(-((f - 600.0) ** 2) / (2 * 250.0 * 250.0)) + 0.1 / h
            s += gain * math.sin(2 * math.pi * f * t)
        out.append(AMPLITUDE * 0.8 * env * s)
    peak = max(1e-9, max(abs(v) for v in out))
    return [v / peak * AMPLITUDE * 0.8 for v in out]


def _silence(duration_ms: float) -> list[float]:
    return [0.0] * int(SAMPLE_RATE * duration_ms / 1000)


def _word(rng: random.Random, syllables: int | None = None) -> list[float]:
    """A 'word': 1–6 syllable bursts separated by tiny intra-word gaps."""
    count = syllables if syllables is not None else rng.randint(1, 6)
    out: list[float] = []
    for i in range(count):
        out += _syllable(rng, rng.uniform(120, 320))
        if i < count - 1:
            out += _silence(rng.uniform(20, 70))
    return out


def fala_curta(seed: int = 9001) -> list[float]:
    """~2 s: two short 'words', like 'olá mundo'."""
    rng = random.Random(seed)
    out = _silence(300)
    out += _word(rng, 2)
    out += _silence(rng.uniform(180, 260))
    out += _word(rng, 2)
    out += _silence(300)
    return out


def fala_longa_muletas(seed: int = 9002) -> list[float]:
    """~15 s: several phrases separated by hesitation drones and long pauses —
    the 'fala longa com muletas' case for filler-word cleanup tests (T-035)."""
    rng = random.Random(seed)
    out = _silence(300)
    for phrase in range(3):
        for _ in range(rng.randint(3, 5)):
            out += _word(rng)
            out += _silence(rng.uniform(120, 350))
        if phrase < 2:
            # muleta: 'ahn…' drone then a thinking pause
            out += _hesitation_drone(rng, rng.uniform(350, 650))
            out += _silence(rng.uniform(300, 700))
            # an immediate word repetition, like "eu eu quero…"
            if rng.random() < 0.6:
                w = _word(rng, 1)
                out += w + _silence(rng.uniform(60, 120)) + w
                out += _silence(rng.uniform(150, 300))
    out += _silence(400)
    return out


def write_wav(path: Path, samples: list[float]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    pcm = struct.pack(
        f"<{len(samples)}h",
        *[max(-32768, min(32767, int(round(s * 32767)))) for s in samples],
    )
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(pcm)
    print(f"wrote {path} ({len(samples) / SAMPLE_RATE:.2f} s)")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=Path(__file__).resolve().parent / "pt-br",
        help="output directory (default: ./pt-br next to this script)",
    )
    args = parser.parse_args()
    write_wav(args.out / "fala_curta.wav", fala_curta())
    write_wav(args.out / "fala_longa_muletas.wav", fala_longa_muletas())


if __name__ == "__main__":
    main()
