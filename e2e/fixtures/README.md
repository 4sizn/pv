# Deterministic media fixture

`continuous-tone.wav` is a generated 1 kHz sine wave: mono, 48 kHz, signed 16-bit PCM, 0.25 seconds, amplitude 8192. Chromium loops this synthetic source for every fake microphone. No recorded voice or real microphone is used.

The default Chromium fake microphone consumes a shared one-shot beep flag, so concurrent captures are not independently guaranteed non-silent input. A looped file makes the mesh audio byte-growth checks independent of that shared fixture state. See [Chromium BeepingSource](https://chromium.googlesource.com/chromium/src/+/d65a7128bcbb5ff37a71fcfe66e4f7e05520eee6/media/audio/simple_sources.cc).

Recreate the 24,044-byte file from this directory with Python's standard library:

```python
import math
import struct
import wave

with wave.open("continuous-tone.wav", "wb") as audio:
    audio.setparams((1, 2, 48000, 0, "NONE", "not compressed"))
    audio.writeframes(b"".join(
        struct.pack("<h", round(8192 * math.sin(2 * math.pi * 1000 * i / 48000)))
        for i in range(12000)
    ))
```

The fixture is checked in; running CI requires no Python or fixture generation step.
