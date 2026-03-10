
# Improved Design for ingest_video_transcription Tool (Updated March 2026)

## GOAL (Aligned with AGENTS.md, KISS/YAGNI/SRP)
Add `ingest_video_transcription` Tool implementing current `src/tools/traits::Tool` (parameters_schema + execute).

**Requirements** (Quality-Focused Update):
- Inputs: `input_path` (transcription file md or srt), optional `video_id`, `max_clip_duration_sec` (~90s default), `force`, `rules_path` (default `docs/video_classification_rules.md` for easy refinement).
- Parse whisper srt segments (reuse `audio_transcribe.rs` patterns).
- **Rule-based chunking** (time windows + pause/silence detection from whisper segments - reliable, fast, no tokens).
- **Classification/Sentiment/Topics/Pros-Cons**: Load structured rules from `docs/video_classification_rules.md` (newly created; sections `## Topics`, `## Sentiment Indicators`, `## Pros`, `## Cons/Risks`, `## Best Practices`). Use **existing `EmbeddingProvider`** (from memory or injected) for cheap similarity: embed chunk vs rule phrases/examples; cosine > 0.65 threshold = classify (topics list, sentiment, pros/cons flags, risk). Keyword fallback for speed. No heavy LLM reasoning - classification only. Rules MD is living doc for refinement (health/weight loss focus, YouTube monetization risks, before/after disclaimers, misleading claims like "lose weight without diet").
- Store via `memory.store(...)` with YAML frontmatter in `content` (video_id, start/end, topics: [], sentiment: "positive", pros: [], cons: [], risk: false, risk_reason: "...", score).
- Idempotent/resumable: `memory.get(key)` check.
- Secure: `SecurityPolicy`, path validation (rules_path + input_path in workspace), explicit errors.
- Local-first, efficient (<5min/1h video).

**Feasibility**: Very high. Embeddings already in `memory/embeddings.rs` + `sqlite.rs` (reuse `NoopEmbedding` fallback or local). MD parsing = simple string split/regex (no new deps). Quality much better than pure keywords while KISS (no per-chunk LLM chat). Cheap embed calls scale well.

**Non-goals**: Full LLM reasoning per chunk, new Classifier trait (YAGNI until rule-of-3), complex MD parser.

**Best practices**: AGENTS.md (KISS rule-based chunk + embed classify, load rules dynamically, SRP, secure-by-default, no provider coupling in core path, test classification thresholds). Aligns with style-guide (error handling, no unsafe).

## High Level Solution (Current Architecture Compliant)
Follow AGENTS.md playbook: read existing (audio_transcribe.rs, memory_store.rs, sqlite.rs), minimal change, register in `all_tools_with_runtime`, security first.

**Step 1: New file `src/tools/ingest_video_transcription.rs`**

```rust
// ...existing code...
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;

use super::traits::{Tool, ToolResult};
use crate::memory::{Memory, MemoryCategory};
use crate::security::SecurityPolicy;
use crate::security::policy::ToolOperation;

// Struct with injected deps (like AudioTranscribeTool, MemoryStoreTool)
#[derive(Debug)]
pub struct IngestVideoTranscriptionTool {
    memory: Arc<dyn Memory>,
    security: Arc<SecurityPolicy>,
    workspace_dir: PathBuf,
}

impl IngestVideoTranscriptionTool {
    pub fn new(
        memory: Arc<dyn Memory>,
        security: Arc<SecurityPolicy>,
        workspace_dir: PathBuf,
    ) -> Self {
        Self { memory, security, workspace_dir }
    }

    // Helpers: parse_whisper_json, rule_based_chunk (group by time/pause), 
    // extract_topics_keywords, is_demonetization_risk (keyword list), 
    // format_clip_markdown (with YAML frontmatter)
    async fn parse_and_store_clips(&self, input_path: &PathBuf, video_id: &str, max_duration: f64, force: bool) -> Result<usize> {
        // Implementation details...
        // - Security check path
        // - Read/parse JSON
        // - Chunk segments
        // - For each: build content with frontmatter, key = format!("video_clip_{}_{}", video_id, start)
        // - if !force && self.memory.get(&key).await?.is_some() { skip }
        // - self.memory.store(&key, &content, MemoryCategory::Custom("video_clip".to_string()), None).await?;
        Ok(0)
    }
}

#[async_trait]
impl Tool for IngestVideoTranscriptionTool {
    fn name(&self) -> &str { "ingest_video_transcription" }

    fn description(&self) -> &str {
        "Ingests video transcript JSON, applies rule-based chunking into timestamped clips, adds topic/risk metadata via frontmatter, stores in memory under 'video_clip' category for hybrid semantic search."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "input_path": {"type": "string", "description": "Path to whisper JSON or media file"},
                "video_id": {"type": "string", "description": "Video identifier"},
                "max_clip_duration": {"type": "number", "default": 90, "description": "Max clip length in seconds"},
                "force": {"type": "boolean", "default": false, "description": "Overwrite existing"}
            },
            "required": ["input_path"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let input_path_str = args.get("input_path").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing input_path"))?;
        let input_path = PathBuf::from(input_path_str);
        // validate path with security...
        if let Err(e) = self.security.enforce_tool_operation(ToolOperation::Act, self.name()) {
            return Ok(ToolResult { success: false, output: "".into(), error: Some(e) });
        }
        let video_id = args.get("video_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let max_dur = args.get("max_clip_duration").and_then(|v| v.as_f64()).unwrap_or(90.0);
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

        let stored = self.parse_and_store_clips(&input_path, &video_id, max_dur, force).await?;

        Ok(ToolResult {
            success: true,
            output: format!("Successfully ingested {} clips for video {}", stored, video_id),
            error: None,
        })
    }
}
```

