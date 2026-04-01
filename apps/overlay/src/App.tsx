import { useEffect, useRef, useState, useCallback, KeyboardEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

// ── Types ─────────────────────────────────────────────────────────────────────

interface TranscriptLine { channel: string; text: string }
interface ConvEntry { query: string; response: string; created_at: string; streaming?: boolean }
interface PromptInfo { id: string; label: string; description: string }
interface Settings {
  provider: string; model: string; active_prompt: string;
  anthropic_key: string; openai_key: string; deepgram_key: string;
  groq_key: string; ollama_endpoint: string;
}
interface KbDoc { id: number; name: string; doc_type: string; chunk_count: number; created_at: string }
interface PNote { id: number; title: string; content: string; sort_order: number; prompt_category: string }
interface BNote { id: number; category: string; content: string }
interface BFolder { id: number; name: string; linked_prompt: string | null; doc_count: number }

type SettingsTab = "prompt" | "keys" | "model" | "kb" | "brain" | "setup";

// ── Constants ─────────────────────────────────────────────────────────────────

const PROVIDERS = ["anthropic", "openai", "groq", "ollama"];

// ── Channel helpers ───────────────────────────────────────────────────────────

const chColor = (ch: string) => ch === "system" || ch === "SYS" ? "var(--cyan)" : "var(--green)";
const chLabel = (ch: string) => ch === "system" || ch === "SYS" ? "SYS" : "MIC";

// ── Main component ────────────────────────────────────────────────────────────

export default function App() {
  // Chat state
  const [transcript, setTranscript] = useState<TranscriptLine[]>([]);
  const [conv, setConv] = useState<ConvEntry[]>([]);
  const [status, setStatus] = useState("Starting...");
  const [error, setError] = useState<string | null>(null);
  const [listening, setListening] = useState(false);
  const [input, setInput] = useState("");

  // STT live transcription state
  const sttCommittedRef = useRef("");    // finalized text so far
  const sttInterimRef = useRef("");      // current interim partial
  const [view, setView] = useState<"chat" | "settings" | "prompter">("chat");

  // Prompter state
  const [pNotes, setPNotes] = useState<PNote[]>([]);
  const [pEditId, setPEditId] = useState<number | null>(null);
  const [pDraftTitle, setPDraftTitle] = useState("");
  const [pDraftContent, setPDraftContent] = useState("");
  const [pAdding, setPAdding] = useState(false);
  const [pAutoScroll, setPAutoScroll] = useState(true);
  const [pActiveCard, setPActiveCard] = useState(0);
  const prompterRef = useRef<HTMLDivElement>(null);
  const pScrollTimerRef = useRef<number | null>(null);

  // Settings state
  const [stab, setStab] = useState<SettingsTab>("prompt");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [settingsDraft, setSettingsDraft] = useState<Settings | null>(null);
  const [prompts, setPrompts] = useState<PromptInfo[]>([]);
  const [kbDocs, setKbDocs] = useState<KbDoc[]>([]);
  const [kbPath, setKbPath] = useState("");
  const [saving, setSaving] = useState(false);

  // Brain state
  const [brainNotes, setBrainNotes] = useState<BNote[]>([]);
  const [brainFolders, setBrainFolders] = useState<BFolder[]>([]);
  const [bnEditCat, setBnEditCat] = useState<string | null>(null);
  const [bnDraftCat, setBnDraftCat] = useState("");
  const [bnDraftContent, setBnDraftContent] = useState("");
  const [bfNewName, setBfNewName] = useState("");
  const [bfNewLink, setBfNewLink] = useState("");

  // Refs
  const convEndRef = useRef<HTMLDivElement>(null);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const userScrolledRef = useRef(false);
  const convContainerRef = useRef<HTMLDivElement>(null);

  // ── Load history on mount ──────────────────────────────────────────────────

  useEffect(() => {
    invoke<{ query: string; response: string; created_at: string }[]>("load_history")
      .then((items) => setConv(items.map((h) => ({ ...h, streaming: false }))))
      .catch(() => {});
  }, []);

  // ── Global keyboard shortcut: Ctrl+Shift+H to hide/show window ────────────

  useEffect(() => {
    const handler = (e: globalThis.KeyboardEvent) => {
      if (e.key === "H" && e.ctrlKey && e.shiftKey) {
        e.preventDefault();
        invoke("toggle_window").catch(() => {});
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // ── Auto-scroll conversation ───────────────────────────────────────────────

  useEffect(() => {
    if (!userScrolledRef.current) {
      convEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [conv]);

  useEffect(() => {
    if (transcriptRef.current) {
      transcriptRef.current.scrollTop = transcriptRef.current.scrollHeight;
    }
  }, [transcript]);

  // ── Tauri events ───────────────────────────────────────────────────────────

  useEffect(() => {
    const unsubs: (() => void)[] = [];

    listen<{ channel: string; text: string }>("transcript", (e) => {
      setTranscript((prev) => [...prev.slice(-80), e.payload]);
    }).then((u) => unsubs.push(u));

    listen<{ token: string }>("response-token", (e) => {
      setConv((prev) => {
        if (prev.length === 0) return prev;
        const last = prev[prev.length - 1];
        if (!last.streaming) return prev;
        return [
          ...prev.slice(0, -1),
          { ...last, response: last.response + e.payload.token },
        ];
      });
    }).then((u) => unsubs.push(u));

    listen("response-done", () => {
      userScrolledRef.current = false;
      setConv((prev) => {
        if (prev.length === 0) return prev;
        return [...prev.slice(0, -1), { ...prev[prev.length - 1], streaming: false }];
      });
      setTimeout(() => convEndRef.current?.scrollIntoView({ behavior: "smooth" }), 50);
    }).then((u) => unsubs.push(u));

    listen<{ text: string; is_final: boolean }>("stt-live", (e) => {
      const { text, is_final } = e.payload;
      if (is_final) {
        // Commit this final segment
        if (text) {
          sttCommittedRef.current = sttCommittedRef.current
            ? sttCommittedRef.current + " " + text
            : text;
        }
        sttInterimRef.current = "";
      } else {
        // Update interim (partial) text
        sttInterimRef.current = text;
      }
      // Build the live input: committed + interim
      const committed = sttCommittedRef.current;
      const interim = sttInterimRef.current;
      const combined = committed && interim
        ? committed + " " + interim
        : committed || interim;
      setInput(combined);
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

  // ── Commands ───────────────────────────────────────────────────────────────

  const toggleListen = useCallback(async () => {
    const newState = await invoke<boolean>("toggle_listening");
    setListening(newState);
    if (newState) {
      // Starting to listen — reset STT accumulators
      sttCommittedRef.current = "";
      sttInterimRef.current = "";
    } else {
      // Stopped listening — commit any remaining interim text
      if (sttInterimRef.current) {
        sttCommittedRef.current = sttCommittedRef.current
          ? sttCommittedRef.current + " " + sttInterimRef.current
          : sttInterimRef.current;
        sttInterimRef.current = "";
        setInput(sttCommittedRef.current);
      }
      setError(null);
    }
  }, []);

  const sendQuery = useCallback(async () => {
    const q = input.trim();
    if (!q) return;
    setInput("");
    sttCommittedRef.current = "";
    sttInterimRef.current = "";
    userScrolledRef.current = false;
    const entry: ConvEntry = {
      query: q,
      response: "",
      created_at: new Date().toISOString(),
      streaming: true,
    };
    setConv((prev) => [...prev, entry]);
    await invoke("send_query", { query: q });
  }, [input]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); sendQuery(); }
      if (e.key === "l" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); toggleListen(); }
    },
    [sendQuery, toggleListen]
  );

  const handleConvScroll = useCallback(() => {
    const el = convContainerRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    userScrolledRef.current = !atBottom;
  }, []);

  // ── Settings helpers ───────────────────────────────────────────────────────

  const openSettings = useCallback(async () => {
    const [s, p, kb, bn, bf] = await Promise.all([
      invoke<Settings>("get_settings"),
      invoke<PromptInfo[]>("get_prompts"),
      invoke<KbDoc[]>("list_kb"),
      invoke<BNote[]>("list_brain_notes"),
      invoke<BFolder[]>("list_brain_folders"),
    ]);
    setSettings(s);
    setSettingsDraft(s);
    setPrompts(p);
    setKbDocs(kb);
    setBrainNotes(bn);
    setBrainFolders(bf);
    setView("settings");
  }, []);

  const saveSettings = useCallback(async () => {
    if (!settingsDraft) return;
    setSaving(true);
    try {
      await invoke("save_settings", { settings: settingsDraft });
      setSettings(settingsDraft);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }, [settingsDraft]);

  const deleteKbDoc = useCallback(async (id: number) => {
    await invoke("delete_kb_doc", { id });
    setKbDocs((prev) => prev.filter((d) => d.id !== id));
  }, []);

  const ingestKbFile = useCallback(async () => {
    const p = kbPath.trim();
    if (!p) return;
    try {
      const doc = await invoke<KbDoc>("ingest_kb_file", { path: p });
      setKbDocs((prev) => [...prev, doc]);
      setKbPath("");
    } catch (e) {
      setError(String(e));
    }
  }, [kbPath]);

  const patchDraft = (patch: Partial<Settings>) =>
    setSettingsDraft((prev) => (prev ? { ...prev, ...patch } : prev));

  // ── Brain helpers ───────────────────────────────────────────────────────────

  const saveBrainNote = useCallback(async () => {
    const cat = bnDraftCat.trim();
    const content = bnDraftContent.trim();
    if (!cat || !content) return;
    try {
      const note = await invoke<BNote>("save_brain_note", { category: cat, content });
      setBrainNotes((prev) => {
        const exists = prev.findIndex((n) => n.category === note.category);
        if (exists >= 0) return [...prev.slice(0, exists), note, ...prev.slice(exists + 1)];
        return [...prev, note];
      });
      setBnEditCat(null);
      setBnDraftCat("");
      setBnDraftContent("");
    } catch (e) { setError(String(e)); }
  }, [bnDraftCat, bnDraftContent]);

  const deleteBrainNote = useCallback(async (category: string) => {
    try {
      await invoke("delete_brain_note", { category });
      setBrainNotes((prev) => prev.filter((n) => n.category !== category));
    } catch (e) { setError(String(e)); }
  }, []);

  const startEditBrainNote = useCallback((note: BNote) => {
    setBnEditCat(note.category);
    setBnDraftCat(note.category);
    setBnDraftContent(note.content);
  }, []);

  const createBrainFolder = useCallback(async () => {
    const name = bfNewName.trim();
    if (!name) return;
    try {
      const folder = await invoke<BFolder>("create_brain_folder", {
        name, linkedPrompt: bfNewLink.trim() || null,
      });
      setBrainFolders((prev) => [...prev, folder]);
      setBfNewName("");
      setBfNewLink("");
    } catch (e) { setError(String(e)); }
  }, [bfNewName, bfNewLink]);

  const deleteBrainFolder = useCallback(async (name: string) => {
    try {
      await invoke("delete_brain_folder", { name });
      setBrainFolders((prev) => prev.filter((f) => f.name !== name));
    } catch (e) { setError(String(e)); }
  }, []);

  // ── Prompter helpers ────────────────────────────────────────────────────────

  const activeCategory = useCallback(() => {
    return settings?.active_prompt ?? "general";
  }, [settings]);

  const loadPrompterNotes = useCallback(async () => {
    try {
      const s = await invoke<Settings>("get_settings");
      setSettings(s);
      const notes = await invoke<PNote[]>("list_prompter_notes", { category: s.active_prompt });
      setPNotes(notes);
    } catch { /* noop */ }
  }, []);

  const openPrompter = useCallback(async () => {
    await loadPrompterNotes();
    setPAdding(false);
    setPEditId(null);
    setView("prompter");
  }, [loadPrompterNotes]);

  const addPrompterNote = useCallback(async () => {
    const t = pDraftTitle.trim();
    const c = pDraftContent.trim();
    if (!c && !t) return;
    try {
      const note = await invoke<PNote>("add_prompter_note", {
        title: t, content: c, category: activeCategory(),
      });
      setPNotes((prev) => [...prev, note]);
      setPDraftTitle("");
      setPDraftContent("");
      setPAdding(false);
    } catch (e) { setError(String(e)); }
  }, [pDraftTitle, pDraftContent, activeCategory]);

  const saveEditNote = useCallback(async () => {
    if (pEditId === null) return;
    try {
      const note = await invoke<PNote>("update_prompter_note", {
        id: pEditId, title: pDraftTitle.trim(), content: pDraftContent.trim(),
      });
      setPNotes((prev) => prev.map((n) => (n.id === note.id ? note : n)));
      setPEditId(null);
      setPDraftTitle("");
      setPDraftContent("");
    } catch (e) { setError(String(e)); }
  }, [pEditId, pDraftTitle, pDraftContent]);

  const deletePrompterNote = useCallback(async (id: number) => {
    try {
      await invoke("delete_prompter_note", { id });
      setPNotes((prev) => prev.filter((n) => n.id !== id));
    } catch (e) { setError(String(e)); }
  }, []);

  const startEdit = useCallback((note: PNote) => {
    setPEditId(note.id);
    setPDraftTitle(note.title);
    setPDraftContent(note.content);
    setPAdding(false);
  }, []);

  const cancelEdit = useCallback(() => {
    setPEditId(null);
    setPDraftTitle("");
    setPDraftContent("");
    setPAdding(false);
  }, []);

  const moveNote = useCallback(async (idx: number, dir: -1 | 1) => {
    const target = idx + dir;
    if (target < 0 || target >= pNotes.length) return;
    const reordered = [...pNotes];
    [reordered[idx], reordered[target]] = [reordered[target], reordered[idx]];
    setPNotes(reordered);
    try {
      await invoke("reorder_prompter_notes", { ids: reordered.map((n) => n.id) });
    } catch { /* best effort */ }
  }, [pNotes]);

  // Auto-scroll prompter cards when listening
  useEffect(() => {
    if (!pAutoScroll || !listening || view !== "prompter" || pNotes.length === 0) {
      if (pScrollTimerRef.current) { clearInterval(pScrollTimerRef.current); pScrollTimerRef.current = null; }
      return;
    }
    pScrollTimerRef.current = window.setInterval(() => {
      setPActiveCard((prev) => {
        const next = prev < pNotes.length - 1 ? prev + 1 : prev;
        const el = document.getElementById(`pcard-${next}`);
        el?.scrollIntoView({ behavior: "smooth", block: "center" });
        return next;
      });
    }, 8000);
    return () => { if (pScrollTimerRef.current) clearInterval(pScrollTimerRef.current); };
  }, [pAutoScroll, listening, view, pNotes.length]);

  // Keyboard navigation for prompter
  useEffect(() => {
    if (view !== "prompter") return;
    const handler = (e: globalThis.KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;

      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        setPActiveCard((prev) => {
          const next = Math.min(prev + 1, pNotes.length - 1);
          document.getElementById(`pcard-${next}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
          return next;
        });
      } else if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        setPActiveCard((prev) => {
          const next = Math.max(prev - 1, 0);
          document.getElementById(`pcard-${next}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
          return next;
        });
      } else if (e.key === "Home") {
        e.preventDefault();
        setPActiveCard(0);
        document.getElementById("pcard-0")?.scrollIntoView({ behavior: "smooth", block: "center" });
      } else if (e.key === "End") {
        e.preventDefault();
        const last = pNotes.length - 1;
        setPActiveCard(last);
        document.getElementById(`pcard-${last}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
      } else if (e.key === "Escape") {
        setView("chat");
      } else if (e.key === " ") {
        e.preventDefault();
        setPAutoScroll((p) => !p);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [view, pNotes.length]);

  // ── Render: Settings ───────────────────────────────────────────────────────

  if (view === "settings") {
    return (
      <div style={{ height: "100vh", width: "100vw", padding: 8, display: "flex", flexDirection: "column" }}>
        <div className="glass" style={{ flex: 1, borderRadius: 18, display: "flex", flexDirection: "column", overflow: "hidden", minHeight: 0 }}>

          {/* Settings title bar */}
          <div className="drag" style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "10px 14px", borderBottom: "1px solid var(--divider)", flexShrink: 0 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <button className="btn-icon no-drag" onClick={() => setView("chat")} title="Back">
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
                  <polyline points="15 18 9 12 15 6" />
                </svg>
              </button>
              <span style={{ color: "var(--text-primary)", fontSize: 13, fontWeight: 600, letterSpacing: "0.03em" }}>Settings</span>
            </div>
            <button
              className="no-drag"
              onClick={saveSettings}
              disabled={saving}
              style={{ fontSize: 11, padding: "4px 12px", borderRadius: 6, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer" }}
            >
              {saving ? "Saving…" : "Save"}
            </button>
          </div>

          {/* Tab bar */}
          <div style={{ display: "flex", gap: 2, padding: "8px 10px 0", borderBottom: "1px solid var(--divider)", flexShrink: 0 }}>
            {(["prompt", "keys", "model", "kb", "brain", "setup"] as SettingsTab[]).map((t) => (
              <button
                key={t}
                className="no-drag"
                onClick={() => setStab(t)}
                style={{
                  fontSize: 11, padding: "4px 10px", borderRadius: "6px 6px 0 0",
                  border: "1px solid transparent", borderBottom: "none",
                  background: stab === t ? "rgba(255,255,255,0.07)" : "transparent",
                  color: stab === t ? "var(--text-primary)" : "var(--text-muted)",
                  cursor: "pointer", transition: "all 150ms ease",
                }}
              >
                {{ prompt: "Prompt", keys: "API Keys", model: "Model", kb: "Knowledge Base", brain: "Brain", setup: "Setup" }[t]}
              </button>
            ))}
          </div>

          {/* Tab content */}
          <div style={{ flex: 1, overflowY: "auto", padding: "16px 16px", minHeight: 0 }}>
            {stab === "prompt" && settingsDraft && (
              <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                <p style={{ color: "var(--text-muted)", fontSize: 11, margin: "0 0 8px" }}>Select the AI persona for this session.</p>
                {prompts.map((p) => (
                  <label
                    key={p.id}
                    className="no-drag"
                    style={{
                      display: "flex", alignItems: "flex-start", gap: 10, padding: "10px 12px",
                      borderRadius: 10, border: `1px solid ${settingsDraft.active_prompt === p.id ? "var(--accent-border)" : "var(--divider)"}`,
                      background: settingsDraft.active_prompt === p.id ? "var(--accent-dim)" : "rgba(255,255,255,0.02)",
                      cursor: "pointer", transition: "all 150ms ease",
                    }}
                  >
                    <input
                      type="radio"
                      name="prompt"
                      value={p.id}
                      checked={settingsDraft.active_prompt === p.id}
                      onChange={() => patchDraft({ active_prompt: p.id })}
                      style={{ marginTop: 2, accentColor: "var(--accent)", flexShrink: 0 }}
                    />
                    <div>
                      <div style={{ color: "var(--text-primary)", fontSize: 12, fontWeight: 600 }}>{p.label}</div>
                      <div style={{ color: "var(--text-muted)", fontSize: 11, marginTop: 2, lineHeight: 1.4 }}>{p.description}</div>
                    </div>
                  </label>
                ))}
              </div>
            )}

            {stab === "keys" && settingsDraft && (
              <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
                <p style={{ color: "var(--text-muted)", fontSize: 11, margin: "0 0 4px" }}>Keys are saved to <code style={{ color: "var(--cyan)", fontSize: 10 }}>~/.config/cue/.env</code></p>
                {([
                  ["Anthropic", "anthropic_key", "sk-ant-…"],
                  ["OpenAI", "openai_key", "sk-…"],
                  ["Deepgram", "deepgram_key", "STT transcription"],
                  ["Groq", "groq_key", "gsk_…"],
                ] as [string, keyof Settings, string][]).map(([label, field, placeholder]) => (
                  <div key={field}>
                    <label style={{ display: "block", color: "var(--text-secondary)", fontSize: 11, marginBottom: 4 }}>{label}</label>
                    <input
                      className="glass-input no-drag"
                      type="password"
                      value={settingsDraft[field] as string}
                      onChange={(e) => patchDraft({ [field]: e.target.value })}
                      placeholder={placeholder}
                      style={{ width: "100%" }}
                    />
                  </div>
                ))}
              </div>
            )}

            {stab === "model" && settingsDraft && (
              <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
                <div>
                  <label style={{ display: "block", color: "var(--text-secondary)", fontSize: 11, marginBottom: 6 }}>Provider</label>
                  <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                    {PROVIDERS.map((p) => (
                      <button
                        key={p}
                        className="no-drag"
                        onClick={() => patchDraft({ provider: p })}
                        style={{
                          fontSize: 11, padding: "5px 12px", borderRadius: 7,
                          border: `1px solid ${settingsDraft.provider === p ? "var(--accent-border)" : "var(--divider)"}`,
                          background: settingsDraft.provider === p ? "var(--accent-dim)" : "rgba(255,255,255,0.03)",
                          color: settingsDraft.provider === p ? "var(--accent)" : "var(--text-secondary)",
                          cursor: "pointer", transition: "all 150ms ease",
                        }}
                      >
                        {p}
                      </button>
                    ))}
                  </div>
                </div>
                <div>
                  <label style={{ display: "block", color: "var(--text-secondary)", fontSize: 11, marginBottom: 4 }}>Model</label>
                  <input
                    className="glass-input no-drag"
                    value={settingsDraft.model}
                    onChange={(e) => patchDraft({ model: e.target.value })}
                    placeholder="e.g. claude-sonnet-4-6 / gpt-4o / llama3-70b"
                    style={{ width: "100%" }}
                  />
                </div>
                {settingsDraft.provider === "ollama" && (
                  <div>
                    <label style={{ display: "block", color: "var(--text-secondary)", fontSize: 11, marginBottom: 4 }}>Ollama Endpoint</label>
                    <input
                      className="glass-input no-drag"
                      value={settingsDraft.ollama_endpoint}
                      onChange={(e) => patchDraft({ ollama_endpoint: e.target.value })}
                      placeholder="http://localhost:11434"
                      style={{ width: "100%" }}
                    />
                  </div>
                )}
              </div>
            )}

            {stab === "kb" && (
              <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
                <p style={{ color: "var(--text-muted)", fontSize: 11, margin: "0 0 4px" }}>Documents are embedded and used for context during queries.</p>
                {/* Ingest row */}
                <div style={{ display: "flex", gap: 6 }}>
                  <input
                    className="glass-input no-drag"
                    value={kbPath}
                    onChange={(e) => setKbPath(e.target.value)}
                    placeholder="/path/to/document.pdf or .txt"
                    style={{ flex: 1 }}
                    onKeyDown={(e) => { if (e.key === "Enter") ingestKbFile(); }}
                  />
                  <button
                    className="no-drag"
                    onClick={ingestKbFile}
                    style={{ fontSize: 11, padding: "0 12px", borderRadius: 7, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer", flexShrink: 0 }}
                  >
                    Add
                  </button>
                </div>
                {/* Doc list */}
                {kbDocs.length === 0 ? (
                  <p style={{ color: "var(--text-muted)", fontSize: 11, margin: 0 }}>No documents yet.</p>
                ) : (
                  kbDocs.map((doc) => (
                    <div key={doc.id} style={{ display: "flex", alignItems: "center", gap: 8, padding: "8px 10px", borderRadius: 8, border: "1px solid var(--divider)", background: "rgba(255,255,255,0.02)" }}>
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ color: "var(--text-primary)", fontSize: 12, fontWeight: 500, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{doc.name}</div>
                        <div style={{ color: "var(--text-muted)", fontSize: 10, marginTop: 2 }}>{doc.doc_type} · {doc.chunk_count} chunks · {doc.created_at}</div>
                      </div>
                      <button
                        className="no-drag"
                        onClick={() => deleteKbDoc(doc.id)}
                        style={{ fontSize: 10, padding: "3px 8px", borderRadius: 5, border: "1px solid var(--danger-border)", background: "var(--danger-dim)", color: "var(--danger)", cursor: "pointer", flexShrink: 0 }}
                      >
                        Remove
                      </button>
                    </div>
                  ))
                )}
              </div>
            )}

            {stab === "brain" && (
              <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
                {/* Brain Notes section */}
                <div>
                  <p style={{ color: "var(--text-primary)", fontSize: 12, fontWeight: 600, margin: "0 0 4px" }}>Notes</p>
                  <p style={{ color: "var(--text-muted)", fontSize: 11, margin: "0 0 10px" }}>Per-prompt notes injected into AI context. One note per prompt category.</p>

                  {brainNotes.map((note) => (
                    bnEditCat === note.category ? (
                      <div key={note.category} style={{ marginBottom: 8, padding: "10px 12px", borderRadius: 10, border: "1px solid var(--accent-border)", background: "var(--accent-dim)" }}>
                        <input
                          className="glass-input no-drag"
                          value={bnDraftCat}
                          onChange={(e) => setBnDraftCat(e.target.value)}
                          placeholder="Category (e.g. coding)"
                          style={{ width: "100%", marginBottom: 6, fontSize: 12 }}
                          disabled
                        />
                        <textarea
                          className="glass-input no-drag"
                          value={bnDraftContent}
                          onChange={(e) => setBnDraftContent(e.target.value)}
                          placeholder="Note content..."
                          style={{ width: "100%", minHeight: 60, resize: "vertical", padding: "8px 12px", fontSize: 12, lineHeight: 1.6, fontFamily: "inherit" }}
                          autoFocus
                        />
                        <div style={{ display: "flex", gap: 6, marginTop: 6, justifyContent: "flex-end" }}>
                          <button className="no-drag" onClick={() => { setBnEditCat(null); setBnDraftCat(""); setBnDraftContent(""); }} style={{ fontSize: 10, padding: "3px 8px", borderRadius: 5, border: "1px solid var(--divider)", background: "rgba(255,255,255,0.03)", color: "var(--text-muted)", cursor: "pointer" }}>Cancel</button>
                          <button className="no-drag" onClick={saveBrainNote} style={{ fontSize: 10, padding: "3px 8px", borderRadius: 5, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer" }}>Save</button>
                        </div>
                      </div>
                    ) : (
                      <div key={note.category} style={{ marginBottom: 8, display: "flex", alignItems: "flex-start", gap: 8, padding: "8px 10px", borderRadius: 8, border: "1px solid var(--divider)", background: "rgba(255,255,255,0.02)" }}>
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div style={{ color: "var(--accent)", fontSize: 11, fontWeight: 600 }}>{note.category}</div>
                          <div style={{ color: "var(--text-secondary)", fontSize: 11, marginTop: 2, lineHeight: 1.5, whiteSpace: "pre-wrap", maxHeight: 60, overflow: "hidden" }}>{note.content}</div>
                        </div>
                        <div style={{ display: "flex", gap: 4, flexShrink: 0 }}>
                          <button className="btn-icon no-drag" onClick={() => startEditBrainNote(note)} style={{ width: 24, height: 24 }} title="Edit">
                            <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
                          </button>
                          <button className="btn-icon no-drag" onClick={() => deleteBrainNote(note.category)} style={{ width: 24, height: 24 }} title="Delete">
                            <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="var(--danger)" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                          </button>
                        </div>
                      </div>
                    )
                  ))}

                  {/* Add new note */}
                  {bnEditCat === "__new__" ? (
                    <div style={{ padding: "10px 12px", borderRadius: 10, border: "1px solid var(--accent-border)", background: "var(--accent-dim)" }}>
                      <input
                        className="glass-input no-drag"
                        value={bnDraftCat}
                        onChange={(e) => setBnDraftCat(e.target.value)}
                        placeholder="Category (e.g. coding, meeting, sales)"
                        style={{ width: "100%", marginBottom: 6, fontSize: 12 }}
                        autoFocus
                      />
                      <textarea
                        className="glass-input no-drag"
                        value={bnDraftContent}
                        onChange={(e) => setBnDraftContent(e.target.value)}
                        placeholder="Note content..."
                        style={{ width: "100%", minHeight: 60, resize: "vertical", padding: "8px 12px", fontSize: 12, lineHeight: 1.6, fontFamily: "inherit" }}
                      />
                      <div style={{ display: "flex", gap: 6, marginTop: 6, justifyContent: "flex-end" }}>
                        <button className="no-drag" onClick={() => { setBnEditCat(null); setBnDraftCat(""); setBnDraftContent(""); }} style={{ fontSize: 10, padding: "3px 8px", borderRadius: 5, border: "1px solid var(--divider)", background: "rgba(255,255,255,0.03)", color: "var(--text-muted)", cursor: "pointer" }}>Cancel</button>
                        <button className="no-drag" onClick={saveBrainNote} style={{ fontSize: 10, padding: "3px 8px", borderRadius: 5, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer" }}>Save</button>
                      </div>
                    </div>
                  ) : (
                    <button
                      className="no-drag"
                      onClick={() => { setBnEditCat("__new__"); setBnDraftCat(""); setBnDraftContent(""); }}
                      style={{ fontSize: 11, padding: "6px 12px", borderRadius: 7, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer", alignSelf: "flex-start" }}
                    >
                      + Add Note
                    </button>
                  )}
                </div>

                {/* Brain Folders section */}
                <div style={{ borderTop: "1px solid var(--divider)", paddingTop: 14 }}>
                  <p style={{ color: "var(--text-primary)", fontSize: 12, fontWeight: 600, margin: "0 0 4px" }}>Folders</p>
                  <p style={{ color: "var(--text-muted)", fontSize: 11, margin: "0 0 10px" }}>Reference material organized by prompt. Link a folder to a prompt to auto-inject its documents.</p>

                  {brainFolders.length === 0 ? (
                    <p style={{ color: "var(--text-muted)", fontSize: 11, margin: "0 0 8px" }}>No folders yet. Use <code style={{ color: "var(--cyan)", fontSize: 10 }}>cue brain add</code> to add documents.</p>
                  ) : (
                    brainFolders.map((f) => (
                      <div key={f.id} style={{ marginBottom: 8, display: "flex", alignItems: "center", gap: 8, padding: "8px 10px", borderRadius: 8, border: "1px solid var(--divider)", background: "rgba(255,255,255,0.02)" }}>
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div style={{ color: "var(--text-primary)", fontSize: 12, fontWeight: 500 }}>{f.name}</div>
                          <div style={{ color: "var(--text-muted)", fontSize: 10, marginTop: 2 }}>
                            {f.doc_count} doc{f.doc_count !== 1 ? "s" : ""}
                            {f.linked_prompt && <span> · linked to <span style={{ color: "var(--accent)" }}>{f.linked_prompt}</span></span>}
                          </div>
                        </div>
                        <button className="btn-icon no-drag" onClick={() => deleteBrainFolder(f.name)} style={{ width: 24, height: 24 }} title="Delete folder">
                          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="var(--danger)" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                        </button>
                      </div>
                    ))
                  )}

                  {/* Add new folder */}
                  <div style={{ display: "flex", gap: 6, marginTop: 4 }}>
                    <input
                      className="glass-input no-drag"
                      value={bfNewName}
                      onChange={(e) => setBfNewName(e.target.value)}
                      placeholder="Folder name"
                      style={{ flex: 1, fontSize: 11 }}
                      onKeyDown={(e) => { if (e.key === "Enter") createBrainFolder(); }}
                    />
                    <input
                      className="glass-input no-drag"
                      value={bfNewLink}
                      onChange={(e) => setBfNewLink(e.target.value)}
                      placeholder="Link to prompt (optional)"
                      style={{ flex: 1, fontSize: 11 }}
                      onKeyDown={(e) => { if (e.key === "Enter") createBrainFolder(); }}
                    />
                    <button
                      className="no-drag"
                      onClick={createBrainFolder}
                      style={{ fontSize: 11, padding: "0 10px", borderRadius: 7, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer", flexShrink: 0 }}
                    >
                      Add
                    </button>
                  </div>
                </div>
              </div>
            )}

            {stab === "setup" && (
              <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
                <div>
                  <p style={{ color: "var(--text-primary)", fontSize: 12, fontWeight: 600, margin: "0 0 8px" }}>Hyprland — Float + Hide from Screen Share</p>
                  <p style={{ color: "var(--text-muted)", fontSize: 11, margin: "0 0 10px", lineHeight: 1.6 }}>
                    Add these rules to <code style={{ color: "var(--cyan)", fontSize: 10 }}>~/.config/hypr/hyprland.conf</code> so the overlay floats and stays hidden from screen recording:
                  </p>
                  <pre style={{
                    background: "rgba(0,0,0,0.4)", border: "1px solid var(--divider)", borderRadius: 8,
                    padding: "10px 12px", fontSize: 11, color: "var(--green)", lineHeight: 1.7, margin: 0,
                    overflowX: "auto", whiteSpace: "pre",
                  }}>
{`windowrulev2 = float, class:cue
windowrulev2 = pin, class:cue
windowrulev2 = noscreencast, class:cue
windowrulev2 = nofocus, class:cue, title:^$`}
                  </pre>
                </div>
                <div>
                  <p style={{ color: "var(--text-primary)", fontSize: 12, fontWeight: 600, margin: "0 0 8px" }}>Launch Command</p>
                  <pre style={{
                    background: "rgba(0,0,0,0.4)", border: "1px solid var(--divider)", borderRadius: 8,
                    padding: "10px 12px", fontSize: 11, color: "var(--cyan)", lineHeight: 1.7, margin: 0,
                    overflowX: "auto", whiteSpace: "pre",
                  }}>
{`GDK_BACKEND=x11 \\
  WEBKIT_DISABLE_COMPOSITING_MODE=1 \\
  WEBKIT_FORCE_SANDBOX=0 \\
  ./target/release/cue-overlay`}
                  </pre>
                </div>
                <div>
                  <p style={{ color: "var(--text-primary)", fontSize: 12, fontWeight: 600, margin: "0 0 8px" }}>Keyboard Shortcuts</p>
                  <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 11 }}>
                    {[["Ctrl+L", "Toggle microphone on/off"], ["Enter", "Send query to AI"], ["Escape", "Clear input"], ["Prompter icon", "Open teleprompter with cue cards"]].map(([k, v]) => (
                      <tr key={k} style={{ borderBottom: "1px solid var(--divider)" }}>
                        <td style={{ padding: "6px 0", color: "var(--accent)", fontFamily: "monospace", paddingRight: 16 }}>{k}</td>
                        <td style={{ padding: "6px 0", color: "var(--text-secondary)" }}>{v}</td>
                      </tr>
                    ))}
                  </table>
                </div>
              </div>
            )}
          </div>

          {/* Error bar */}
          {error && (
            <div style={{ padding: "6px 14px", background: "var(--danger-dim)", borderTop: "1px solid var(--danger-border)", color: "var(--danger)", fontSize: 11, lineHeight: 1.5, flexShrink: 0 }}>
              {error}
            </div>
          )}
        </div>
      </div>
    );
  }

  // ── Render: Prompter ─────────────────────────────────────────────────────────

  if (view === "prompter") {
    const isEditing = pEditId !== null || pAdding;

    return (
      <div style={{ height: "100vh", width: "100vw", padding: 8, display: "flex", flexDirection: "column" }}>
        <div className="glass" style={{ flex: 1, borderRadius: 18, display: "flex", flexDirection: "column", overflow: "hidden", minHeight: 0 }}>

          {/* Prompter title bar */}
          <div className="drag" style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "10px 14px", borderBottom: "1px solid var(--divider)", flexShrink: 0 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <button className="btn-icon no-drag" onClick={() => setView("chat")} title="Back to chat">
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
                  <polyline points="15 18 9 12 15 6" />
                </svg>
              </button>
              <div
                className={listening ? "dot-listening" : ""}
                style={{ width: 7, height: 7, borderRadius: "50%", background: listening ? "var(--danger)" : "rgba(255,255,255,0.2)", transition: "background 200ms ease", flexShrink: 0 }}
              />
              <span style={{ color: "var(--text-primary)", fontSize: 13, fontWeight: 600, letterSpacing: "0.03em" }}>Prompter</span>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <button
                className={`btn-icon no-drag ${pAutoScroll ? "active" : ""}`}
                onClick={() => { setPAutoScroll((p) => !p); setPActiveCard(0); }}
                title={pAutoScroll ? "Disable auto-scroll" : "Enable auto-scroll"}
                style={{ width: 28, height: 28 }}
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <polyline points="6 9 12 15 18 9" />
                </svg>
              </button>
              <button
                className={`btn-icon no-drag ${listening ? "listening" : ""}`}
                onClick={toggleListen}
                title="Ctrl+L"
                style={{ width: 28, height: 28 }}
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
                  <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
                  <line x1="12" y1="19" x2="12" y2="23" />
                  <line x1="8" y1="23" x2="16" y2="23" />
                </svg>
              </button>
              <button
                className="no-drag"
                onClick={() => { setPAdding(true); setPEditId(null); setPDraftTitle(""); setPDraftContent(""); }}
                style={{ fontSize: 11, padding: "4px 10px", borderRadius: 6, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer" }}
              >
                + Note
              </button>
            </div>
          </div>

          {/* Prompter cards area */}
          <div
            ref={prompterRef}
            className="prompter-scroll"
            style={{ flex: 1, overflowY: "auto", padding: "16px 16px", minHeight: 0, display: "flex", flexDirection: "column", gap: 12 }}
          >
            {pNotes.length === 0 && !isEditing ? (
              <div style={{ margin: "auto", textAlign: "center", padding: "40px 20px" }}>
                <p style={{ color: "var(--text-muted)", fontSize: 14, margin: "0 0 12px", lineHeight: 1.7 }}>
                  No cue cards yet.
                </p>
                <p style={{ color: "var(--text-muted)", fontSize: 12, margin: "0 0 16px" }}>
                  Add your talking points, script, or notes — they'll scroll as you speak.
                </p>
                <button
                  className="no-drag"
                  onClick={() => { setPAdding(true); setPDraftTitle(""); setPDraftContent(""); }}
                  style={{ fontSize: 12, padding: "8px 20px", borderRadius: 8, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer" }}
                >
                  Add your first note
                </button>
              </div>
            ) : (
              <>
                {pNotes.map((note, idx) => (
                  pEditId === note.id ? (
                    /* Inline edit form */
                    <div key={note.id} className="prompter-card editing" style={{ padding: "12px 14px", borderRadius: 12, border: "1px solid var(--accent-border)", background: "var(--accent-dim)" }}>
                      <input
                        className="glass-input no-drag"
                        value={pDraftTitle}
                        onChange={(e) => setPDraftTitle(e.target.value)}
                        placeholder="Title (optional)"
                        style={{ width: "100%", marginBottom: 8, fontSize: 13, fontWeight: 600 }}
                        autoFocus
                      />
                      <textarea
                        className="glass-input no-drag"
                        value={pDraftContent}
                        onChange={(e) => setPDraftContent(e.target.value)}
                        placeholder="Your talking point or note..."
                        style={{ width: "100%", minHeight: 80, resize: "vertical", padding: "8px 12px", fontSize: 13, lineHeight: 1.7, fontFamily: "inherit" }}
                      />
                      <div style={{ display: "flex", gap: 6, marginTop: 8, justifyContent: "flex-end" }}>
                        <button className="no-drag" onClick={cancelEdit} style={{ fontSize: 11, padding: "4px 10px", borderRadius: 6, border: "1px solid var(--divider)", background: "rgba(255,255,255,0.03)", color: "var(--text-muted)", cursor: "pointer" }}>Cancel</button>
                        <button className="no-drag" onClick={saveEditNote} style={{ fontSize: 11, padding: "4px 10px", borderRadius: 6, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer" }}>Save</button>
                      </div>
                    </div>
                  ) : (
                    /* Display card */
                    <div
                      key={note.id}
                      id={`pcard-${idx}`}
                      className={`prompter-card no-drag ${pActiveCard === idx && listening ? "active-card" : ""}`}
                      style={{
                        padding: "14px 16px",
                        borderRadius: 12,
                        border: `1px solid ${pActiveCard === idx && listening ? "var(--accent-border)" : "var(--divider)"}`,
                        background: pActiveCard === idx && listening ? "rgba(59, 130, 246, 0.08)" : "rgba(255,255,255,0.02)",
                        transition: "all 300ms ease",
                        cursor: "pointer",
                      }}
                      onClick={() => setPActiveCard(idx)}
                    >
                      <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 8 }}>
                        <div style={{ flex: 1, minWidth: 0 }}>
                          {note.title && (
                            <div style={{ color: "var(--accent)", fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}>
                              {note.title}
                            </div>
                          )}
                          <div style={{ color: "var(--text-primary)", fontSize: 15, lineHeight: 1.8, whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
                            {note.content}
                          </div>
                        </div>
                        <div style={{ display: "flex", flexDirection: "column", gap: 2, flexShrink: 0 }}>
                          {idx > 0 && (
                            <button className="btn-icon no-drag" onClick={(e) => { e.stopPropagation(); moveNote(idx, -1); }} style={{ width: 22, height: 22, fontSize: 10 }} title="Move up">
                              <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round"><polyline points="18 15 12 9 6 15"/></svg>
                            </button>
                          )}
                          {idx < pNotes.length - 1 && (
                            <button className="btn-icon no-drag" onClick={(e) => { e.stopPropagation(); moveNote(idx, 1); }} style={{ width: 22, height: 22, fontSize: 10 }} title="Move down">
                              <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
                            </button>
                          )}
                          <button className="btn-icon no-drag" onClick={(e) => { e.stopPropagation(); startEdit(note); }} style={{ width: 22, height: 22 }} title="Edit">
                            <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
                          </button>
                          <button className="btn-icon no-drag" onClick={(e) => { e.stopPropagation(); deletePrompterNote(note.id); }} style={{ width: 22, height: 22 }} title="Delete">
                            <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="var(--danger)" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                          </button>
                        </div>
                      </div>
                      <div style={{ color: "var(--text-muted)", fontSize: 10, marginTop: 6 }}>
                        {idx + 1} / {pNotes.length}
                      </div>
                    </div>
                  )
                ))}

                {/* Add new note form (appended at end) */}
                {pAdding && (
                  <div className="prompter-card editing" style={{ padding: "12px 14px", borderRadius: 12, border: "1px solid var(--accent-border)", background: "var(--accent-dim)" }}>
                    <input
                      className="glass-input no-drag"
                      value={pDraftTitle}
                      onChange={(e) => setPDraftTitle(e.target.value)}
                      placeholder="Title (optional)"
                      style={{ width: "100%", marginBottom: 8, fontSize: 13, fontWeight: 600 }}
                      autoFocus
                    />
                    <textarea
                      className="glass-input no-drag"
                      value={pDraftContent}
                      onChange={(e) => setPDraftContent(e.target.value)}
                      placeholder="Your talking point or note..."
                      style={{ width: "100%", minHeight: 80, resize: "vertical", padding: "8px 12px", fontSize: 13, lineHeight: 1.7, fontFamily: "inherit" }}
                    />
                    <div style={{ display: "flex", gap: 6, marginTop: 8, justifyContent: "flex-end" }}>
                      <button className="no-drag" onClick={cancelEdit} style={{ fontSize: 11, padding: "4px 10px", borderRadius: 6, border: "1px solid var(--divider)", background: "rgba(255,255,255,0.03)", color: "var(--text-muted)", cursor: "pointer" }}>Cancel</button>
                      <button className="no-drag" onClick={addPrompterNote} style={{ fontSize: 11, padding: "4px 10px", borderRadius: 6, border: "1px solid var(--accent-border)", background: "var(--accent-dim)", color: "var(--accent)", cursor: "pointer" }}>Add</button>
                    </div>
                  </div>
                )}
              </>
            )}
          </div>

          {/* Compact transcript strip */}
          <div style={{ padding: "6px 14px", borderTop: "1px solid var(--divider)", maxHeight: 56, overflowY: "auto", flexShrink: 0 }}>
            {transcript.length === 0 ? (
              <p style={{ color: "var(--text-muted)", fontSize: 10, margin: 0 }}>No audio yet — Ctrl+L to listen</p>
            ) : (
              transcript.slice(-3).map((line, i) => (
                <div key={i} style={{ fontSize: 10, lineHeight: 1.5 }}>
                  <span style={{ color: chColor(line.channel), fontWeight: 600, marginRight: 4, fontSize: 9 }}>[{chLabel(line.channel)}]</span>
                  <span style={{ color: "var(--text-secondary)" }}>{line.text}</span>
                </div>
              ))
            )}
          </div>

          {/* Compact input bar */}
          <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "6px 10px", borderTop: "1px solid var(--divider)", flexShrink: 0 }}>
            <input
              className="glass-input no-drag"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="Ask AI..."
              style={{ height: 28, fontSize: 12 }}
            />
            <button
              className={`btn-icon no-drag ${input.trim() ? "active" : ""}`}
              onClick={sendQuery}
              disabled={!input.trim()}
              title="Send (Enter)"
              style={{ width: 28, height: 28 }}
            >
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
                <line x1="22" y1="2" x2="11" y2="13" />
                <polygon points="22 2 15 22 11 13 2 9 22 2" />
              </svg>
            </button>
          </div>

          {/* Last AI response preview */}
          {conv.length > 0 && conv[conv.length - 1].response && (
            <div style={{
              padding: "6px 14px", borderTop: "1px solid var(--divider)",
              maxHeight: 64, overflowY: "auto", flexShrink: 0,
              background: "rgba(255,255,255,0.02)",
            }}>
              <div style={{ fontSize: 10, color: "var(--accent)", fontWeight: 600, marginBottom: 2 }}>AI</div>
              <div style={{ fontSize: 11, color: "var(--text-primary)", lineHeight: 1.5, whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
                {conv[conv.length - 1].response.slice(0, 300)}
                {conv[conv.length - 1].response.length > 300 && "…"}
                {conv[conv.length - 1].streaming && <span className="cursor" style={{ color: "var(--accent)", marginLeft: 1 }}>▋</span>}
              </div>
            </div>
          )}

          {/* Error bar */}
          {error && (
            <div style={{ padding: "6px 14px", background: "var(--danger-dim)", borderTop: "1px solid var(--danger-border)", color: "var(--danger)", fontSize: 11, lineHeight: 1.5, flexShrink: 0 }}>
              {error}
            </div>
          )}
        </div>
      </div>
    );
  }

  // ── Render: Chat ───────────────────────────────────────────────────────────

  return (
    <div style={{ height: "100vh", width: "100vw", padding: 8, display: "flex", flexDirection: "column" }}>
      <div className="glass" style={{ flex: 1, borderRadius: 18, display: "flex", flexDirection: "column", overflow: "hidden", minHeight: 0 }}>

        {/* Title bar — the parent is draggable; only buttons opt out */}
        <div className="drag" style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "10px 14px", borderBottom: "1px solid var(--divider)", flexShrink: 0 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8, pointerEvents: "none" }}>
            <div
              className={listening ? "dot-listening" : ""}
              style={{ width: 7, height: 7, borderRadius: "50%", background: listening ? "var(--danger)" : "rgba(255,255,255,0.2)", transition: "background 200ms ease", flexShrink: 0 }}
            />
            <span style={{ color: "var(--text-primary)", fontSize: 13, fontWeight: 600, letterSpacing: "0.03em" }}>cue</span>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <span style={{ color: listening ? "var(--danger)" : "var(--text-muted)", fontSize: 11, maxWidth: 180, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", transition: "color 200ms ease" }}>
              {listening ? "listening..." : error ? "error" : status}
            </span>
            <button className="btn-icon no-drag" onClick={openPrompter} title="Prompter">
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
                <line x1="8" y1="21" x2="16" y2="21" />
                <line x1="12" y1="17" x2="12" y2="21" />
              </svg>
            </button>
            <button className="btn-icon no-drag" onClick={openSettings} title="Settings">
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <circle cx="12" cy="12" r="3" />
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
              </svg>
            </button>
          </div>
        </div>

        {/* Transcript band */}
        <div ref={transcriptRef} style={{ padding: "6px 14px", borderBottom: "1px solid var(--divider)", maxHeight: 72, minHeight: 28, overflowY: "auto", flexShrink: 0 }}>
          {transcript.length === 0 ? (
            <p style={{ color: "var(--text-muted)", fontSize: 11, margin: 0, lineHeight: 1.5 }}>No audio yet — Ctrl+L to listen</p>
          ) : (
            transcript.slice(-5).map((line, i) => (
              <div key={i} style={{ fontSize: 11, lineHeight: 1.6 }}>
                <span style={{ color: chColor(line.channel), fontWeight: 600, marginRight: 5, fontSize: 10, letterSpacing: "0.04em" }}>[{chLabel(line.channel)}]</span>
                <span style={{ color: "var(--text-primary)" }}>{line.text}</span>
              </div>
            ))
          )}
        </div>

        {/* Conversation area */}
        <div
          ref={convContainerRef}
          onScroll={handleConvScroll}
          style={{ flex: 1, overflowY: "auto", padding: "12px 14px", minHeight: 0, display: "flex", flexDirection: "column", gap: 16 }}
        >
          {conv.length === 0 ? (
            <p style={{ color: "var(--text-muted)", fontSize: 12, margin: "auto", textAlign: "center", lineHeight: 1.7 }}>
              Ask a question or press Ctrl+L<br />to start listening
            </p>
          ) : (
            conv.map((entry, i) => (
              <div key={i} style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {/* User query */}
                <div style={{ display: "flex", justifyContent: "flex-end" }}>
                  <div style={{
                    maxWidth: "85%", padding: "7px 12px", borderRadius: "12px 12px 3px 12px",
                    background: "var(--accent-dim)", border: "1px solid var(--accent-border)",
                    color: "var(--text-primary)", fontSize: 12, lineHeight: 1.6, wordBreak: "break-word",
                  }}>
                    {entry.query}
                  </div>
                </div>
                {/* AI response */}
                {(entry.response || entry.streaming) && (
                  <div style={{
                    maxWidth: "95%", padding: "8px 12px", borderRadius: "3px 12px 12px 12px",
                    background: "rgba(255,255,255,0.03)", border: "1px solid var(--divider)",
                    color: "var(--text-primary)", fontSize: 12.5, lineHeight: 1.8,
                    whiteSpace: "pre-wrap", wordBreak: "break-word",
                  }}>
                    {entry.response || <span style={{ color: "var(--text-muted)" }}>Thinking…</span>}
                    {entry.streaming && (
                      <span className="cursor" style={{ color: "var(--accent)", marginLeft: 1 }}>▋</span>
                    )}
                  </div>
                )}
              </div>
            ))
          )}
          <div ref={convEndRef} />
        </div>

        {/* Input bar */}
        <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "8px 10px", borderTop: "1px solid var(--divider)", flexShrink: 0 }}>
          <button className={`btn-icon no-drag ${listening ? "listening" : ""}`} onClick={toggleListen} title="Ctrl+L">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
              <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
              <line x1="12" y1="19" x2="12" y2="23" />
              <line x1="8" y1="23" x2="16" y2="23" />
            </svg>
          </button>
          <input
            className="glass-input no-drag"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Ask anything..."
            autoFocus
          />
          <button
            className={`btn-icon no-drag ${input.trim() ? "active" : ""}`}
            onClick={sendQuery}
            disabled={!input.trim()}
            title="Send (Enter)"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
              <line x1="22" y1="2" x2="11" y2="13" />
              <polygon points="22 2 15 22 11 13 2 9 22 2" />
            </svg>
          </button>
        </div>

        {/* Error toast */}
        {error && (
          <div style={{ padding: "6px 14px", background: "var(--danger-dim)", borderTop: "1px solid var(--danger-border)", color: "var(--danger)", fontSize: 11, lineHeight: 1.5, flexShrink: 0 }}>
            {error}
          </div>
        )}
      </div>
    </div>
  );
}
