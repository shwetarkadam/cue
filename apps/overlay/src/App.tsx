import { useEffect, useRef, useState, useCallback, KeyboardEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

interface TranscriptLine {
  channel: string;
  text: string;
}

export default function App() {
  const [transcript, setTranscript] = useState<TranscriptLine[]>([]);
  const [response, setResponse] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [status, setStatus] = useState("Starting...");
  const [error, setError] = useState<string | null>(null);
  const [listening, setListening] = useState(false);
  const [input, setInput] = useState("");

  const responseRef = useRef<HTMLDivElement>(null);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const userScrolledRef = useRef(false);

  // ── Auto-scroll response ──────────────────────────────────────────────────
  useEffect(() => {
    if (!userScrolledRef.current && responseRef.current) {
      responseRef.current.scrollTop = responseRef.current.scrollHeight;
    }
  }, [response]);

  // ── Auto-scroll transcript ────────────────────────────────────────────────
  useEffect(() => {
    if (transcriptRef.current) {
      transcriptRef.current.scrollTop = transcriptRef.current.scrollHeight;
    }
  }, [transcript]);

  // ── Tauri event listeners ─────────────────────────────────────────────────
  useEffect(() => {
    const unsubs: Array<() => void> = [];

    listen<{ channel: string; text: string }>("transcript", (e) => {
      setTranscript((prev) => [...prev.slice(-80), e.payload]);
    }).then((u) => unsubs.push(u));

    listen<{ token: string }>("response-token", (e) => {
      setIsStreaming(true);
      setResponse((prev) => prev + e.payload.token);
    }).then((u) => unsubs.push(u));

    listen("response-done", () => {
      setIsStreaming(false);
      userScrolledRef.current = false;
      // Snap to bottom on completion
      setTimeout(() => {
        if (responseRef.current) {
          responseRef.current.scrollTop = responseRef.current.scrollHeight;
        }
      }, 50);
    }).then((u) => unsubs.push(u));

    listen<{ message: string }>("status", (e) => {
      setStatus(e.payload.message);
      setError(null);
    }).then((u) => unsubs.push(u));

    listen<{ message: string }>("error", (e) => {
      setError(e.payload.message);
    }).then((u) => unsubs.push(u));

    return () => unsubs.forEach((u) => u());
  }, []);

  // ── Commands ──────────────────────────────────────────────────────────────
  const toggleListen = useCallback(async () => {
    const newState = await invoke<boolean>("toggle_listening");
    setListening(newState);
    if (!newState) setError(null);
  }, []);

  const sendQuery = useCallback(async () => {
    const q = input.trim();
    setInput("");
    setResponse("");
    setIsStreaming(false);
    userScrolledRef.current = false;
    await invoke("send_query", { query: q });
  }, [input]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        sendQuery();
      }
      if (e.key === "l" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        toggleListen();
      }
    },
    [sendQuery, toggleListen]
  );

  const handleResponseScroll = useCallback(() => {
    const el = responseRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 32;
    userScrolledRef.current = !atBottom;
  }, []);

  // ── Render ────────────────────────────────────────────────────────────────
  const channelColor = (ch: string) =>
    ch === "system" || ch === "SYS" ? "var(--cyan)" : "var(--green)";
  const channelLabel = (ch: string) =>
    ch === "system" || ch === "SYS" ? "SYS" : "MIC";

  return (
    <div
      style={{
        height: "100vh",
        width: "100vw",
        padding: 8,
        display: "flex",
        flexDirection: "column",
      }}
    >
      {/* Outer glass shell */}
      <div
        className="glass"
        style={{
          flex: 1,
          borderRadius: 18,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          minHeight: 0,
        }}
      >
        {/* ── Title bar ──────────────────────────────────────────────────── */}
        <div
          className="drag"
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "10px 14px",
            borderBottom: "1px solid var(--divider)",
            flexShrink: 0,
          }}
        >
          {/* Left: indicator + name */}
          <div
            className="no-drag"
            style={{ display: "flex", alignItems: "center", gap: 8 }}
          >
            <div
              className={listening ? "dot-listening" : ""}
              style={{
                width: 7,
                height: 7,
                borderRadius: "50%",
                background: listening ? "var(--danger)" : "rgba(255,255,255,0.2)",
                transition: "background 200ms ease",
                flexShrink: 0,
              }}
            />
            <span
              style={{
                color: "var(--text-primary)",
                fontSize: 13,
                fontWeight: 600,
                letterSpacing: "0.03em",
              }}
            >
              cue
            </span>
          </div>

          {/* Right: status */}
          <span
            style={{
              color: listening ? "var(--danger)" : "var(--text-muted)",
              fontSize: 11,
              maxWidth: 220,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              transition: "color 200ms ease",
            }}
          >
            {listening ? "listening..." : error ? "error" : status}
          </span>
        </div>

        {/* ── Transcript ─────────────────────────────────────────────────── */}
        <div
          ref={transcriptRef}
          style={{
            padding: "8px 14px",
            borderBottom: "1px solid var(--divider)",
            maxHeight: 96,
            minHeight: 36,
            overflowY: "auto",
            flexShrink: 0,
          }}
        >
          {transcript.length === 0 ? (
            <p
              style={{
                color: "var(--text-muted)",
                fontSize: 11,
                margin: 0,
                lineHeight: 1.5,
              }}
            >
              No transcript yet — press Ctrl+L to start listening
            </p>
          ) : (
            transcript.slice(-8).map((line, i) => (
              <div
                key={i}
                style={{ fontSize: 11, lineHeight: 1.6, marginBottom: 1 }}
              >
                <span
                  style={{
                    color: channelColor(line.channel),
                    fontWeight: 600,
                    marginRight: 6,
                    fontSize: 10,
                    letterSpacing: "0.04em",
                  }}
                >
                  [{channelLabel(line.channel)}]
                </span>
                <span style={{ color: "var(--text-primary)" }}>
                  {line.text}
                </span>
              </div>
            ))
          )}
        </div>

        {/* ── AI Response ────────────────────────────────────────────────── */}
        <div
          ref={responseRef}
          onScroll={handleResponseScroll}
          style={{
            flex: 1,
            padding: "14px 16px",
            overflowY: "auto",
            minHeight: 0,
          }}
        >
          {response ? (
            <p
              style={{
                color: "var(--text-primary)",
                fontSize: 13,
                lineHeight: 1.75,
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
                margin: 0,
              }}
            >
              {response}
              {isStreaming && (
                <span className="cursor" style={{ color: "var(--accent)", marginLeft: 1 }}>
                  ▋
                </span>
              )}
            </p>
          ) : (
            <p
              style={{
                color: "var(--text-muted)",
                fontSize: 12,
                margin: 0,
                lineHeight: 1.6,
              }}
            >
              AI response will stream here...
            </p>
          )}
        </div>

        {/* ── Input bar ──────────────────────────────────────────────────── */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 8,
            padding: "8px 10px",
            borderTop: "1px solid var(--divider)",
            flexShrink: 0,
          }}
        >
          {/* Mic toggle */}
          <button
            className={`btn-icon no-drag ${listening ? "listening" : ""}`}
            onClick={toggleListen}
            title="Ctrl+L to toggle listening"
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
              <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
              <line x1="12" y1="19" x2="12" y2="23" />
              <line x1="8" y1="23" x2="16" y2="23" />
            </svg>
          </button>

          {/* Text input */}
          <input
            className="glass-input no-drag"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Ask anything..."
            autoFocus
          />

          {/* Send */}
          <button
            className={`btn-icon no-drag ${input.trim() ? "active" : ""}`}
            onClick={sendQuery}
            disabled={!input.trim() && !isStreaming}
            title="Send (Enter)"
          >
            <svg
              width="13"
              height="13"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <line x1="22" y1="2" x2="11" y2="13" />
              <polygon points="22 2 15 22 11 13 2 9 22 2" />
            </svg>
          </button>
        </div>

        {/* ── Error toast ─────────────────────────────────────────────────── */}
        {error && (
          <div
            style={{
              padding: "6px 14px",
              background: "var(--danger-dim)",
              borderTop: "1px solid var(--danger-border)",
              color: "var(--danger)",
              fontSize: 11,
              lineHeight: 1.5,
              flexShrink: 0,
            }}
          >
            {error}
          </div>
        )}
      </div>
    </div>
  );
}
