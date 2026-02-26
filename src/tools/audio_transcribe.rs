use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::fs;
use tokio::process::Command;
use super::traits::{Tool, ToolResult};

#[derive(Debug, Clone)]
pub struct AudioTranscribeTool;

#[async_trait]
impl Tool for AudioTranscribeTool {
    fn name(&self) -> &str { "audio_transcribe" }

    fn description(&self) -> &str {
        "Transcribes audio from local file or YouTube URL using local faster-whisper (preferred, offline, fast) or OpenAI Whisper API fallback. Supports timestamps, SRT/VTT, initial prompt. Ideal for voice notes, meetings, podcasts."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "description": "Local audio path or YouTube URL (required)" },
                "model": { "type": "string", "default": "auto", "description": "faster-whisper model or OpenAI model" },
                "language": { "type": "string", "default": "auto" },
                "format": { "type": "string", "enum": ["text", "json", "srt", "vtt"], "default": "text" },
                "word_timestamps": { "type": "boolean", "default": false },
                "initial_prompt": { "type": "string" },
                "output_dir": { "type": "string", "description": "Optional output dir" }
            },
            "required": ["input"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let input = args["input"].as_str().context("Missing 'input'")?.to_string();

        // Early check: prefer local faster-whisper
        let use_local = Command::new("python3")
            .arg("-m")
            .arg("faster_whisper")
            .arg("--version")
            .output()
            .await
            .is_ok();

        if !use_local && std::env::var("OPENAI_API_KEY").is_err() {
            return Ok(ToolResult {
                success: false,
                output: "".to_string(),
                error: Some("Neither faster-whisper nor OPENAI_API_KEY is available. Install faster-whisper or set OPENAI_API_KEY env var.".to_string()),
            });
        }

        let model = args["model"].as_str().unwrap_or("auto").to_string();
        let language = args["language"].as_str().unwrap_or("auto").to_string();
        let format = args["format"].as_str().unwrap_or("text").to_string();
        let word_timestamps = args["word_timestamps"].as_bool().unwrap_or(false);
        let initial_prompt = args["initial_prompt"].as_str().map(|s| s.to_string());
        let output_dir = if let Some(d) = args["output_dir"].as_str() {
            PathBuf::from(d)
        } else {
            std::env::current_dir()?.join("downloads/transcripts")
        };
        fs::create_dir_all(&output_dir).await.ok();

        // 1. If URL → download audio only (reuse yt-dlp logic)
        let audio_path = if input.starts_with("http") {
            let temp_audio = std::env::temp_dir().join(format!("yt_audio_{}.mp3", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()));
            let status = Command::new("yt-dlp")
                .arg("--extract-audio")
                .arg("--audio-format").arg("mp3")
                .arg("-o").arg(temp_audio.to_str().unwrap())
                .arg("--no-playlist")
                .arg(&input)
                .status().await?;
            if !status.success() {
                return Ok(ToolResult { success: false, output: "".to_string(), error: Some("Failed to download audio from URL".to_string()) });
            }
            temp_audio
        } else {
            PathBuf::from(&input)
        };

        // 2. Transcribe
        let result = if Command::new("faster-whisper").arg("--version").output().await.is_ok() {
            self.transcribe_local(&audio_path, &model, &language, &format, word_timestamps, initial_prompt.as_deref(), &output_dir).await?
        } else {
            self.transcribe_openai(&audio_path, &model, &language, &format, word_timestamps, initial_prompt.as_deref()).await?
        };

        // 3. Cleanup temporary audio if downloaded from URL
        if input.starts_with("http") {
            let _ = fs::remove_file(&audio_path).await;
        }

        Ok(result)
    }
}

impl AudioTranscribeTool {
    async fn transcribe_local(
        &self,
        audio_path: &PathBuf,
        model: &str,
        language: &str,
        format: &str,
        word_timestamps: bool,
        initial_prompt: Option<&str>,
        output_dir: &PathBuf,
    ) -> Result<ToolResult> {
        let mut cmd = Command::new("python3");
        cmd.arg("-m").arg("faster_whisper")
        .arg(audio_path.to_str().context("Invalid audio path")?)
        .arg("--model").arg(if model == "auto" { "distil-large-v3.5" } else { model })
        .arg("--language").arg(language)
        .arg("--format").arg(format)
        .arg("--output_dir").arg(output_dir.to_str().unwrap());

        if word_timestamps {
            cmd.arg("--word-timestamps");
        }
        if let Some(p) = initial_prompt {
            cmd.arg("--initial-prompt").arg(p);
        }

        let output = cmd.output().await.context("faster-whisper execution failed")?;

        let transcript = if output.status.success() {
            String::from_utf8_lossy(&output.stdout).to_string().trim().to_string()
        } else {
            String::from_utf8_lossy(&output.stderr).to_string()
        };

        // Collect output files asynchronously
        let mut files = Vec::new();
        if let Ok(mut entries) = fs::read_dir(output_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                files.push(entry.path().to_string_lossy().to_string());
            }
        }

