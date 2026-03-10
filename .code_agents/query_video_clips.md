# Improved Design for query_video_clips Tool

## GOAL & Requirements (Improved)
Companion to ingest: allows semantic/hybrid search over video_clip memories.

**Requirements**:
- Query text for hybrid recall (leverages SqliteMemory's FTS + vector).
- Filters: exclude_risky (parse frontmatter from content), video_id, topics (post-filter since trait recall doesn't support advanced metadata filters - YAGNI to extend trait yet), min_duration.
- Return structured ClipMatch list with parsed metadata from content frontmatter.
- Post-filter results from `memory.recall(query, limit*2, None)` for efficiency.
- Structured output for agent to use in clipper workflow.

**Feasibility**: High. Uses existing recall + post-processing (simple string parse for frontmatter). Hybrid search already in sqlite. Post-filter is acceptable per KISS (avoid trait changes).

**Best practices**: SRP (query only, no store), security (read only), explicit parsing errors, limit results, update docs.

**Non-goals**: Native metadata filtering in memory (add later if rule-of-three), pagination.

## High Level Solution
Follow same as improved ingest.

1. Create `src/tools/query_video_clips.rs`

```rust
// ...existing imports...
use crate::memory::{Memory, MemoryCategory};
use crate::security::SecurityPolicy;
// ...

pub struct QueryVideoClipsTool {
    memory: Arc<dyn Memory>,
    security: Arc<SecurityPolicy>,
}

#[async_trait]
impl Tool for QueryVideoClipsTool {
    // name, description, parameters_schema (update to match current trait)
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let query = args.get("query_text").and_then(|v| v.as_str()).ok_or(...) ?;
        let results = self.memory.recall(query, 20, None).await?;
        let filtered = self.filter_video_clips(&results, &args); // parse frontmatter, apply filters
        // build ClipMatch vec by parsing YAML frontmatter from content
        // ...
        Ok(ToolResult { success: true, output: serde_json::to_string(&matches)? , error: None })
    }
}
```

2. Register similarly in mod.rs all_tools_with_runtime.

3. Test with recall on video_clip contents.

This pairs well with ingest for end-to-end video knowledge workflow.
pub struct QueryResult {
    pub matches: Vec<ClipMatch>,
    pub total_found: usize,
    pub message: String,
}

// ── The tool ────────────────────────────────────────────────────────────────────

pub struct QueryVideoClips;

#[async_trait]
impl Tool for QueryVideoClips {
    fn name(&self) -> &'static str {
        "query_video_clips"
    }

    fn description(&self) -> &'static str {
        "Search across all ingested video clips using semantic/hybrid query. Filter by topics, risk flags (e.g. safe for YouTube), specific video, duration, etc. Returns ranked clips with timestamps and metadata — ideal for selecting content to extract into new videos."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query_text": { "type": "string", "description": "Main search query (semantic + keywords)" },
                "max_results": { "type": "integer", "description": "Max clips to return (default 10)", "default": 10 },
                "exclude_risky": { "type": "boolean", "description": "Exclude clips flagged for potential demonetization (default true)", "default": true },
                "include_topics": { "type": "array", "items": { "type": "string" }, "description": "Only include clips with these topics" },
                "exclude_topics": { "type": "array", "items": { "type": "string" }, "description": "Exclude clips with these topics" },
                "video_id": { "type": "string", "description": "Limit to a specific video ID" },
                "min_duration_sec": { "type": "number", "description": "Minimum clip length in seconds" }
            },
            "required": ["query_text"]
        })
    }

    async fn call(&self, call: ToolCall, ctx: &crate::agent::AgentContext) -> ToolResult {
        let params: QueryParams = serde_json::from_value(call.arguments)
            .context("Invalid arguments for query_video_clips")?;

        let max_results = params.max_results.unwrap_or(10);
        let exclude_risky = params.exclude_risky.unwrap_or(true);

        // Build query options for ZeroClaw memory
        let mut options = MemoryQueryOptions {
            limit: max_results,
            // hybrid weights if supported (e.g. 0.7 vector + 0.3 keyword)
            ..Default::default()
        };

        // Optional: add filters via metadata predicates (ZeroClaw likely supports this)
        // Adapt based on your memory backend's capabilities (sqlite, lucid, etc.)
        let mut metadata_filters = Vec::new();

        if exclude_risky {
            metadata_filters.push(("demonetization_flag".to_string(), json!(false)));
        }

        if let Some(video_id) = &params.video_id {
            metadata_filters.push(("video_id".to_string(), json!(video_id)));
        }

        if let Some(topics) = &params.include_topics {
            // Depending on backend: might need custom filter logic or array contains
            // For sqlite: could use JSON_EXTRACT or custom query
            // Simplest: post-filter below
        }

        // Execute hybrid recall/search
        let entries = ctx.memory
            .search_hybrid(&params.query_text, &options)  // or .recall() if no hybrid
            .await
            .context("Memory search failed")?;

        // Post-filter (safe fallback if backend doesn't support advanced metadata filtering)
        let mut matches = Vec::new();

        for entry in entries {
            let meta = &entry.metadata.extra;

            let demonetization_flag = meta
                .get("demonetization_flag")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if exclude_risky && demonetization_flag {
                continue;
            }

            let video_id_match = match (&params.video_id, meta.get("video_id").and_then(|v| v.as_str())) {
                (Some(want), Some(have)) => want == have,
                (None, _) => true,
                _ => false,
            };
            if !video_id_match {
                continue;
            }

            // Topic include/exclude (simple contains check)
            let topics: HashSet<String> = meta
                .get("topics")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            if let Some(include) = &params.include_topics {
                if !include.iter().any(|t| topics.contains(t)) {
                    continue;
                }
            }

            if let Some(exclude) = &params.exclude_topics {
                if exclude.iter().any(|t| topics.contains(t)) {
                    continue;
                }
            }

            // Duration filter
            let start = meta.get("start_ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let end = meta.get("end_ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let duration = end - start;

            if let Some(min_dur) = params.min_duration_sec {
                if duration < min_dur {
                    continue;
                }
            }

            matches.push(ClipMatch {
                clip_id: entry.id.unwrap_or_default(),  // adapt to your MemoryEntry ID field
                video_id: meta.get("video_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                start_ts: start,
                end_ts: end,
                content: entry.content.clone(),
                topics: topics.into_iter().collect(),
                demonetization_flag,
                risk_reason: meta.get("risk_reason").and_then(|v| v.as_str()).map(String::from),
                score: entry.score.unwrap_or(0.0),  // if backend provides relevance score
            });
        }

        // Sort by score descending if available
        matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // Trim to max_results (in case post-filtering reduced count)
        if matches.len() > max_results {
            matches.truncate(max_results);
        }

        ToolResult::success(json!(QueryResult {
            matches,
            total_found: matches.len(),
            message: format!("Found {} matching clips (filtered from {} raw results)", matches.len(), entries.len()),
        }))
    }
}
2. Register the tool
In src/tools/mod.rs (or your registry file):
Rustpub mod query_video_clips;
pub use query_video_clips::QueryVideoClips;

// In register_builtin_tools or similar:
registry.register(QueryVideoClips);
3. Rebuild
Bashcargo build --release
# or cargo run --release -- ...
4. How the agent uses it (examples)
In chat / prompt:
query_video_clips with query_text="local LLM performance optimization" exclude_risky=true include_topics=["Rust", "AI"] max_results=8
Or more restrictive:
query_video_clips query_text="whisper.cpp vs faster-whisper benchmarks" video_id="yt_abc123" exclude_risky=true
The tool returns structured JSON → agent can read it, summarize, or chain to another tool (e.g. "extract these clips using ffmpeg").
5. Enhancements you might want

Better filtering — if your memory backend (sqlite/lucid/etc.) supports JSON queries or custom predicates, move filtering into MemoryQueryOptions instead of post-filtering.
Score threshold — add min_score param and filter on entry.score.
Pagination — add offset param if needed for large result sets.
Output formatting — if you want human-readable text output instead of JSON, add a format param (json/text/markdown).
Full-text boost — if hybrid search isn't default, tweak weights in options.