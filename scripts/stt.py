#!/usr/bin/env python3
"""CDC STT backend using faster-whisper with Korean-optimized model.

Usage: python3 stt.py <wav_path>
Output: transcribed text to stdout

Requires: pip install faster-whisper
Model: ghost613/faster-whisper-large-v3-turbo-korean (auto-downloaded)
"""
import sys
import os

def transcribe(wav_path: str) -> str:
    from faster_whisper import WhisperModel

    model_id = os.environ.get(
        "CDC_STT_MODEL",
        "ghost613/faster-whisper-large-v3-turbo-korean"
    )
    device = os.environ.get("CDC_STT_DEVICE", "auto")
    compute_type = os.environ.get("CDC_STT_COMPUTE", "int8")

    model = WhisperModel(model_id, device=device, compute_type=compute_type)
    segments, _info = model.transcribe(
        wav_path,
        language="ko",
        beam_size=5,
        vad_filter=True,
    )

    text = " ".join(seg.text.strip() for seg in segments)
    return text.strip()

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: stt.py <wav_path>", file=sys.stderr)
        sys.exit(1)

    # Suppress warnings
    import warnings
    warnings.filterwarnings("ignore")
    import logging
    logging.disable(logging.WARNING)

    result = transcribe(sys.argv[1])
    print(result)
