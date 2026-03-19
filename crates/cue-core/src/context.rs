use anyhow::Result;
use std::sync::Arc;
use tracing::debug;

use crate::config::RagConfig;
use crate::kb::KnowledgeBase;
use crate::llm::Message;
use crate::session::TranscriptEntry;

/// Approximate token count from character count (1 token ≈ 4 chars)
fn approx_tokens(s: &str) -> usize {
    s.len() / 4
}

/// Context engine — assembles the LLM prompt from:
/// 1. System prompt
/// 2. KB context (RAG retrieval)
/// 3. Recent transcript window
/// 4. User query
pub struct ContextEngine {
    kb: Arc<KnowledgeBase>,
    config: RagConfig,
}

impl ContextEngine {
    pub fn new(kb: Arc<KnowledgeBase>, config: RagConfig) -> Self {
        Self { kb, config }
    }

    /// Build the full message list for the LLM
    pub async fn build_prompt(
        &self,
        query: &str,
        transcript: &[TranscriptEntry],
        system_prompt: &str,
    ) -> Result<Vec<Message>> {
        // Token budget: 1500 tokens ≈ 6000 characters
        const TOKEN_BUDGET: usize = 6000;
        let mut budget = TOKEN_BUDGET;

        let mut messages = Vec::new();

        // 1. System message
        let system_msg = system_prompt.to_string();
        budget = budget.saturating_sub(approx_tokens(&system_msg));
        messages.push(Message::system(system_msg));

        // 2. KB context (if enabled and documents exist)
        let mut kb_context = String::new();
        if self.config.enabled && !query.is_empty() {
            let chunks = tokio::task::spawn_blocking({
                let kb = Arc::clone(&self.kb);
                let query = query.to_string();
                let top_k = self.config.top_k;
                move || kb.search(&query, top_k)
            })
            .await??;

            if !chunks.is_empty() {
                kb_context.push_str("## Relevant context from your documents:\n\n");
                for (i, chunk) in chunks.iter().enumerate() {
                    let chunk_text = format!(
                        "[{}] (from: {})\n{}\n\n",
                        i + 1,
                        chunk.doc_name,
                        chunk.content
                    );
                    if approx_tokens(&kb_context) + approx_tokens(&chunk_text) > budget / 2 {
                        break;
                    }
                    kb_context.push_str(&chunk_text);
                }
                budget = budget.saturating_sub(approx_tokens(&kb_context));
                debug!(kb_chars = kb_context.len(), "KB context assembled");
            }
        }

        // 3. Recent transcript
        let mut transcript_text = String::new();
        if !transcript.is_empty() {
            transcript_text.push_str("## Recent conversation transcript:\n\n");

            // Take the most recent entries within budget
            let entries: Vec<_> = transcript.iter().rev().take(50).collect();
            let mut transcript_lines: Vec<String> = entries
                .iter()
                .rev()
                .map(|e| {
                    let label = if e.channel == "system" { "[THEM]" } else { "[YOU]" };
                    format!("{} {}", label, e.text)
                })
                .collect();

            // Trim to budget
            while !transcript_lines.is_empty() {
                let text: String = transcript_lines.join("\n");
                if approx_tokens(&text) <= budget / 2 {
                    transcript_text.push_str(&text);
                    break;
                }
                transcript_lines.remove(0);
            }
            let _ = budget.saturating_sub(approx_tokens(&transcript_text));
        }

        // Build user message combining KB context + transcript + query
        let mut user_content = String::new();

        if !kb_context.is_empty() {
            user_content.push_str(&kb_context);
            user_content.push('\n');
        }

        if !transcript_text.is_empty() {
            user_content.push_str(&transcript_text);
            user_content.push_str("\n\n");
        }

        user_content.push_str("## Question:\n");
        user_content.push_str(query);

        messages.push(Message::user(user_content));

        Ok(messages)
    }
}