        Ok(ToolResult {
            success: output.status.success(),
            output: json!({
                "transcript": transcript,
                "language": language,
                "model": model,
                "files": files
            }).to_string(),
            error: if output.status.success() {
                None
            } else {
                Some(String::from_utf8_lossy(&output.stderr).trim().to_string())
            },
        })
    }

    async fn transcribe_openai(&self, audio_path: &PathBuf, model: &str, language: &str, format: &str, word_timestamps: bool, initial_prompt: Option<&str>) -> Result<ToolResult> {
        let api_key = std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY not set for OpenAI fallback")?;
        let client = reqwest::Client::new();

        let mut form = reqwest::multipart::Form::new()
            .file("file", audio_path).await?
            .text("model", model.to_string());  // clone to owned String

        if language != "auto" {
            form = form.text("language", language.to_string());
        }
        if let Some(p) = initial_prompt {
            form = form.text("prompt", p.to_string());
        }
        if format == "verbose_json" || word_timestamps {
            form = form.text("response_format", "verbose_json".to_string());
        }

        let res = client.post("https://api.openai.com/v1/audio/transcriptions")
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send().await?;

        let json: Value = res.json().await?;

        Ok(ToolResult {
            success: true,
            output: json.to_string(),
            error: None,
        })
    }
}

// =============================================================================
// TESTS with full cleanup
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use tokio::fs;

    struct TestDirGuard(PathBuf);

    impl Drop for TestDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn test_output_dir() -> (PathBuf, TestDirGuard) {
        let dir = std::env::temp_dir().join("zeroclaw_transcribe_test");
        fs::create_dir_all(&dir).await.ok();

        // Clone BEFORE moving into guard
        let dir_for_guard = dir.clone();

        (dir, TestDirGuard(dir_for_guard))
    }

    const TEST_VIDEO: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";  
    const TEST_VIDEO_SUBS: &str = "https://www.youtube.com/watch?v=3tmd-ClpJxA";  
 
    #[tokio::test]
    async fn test_audio_transcribe_youtube_default() {
        let (dir, _guard) = test_output_dir().await;
        let tool = AudioTranscribeTool;
        let res = tool.execute(json!({
            "input": TEST_VIDEO,
            "output_dir": dir.to_string_lossy().to_string()
        })).await.unwrap();

        if !res.success {
            // Acceptable if no backend
            assert!(res.error.as_ref().unwrap().contains("faster-whisper") || res.error.as_ref().unwrap().contains("OPENAI_API_KEY"));
            return;
        }

        let output: Value = serde_json::from_str(&res.output).unwrap();
        assert!(!output["transcript"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_audio_transcribe_with_timestamps() {
        let (dir, _guard) = test_output_dir().await;
        let tool = AudioTranscribeTool;
        let res = tool.execute(json!({
            "input": TEST_VIDEO_SUBS,
            "word_timestamps": true,
            "format": "json",
            "output_dir": dir.to_string_lossy().to_string()
        })).await.unwrap();

        if !res.success {
            // Graceful handling: video unavailable is acceptable (transient)
            let err = res.error.unwrap_or_default();
            if err.contains("Video unavailable") || err.contains("unavailable") {
                println!("Skipping test due to transient YouTube unavailable: {}", err);
                return;
            }
            panic!("Transcription failed: {:?}", err);
        }

        let output: Value = serde_json::from_str(&res.output).unwrap();
        assert!(output["segments"].is_array(), "Expected segments in JSON output");
        assert!(!output["transcript"].as_str().unwrap_or("").is_empty());
    }

    #[tokio::test]
    async fn test_audio_transcribe_error_no_input() {
        let tool = AudioTranscribeTool;
        let result = tool.execute(json!({})).await;

        assert!(result.is_err(), "Expected Err on missing input");
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("Missing 'input'") || err_msg.contains("url") || err_msg.contains("input"), 
                "Error message did not mention missing input: {}", err_msg);
    }
}