#!/usr/bin/env python3
"""Long-running JSON-line TTS server for KittenTTS.

Reads JSON requests from stdin, writes JSON responses to stdout.
The model is loaded once and reused across requests.

Request format:  {"text": "Hello world", "voice": "Jasper", "speed": 1.0}
Response format: {"ok": true, "path": "/tmp/cue_tts_abc123.wav"}
                 {"ok": false, "error": "some error message"}
"""
import json
import os
import sys
import tempfile

def main():
    model_id = os.environ.get("KITTEN_MODEL", "KittenML/kitten-tts-nano-0.8-int8")

    try:
        from kittentts import KittenTTS
        model = KittenTTS(model_id)
    except Exception as e:
        # Report load failure and exit
        sys.stdout.write(json.dumps({"ok": False, "error": f"Failed to load model: {e}"}) + "\n")
        sys.stdout.flush()
        sys.exit(1)

    # Signal ready
    sys.stdout.write(json.dumps({"ok": True, "ready": True}) + "\n")
    sys.stdout.flush()

    tts_dir = os.path.join(tempfile.gettempdir(), "cue_tts")
    os.makedirs(tts_dir, exist_ok=True)

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            text = req.get("text", "")
            if not text:
                raise ValueError("empty text")

            voice = req.get("voice", "Jasper")
            speed = float(req.get("speed", 1.0))

            # Generate to temp file
            fd, out_path = tempfile.mkstemp(suffix=".wav", dir=tts_dir)
            os.close(fd)

            # Suppress KittenTTS stdout prints ("Audio saved to ...")
            # by temporarily redirecting stdout to devnull during generation.
            _real_stdout = sys.stdout
            sys.stdout = open(os.devnull, "w")
            try:
                model.generate_to_file(
                    text, out_path,
                    voice=voice,
                    speed=speed,
                    sample_rate=24000,
                )
            finally:
                sys.stdout.close()
                sys.stdout = _real_stdout
            resp = {"ok": True, "path": out_path}
        except Exception as e:
            resp = {"ok": False, "error": str(e)}

        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
