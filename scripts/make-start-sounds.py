"""Generate three short, softly enveloped recording cues with the stdlib."""

import math
from pathlib import Path
import struct
import wave


RATE = 44100
DESTINATION = Path(__file__).resolve().parents[1] / "assets" / "sounds"


def envelope(t, duration, attack, decay):
    return (1 - math.exp(-t / attack)) ** 2 * math.exp(-t / decay) * min(1, (duration - t) / 0.04) ** 2


def write_sound(name, duration, signal):
    samples = [signal(i / RATE, duration) for i in range(round(duration * RATE))]
    gain = 0.16 / max(abs(sample) for sample in samples)
    pcm = [round(sample * gain * 32767) for sample in samples]
    assert abs(pcm[0]) == 0 and abs(pcm[-1]) <= 1
    assert max(abs(sample) for sample in pcm) < 6000
    with wave.open(str(DESTINATION / f"{name}.wav"), "wb") as sound:
        sound.setparams((1, 2, RATE, len(pcm), "NONE", "not compressed"))
        sound.writeframes(struct.pack(f"<{len(pcm)}h", *pcm))


def soft(t, duration):
    first = math.sin(math.tau * 440 * t) * envelope(t, duration, 0.012, 0.075)
    later = max(0, t - 0.035)
    second = 0.65 * math.sin(math.tau * 659.255 * later) * envelope(later, duration - 0.035, 0.012, 0.08)
    return first + second


def droplet(t, duration):
    # A falling pitch with a rounded attack, without a sharp transient.
    phase = math.tau * (460 * t + 42 * (1 - math.exp(-t / 0.035)))
    return math.sin(phase) * envelope(t, duration, 0.008, 0.045)


def tap(t, duration):
    return (math.sin(math.tau * 260 * t) + 0.3 * math.sin(math.tau * 520 * t)) * envelope(t, duration, 0.004, 0.022)


if __name__ == "__main__":
    DESTINATION.mkdir(parents=True, exist_ok=True)
    for name, duration, signal in [("soft", 0.3, soft), ("droplet", 0.2, droplet), ("tap", 0.13, tap)]:
        write_sound(name, duration, signal)
    print("Generated soft, droplet, and tap cues; PCM format, peak and edge checks passed")