**Notes**: 
- Implement helpers following `audio_transcribe.rs` style (timeout, models if needed).
- Use `MemoryCategory::Custom` for video clips.
- Content frontmatter enables parsing in query tool.
- Add `youtube_guidelines.md` keywords for risk detection.

// Helper structs & logic (expand as needed)
#[derive(Debug)]
struct ClipChunk {
    start: f64,
    end: f64,
    topic: String,
    summary: String,
    topics: Vec<String>,
    demonetization_flag: bool,
    risk_reason: Option<String>,
}

impl IngestVideoTranscription {
    async fn split_into_topic_clips(
        &self,
        segments: &[Value],
        lang: Option<&str>,
        ctx: &crate::agent::AgentContext,
    ) -> Result<Vec<ClipChunk>> {
        // Build prompt from segments
        let mut transcript_snippet = String::new();
        for seg in segments.iter().take(50) {  // limit for prompt size
            if let (Some(start), Some(end), Some(text)) = (
                seg["start"].as_f64(),
                seg["end"].as_f64(),
                seg["text"].as_str(),
            ) {
                transcript_snippet.push_str(&format!("[{:.1}-{:.1}] {}\n", start, end, text));
            }
        }

        let prompt = format!(
            r#"
Split this video transcript into 5-20 topic-based clips.
Output ONLY valid JSON array of objects with keys:
- start: number (seconds)
- end: number (seconds)
- topic: short title string
- summary: 1-sentence summary
- topics: array of 1-3 tags

Transcript (language hint: {}):
{}
"#,
            lang.unwrap_or("auto"),
            transcript_snippet
        );

        // Call your configured provider (e.g. Ollama, OpenAI, etc.)
        let response = ctx.provider
            .chat(/* model from config */, &prompt, /* options */)
            .await
            .map_err(ProviderError::from)?;

        let json_str = response.content();  // adapt based on your Provider trait response

        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json_str)
            .context("LLM did not return valid JSON array")?;

        let mut chunks = Vec::new();
        for item in parsed {
            let demonetization_flag = self.is_risky_content(item["summary"].as_str().unwrap_or(""));
            let risk_reason = if demonetization_flag { Some("Potential violation detected".to_string()) } else { None };

            chunks.push(ClipChunk {
                start: item["start"].as_f64().unwrap_or(0.0),
                end: item["end"].as_f64().unwrap_or(0.0),
                topic: item["topic"].as_str().unwrap_or("Untitled").to_string(),
                summary: item["summary"].as_str().unwrap_or("").to_string(),
                topics: item["topics"].as_array().map_or(vec![], |a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()),
                demonetization_flag,
                risk_reason,
            });
        }

        Ok(chunks)
    }

    fn is_risky_content(&self, text: &str) -> bool {
        let risky_keywords = ["violence", "hate", "controversy", "medical claim", "brand attack"];
        risky_keywords.iter().any(|&k| text.to_lowercase().contains(k))
    }
}
Step 2: Register the tool
In src/tools/mod.rs (or wherever tools are registered):
Rustpub mod ingest_video_transcription;
pub use ingest_video_transcription::IngestVideoTranscription;

// In the tool registry function (likely src/tools/registry.rs or main init)
pub fn register_builtin_tools(registry: &mut ToolRegistry) {
    // ... existing tools ...
    registry.register(IngestVideoTranscription);
}
Step 3: Rebuild & test