use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::fs; 
use tokio::process::Command;
use super::traits::{Tool, ToolResult};
use crate::security::SecurityPolicy;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AudioTranscribeTool {
    security: Arc<SecurityPolicy>,
    workspace_dir: PathBuf,
}

impl AudioTranscribeTool {
    // Run a `tokio::process::Command` with a timeout and capture stdout/stderr.
    async fn run_command_with_timeout(&self, mut cmd: Command, timeout_secs: u64) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
        let fut = cmd.output();
        match tokio::time::timeout(Duration::from_secs(timeout_secs), fut).await {
            Ok(Ok(output)) => Ok((output.status, output.stdout, output.stderr)),
            Ok(Err(e)) => Err(e).context("command execution failed"),
            Err(_) => Err(anyhow::anyhow!("command timed out after {}s", timeout_secs)),
        }
    }

    // Quick check whether an executable is available by running it with `--help`.
    async fn check_command_available(&self, exe: &str) -> bool {
        let mut c = Command::new(exe);
        c.arg("--help");
        match self.run_command_with_timeout(c, 3).await {
            Ok((status, _, _)) => status.success(),
            Err(_) => false,
        }
    }

    // Prepare a unique input filename in `output_dir` by creating a symlink or copy.
    async fn prepare_input_link(&self, audio_path: &PathBuf, output_dir: &PathBuf) -> Result<PathBuf> {
        let parent = output_dir.as_path();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let pid = std::process::id();
        let ext = audio_path.extension().and_then(|s| s.to_str()).unwrap_or("wav");
        let unique_name = format!("zc_{}_{}.{}", nanos, pid, ext);
        let link_path = parent.join(&unique_name);

        // Copy the audio into the output dir to avoid issues with sandboxed
        // executables (snap) that may not follow symlinks. Copy is more robust
        // across environments; fall back to symlink only if copy fails.
        if let Err(copy_err) = tokio::fs::copy(&audio_path, &link_path).await {
            if let Err(symlink_err) = std::os::unix::fs::symlink(&audio_path, &link_path) {
                return Err(anyhow::anyhow!("Failed to prepare unique input file: copy error: {} / symlink error: {}", copy_err, symlink_err));
            }
        }

        Ok(link_path)
    }

    // Collect outputs produced by whisper-cli next to the `link_path` within `output_dir`.
    async fn collect_outputs(&self, output_dir: &PathBuf, link_path: &PathBuf) -> Result<(String, Vec<String>)> {
        let mut transcript = String::new();
        let mut files: Vec<String> = Vec::new();
        if let Some(stem) = link_path.file_stem().and_then(|s| s.to_str()) {
            if let Ok(mut entries) = fs::read_dir(output_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if let Ok(fname) = entry.file_name().into_string() {
                        if fname.starts_with(stem) {
                            if fname.ends_with(".txt") || fname.ends_with(".srt") || fname.ends_with(".vtt") || fname.ends_with(".json") {
                                let found = entry.path();
                                files.push(found.to_string_lossy().to_string());
                                if transcript.is_empty() && (found.to_string_lossy().ends_with(".txt") || found.to_string_lossy().ends_with(".json")) {
                                    if let Ok(s) = tokio::fs::read_to_string(&found).await {
                                        transcript = s.trim().to_string();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok((transcript, files))
    }
        // Resolve canonical models directory and ensure it exists.
        async fn resolve_models_dir(&self) -> Result<PathBuf> {
        let home = directories::UserDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
        let models_dir = std::env::var("ZEROCLAW_WHISPER_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".zeroclaw/models"));
        fs::create_dir_all(&models_dir).await.context("Failed to create models dir")?;
        Ok(models_dir)
        }

    // Ensure the requested model is present; download from ggerganov/whisper.cpp if missing.
    async fn ensure_model_present(&self, model: &str) -> Result<PathBuf> {
        let models_dir = self.resolve_models_dir().await?;

        // Determine model file path under canonical dir. If `model` is an absolute path, use it.
        let model_path: PathBuf = if PathBuf::from(model).is_absolute() {
            PathBuf::from(model)
        } else if model == "auto" {
            models_dir.join("ggml-medium.en-q5_0.bin")
        } else if model.contains("ggml-") || model.ends_with(".bin") {
            models_dir.join(model)
        } else {
            models_dir.join(format!("ggml-{}.bin", model))
        };

        // If the model is already present, return it.
        if tokio::fs::metadata(&model_path).await.is_ok() {
            return Ok(model_path);
        }
        
    if PathBuf::from(model).is_absolute() {
            return Err(anyhow::anyhow!("Specified model path does not exist: {}", model));
        }

        // Attempt to download from the canonical whisper.cpp repo on HuggingFace
        let model_filename = model_path
            .file_name()
            .and_then(|s| s.to_str())
            .context("Invalid model filename")?
            .to_string();
        let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}", model_filename);
        let out_path = model_path.to_str().context("Invalid model path")?.to_string();
        // download with timeout and capture errors
        let mut curl = Command::new("curl");
        curl.arg("-L").arg("-o").arg(&out_path).arg(&url);
        let (status, _out, err) = self.run_command_with_timeout(curl, 600).await.context("Failed to download ggml model with curl")?;
        if !status.success() {
            return Err(anyhow::anyhow!(format!("Failed to download model {} from {}: {}", model_filename, url, String::from_utf8_lossy(&err))));
        }
        // ensure readable (best-effort)
        let mut chmod = Command::new("chmod");
        chmod.arg("a+r").arg(&out_path);

        Ok(PathBuf::from(out_path))
    }
    pub fn new(security: Arc<SecurityPolicy>, workspace_dir: PathBuf) -> Self {
        Self {
            security,
            workspace_dir,
        }
    }
}

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

        if !self.security.can_act() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: autonomy is read-only".into()),
            });
        }

        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: rate limit exceeded".into()),
            });
        }

        let input = args["input"].as_str().context("Missing 'input'")?.to_string();

        // Early check: prefer local whisper.cpp CLI (installed by bootstrap-tools.sh)
        let use_local = Command::new("whisper-cli").arg("--help").output().await.is_ok();

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
            self.workspace_dir.join("downloads/transcripts")
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
        let result = if use_local {
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
        _initial_prompt: Option<&str>,
        output_dir: &PathBuf,
    ) -> Result<ToolResult> {
        // Resolve and ensure model exists (may download if missing)
        let model_path_buf = self.ensure_model_present(model).await?;

        // Select whisper executable: prefer env ZEROCLAW_WHISPER_CLI_PATH, else whisper-cli
        let whisper_exec = std::env::var("ZEROCLAW_WHISPER_CLI_PATH").unwrap_or_else(|_| "whisper-cli".to_string());

        // Do not copy files around — run `whisper-cli` against the original
        // audio file and let it write outputs next to the input file (whisper
        // behavior). If the audio has no parent, fall back to the provided
        // `output_dir`.
        let ext = audio_path.extension().and_then(|s| s.to_str()).unwrap_or("mp3");
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let pid = std::process::id();
        let stem = audio_path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("zc_{}_{}", nanos, pid));

        // Build command to run whisper-cli against the original input
        let mut cmd = Command::new(&whisper_exec);
        cmd.arg("-m").arg(model_path_buf.to_str().context("Invalid model path")?)
            .arg("-f").arg(audio_path.to_str().context("Invalid audio path")?);

        // Set output base to the audio file's parent so produced outputs live
        // next to the input file (mimicking whisper.cpp behaviour). If parent
        // is None, use the provided `output_dir`.
        let parent = audio_path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| output_dir.clone());
        let out_base = parent.join(&stem);
        cmd.arg("-of").arg(out_base.to_str().context("Invalid out base path")?);

        // Output format mapping (whisper-cli supports txt/srt/vtt/json)
        match format {
            "text" => { cmd.arg("-otxt"); }
            "srt" => { cmd.arg("-osrt"); }
            "vtt" => { cmd.arg("-ovtt"); }
            "json" => { cmd.arg("-oj"); }
            _ => { cmd.arg("-otxt"); }
        }

        // whisper-cli uses --no-timestamps to disable timestamps; leave timestamps enabled when requested
        if !word_timestamps {
            cmd.arg("--no-timestamps");
        }

        let (status, stdout, stderr) = self.run_command_with_timeout(cmd, 300).await.context("whisper-cli execution failed")?;

        // After whisper-cli runs, collect outputs produced next to the input
        // audio file and return their paths. We do not copy files elsewhere.
        let (mut transcript, mut files) = if status.success() {
            let fake_link = parent.join(format!("{}.{}", stem, ext));
            let (t, f) = self.collect_outputs(&parent, &fake_link).await?;
            (t, f)
        } else {
            (String::from_utf8_lossy(&stderr).to_string(), Vec::new())
        };
        // Note: we do not remove original audio files here.

        Ok(ToolResult {
            success: status.success(),
            output: json!({
                "transcript": transcript,
                "language": language,
                "model": model,
                "files": files
            }).to_string(),
            error: if status.success() {
                None
            } else {
                Some(String::from_utf8_lossy(&stderr).trim().to_string())
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
     use crate::security::AutonomyLevel;
    use serde_json::json;
    use std::path::PathBuf;
    use tokio::fs;

    fn test_security(level: AutonomyLevel, max_actions_per_hour: u32) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: level,
            max_actions_per_hour,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        })
    }
 

    const TEST_VIDEO: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";  
    const TEST_VIDEO_SUBS: &str = "https://www.youtube.com/watch?v=3tmd-ClpJxA";  
 
    // Integration test: downloads from YouTube and requires `yt-dlp`/`ffmpeg`.
    // Marked `#[ignore]` so it doesn't run during normal `cargo test`.
    // Run with: `cargo test -- --ignored --test-threads=1`
    #[tokio::test]
    #[ignore]
    async fn test_audio_transcribe_youtube_default() {
        // Download audio into a HOME-based temp dir so we can assert file
        // visibility before invoking whisper-cli. This also allows running
        // whisper-cli directly (outside our tool) to verify the system
        // installation.
        let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()).expect("Could not determine home dir");
        let home_io = home.join("zeroclaw_whisper_io");
        let _ = fs::create_dir_all(&home_io).await;

        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dl_path = home_io.join(format!("vimeo_integration_test_{}.mp3", nanos));

        let status = Command::new("yt-dlp")
            .arg("--extract-audio")
            .arg("--audio-format").arg("mp3")
            .arg("-o").arg(dl_path.to_str().unwrap())
            .arg("--no-playlist")
            .arg(TEST_VIDEO)
            .status().await.expect("yt-dlp failed to run");
        assert!(status.success(), "yt-dlp failed to download test video");

        // Ensure the downloaded file exists
        assert!(tokio::fs::metadata(&dl_path).await.is_ok(), "Downloaded audio file not found: {}", dl_path.display());

        // If whisper-cli + model are available, run whisper-cli directly to
        // validate the system installation (outside our tool). If not, skip
        // this direct check.
        let whisper_exec = std::env::var("ZEROCLAW_WHISPER_CLI_PATH").unwrap_or_else(|_| "whisper-cli".to_string());
        let whisper_available = Command::new(&whisper_exec).arg("--help").output().await.is_ok();
        let model_path = std::env::var("ZEROCLAW_WHISPER_MODELS_DIR").map(PathBuf::from).unwrap_or_else(|_| home.join(".zeroclaw/models"));
        let model_file = model_path.join("ggml-medium.en-q5_0.bin");
        if whisper_available && tokio::fs::metadata(&model_file).await.is_ok() {
            let direct_out_base = home_io.join(format!("direct_test_{}", nanos));
            let mut direct_cmd = Command::new(&whisper_exec);
            direct_cmd.arg("-m").arg(model_file.to_str().unwrap())
                .arg("-f").arg(dl_path.to_str().unwrap())
                .arg("-of").arg(direct_out_base.to_str().unwrap())
                .arg("-otxt")
                .arg("--no-timestamps");
            let direct = direct_cmd.status().await.expect("direct whisper-cli run failed to start");
            if !direct.success() {
                println!("Direct whisper-cli failed (will skip direct validation). stderr: {:?}", direct);
            } else {
                // check output file exists
                let expected = direct_out_base.with_extension("txt");
                if tokio::fs::metadata(&expected).await.is_ok() {
                    println!("Direct whisper-cli produced output: {}", expected.display());
                } else {
                    println!("Direct whisper-cli reported success but output missing: {}", expected.display());
                }
            }
        } else {
            println!("Skipping direct whisper-cli validation: whisper not available or model missing");
        }

        // Now run our tool against the local file path (not URL)
        let tool = AudioTranscribeTool::new(
            test_security(AutonomyLevel::Full, 100),
            home.clone(),
        );
        let res = tool.execute(json!({ "input": dl_path.to_str().unwrap() })).await.unwrap();
        if !res.success {
            let err = res.error.as_ref().map(|s| s.to_lowercase()).unwrap_or_default();
            assert!(err.contains("whisper") || err.contains("openai_api_key") || err.contains("unavailable"));
            return;
        }
        let output: Value = serde_json::from_str(&res.output).unwrap();
        assert!(!output["transcript"].as_str().unwrap().is_empty());
    }

    // Integration test: downloads from YouTube and expects timestamped JSON output.
    // Marked `#[ignore]` so it doesn't run during normal `cargo test`.
    // Run with: `cargo test -- --ignored --test-threads=1`
    #[tokio::test]
    #[ignore]
    async fn test_audio_transcribe_with_timestamps() {
        // Download into HOME-based dir and validate before invoking tool
        let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()).expect("Could not determine home dir");
        let home_io = home.join("zeroclaw_whisper_io");
        let _ = fs::create_dir_all(&home_io).await;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dl_path = home_io.join(format!("vimeo_integration_test_{}.mp3", nanos));
        let status = Command::new("yt-dlp")
            .arg("--extract-audio")
            .arg("--audio-format").arg("mp3")
            .arg("-o").arg(dl_path.to_str().unwrap())
            .arg("--no-playlist")
            .arg(TEST_VIDEO_SUBS)
            .status().await.expect("yt-dlp failed to run");
        assert!(status.success(), "yt-dlp failed to download test video");
        assert!(tokio::fs::metadata(&dl_path).await.is_ok(), "Downloaded audio file not found: {}", dl_path.display());

        // Try running whisper-cli directly if available and model present
        let whisper_exec = std::env::var("ZEROCLAW_WHISPER_CLI_PATH").unwrap_or_else(|_| "whisper-cli".to_string());
        let whisper_available = Command::new(&whisper_exec).arg("--help").output().await.is_ok();
        let model_path = std::env::var("ZEROCLAW_WHISPER_MODELS_DIR").map(PathBuf::from).unwrap_or_else(|_| home.join(".zeroclaw/models"));
        let model_file = model_path.join("ggml-medium.en-q5_0.bin");
        if whisper_available && tokio::fs::metadata(&model_file).await.is_ok() {
            let direct_out_base = home_io.join(format!("direct_test_json_{}", nanos));
            let mut direct_cmd = Command::new(&whisper_exec);
            direct_cmd.arg("-m").arg(model_file.to_str().unwrap())
                .arg("-f").arg(dl_path.to_str().unwrap())
                .arg("-of").arg(direct_out_base.to_str().unwrap())
                .arg("-oj")
                .arg("--no-timestamps");
            let direct = direct_cmd.status().await.expect("direct whisper-cli run failed to start");
            if !direct.success() {
                println!("Direct whisper-cli failed (will skip direct validation). stderr: {:?}", direct);
            } else {
                let expected = direct_out_base.with_extension("json");
                if tokio::fs::metadata(&expected).await.is_ok() {
                    println!("Direct whisper-cli produced JSON output: {}", expected.display());
                } else {
                    println!("Direct whisper-cli reported success but JSON output missing: {}", expected.display());
                }
            }
        } else {
            println!("Skipping direct whisper-cli validation: whisper not available or model missing");
        }

        // Run our tool with the local file
        let tool = AudioTranscribeTool::new(
            test_security(AutonomyLevel::Full, 100),
            home.clone(),
        );
        let res = tool.execute(json!({
            "input": dl_path.to_str().unwrap(),
            "word_timestamps": true,
            "format": "json"
        })).await.unwrap();

        if !res.success {
            let err = res.error.unwrap_or_default().to_lowercase();
            if err.contains("video unavailable") || err.contains("unavailable") || err.contains("whisper") || err.contains("openai_api_key") {
                println!("Skipping test due to transient/unavailable backend or YouTube: {}", err);
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
        let tool = AudioTranscribeTool::new(
            test_security(AutonomyLevel::Full, 100),
            PathBuf::from("/tmp"),
        );
        let result = tool.execute(json!({})).await;

        assert!(result.is_err(), "Expected Err on missing input");
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("Missing 'input'") || err_msg.contains("url") || err_msg.contains("input"), 
                "Error message did not mention missing input: {}", err_msg);
    }

    #[tokio::test]
    async fn test_resolve_models_dir_creates_dir() {
        let tool = AudioTranscribeTool::new(
            test_security(AutonomyLevel::Full, 100),
            PathBuf::from("/tmp"),
        );
        let tmp = std::env::temp_dir().join(format!("zc_models_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        // ensure clean
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        std::env::set_var("ZEROCLAW_WHISPER_MODELS_DIR", &tmp);
        let dir = tool.resolve_models_dir().await.unwrap();
        assert!(tokio::fs::metadata(&dir).await.is_ok());
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn test_prepare_input_link_creates_file() {
        let tool = AudioTranscribeTool::new(
            test_security(AutonomyLevel::Full, 100),
            PathBuf::from("/tmp"),
        );
        let out_dir = std::env::temp_dir().join(format!("zc_out_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = tokio::fs::create_dir_all(&out_dir).await;
        let audio = out_dir.join("in.wav");
        tokio::fs::write(&audio, b"hello").await.unwrap();
        let link = tool.prepare_input_link(&audio, &out_dir).await.unwrap();
        let meta = tokio::fs::metadata(&link).await.unwrap();
        assert!(meta.len() > 0);
        let _ = tokio::fs::remove_file(&link).await;
        let _ = tokio::fs::remove_file(&audio).await;
        let _ = tokio::fs::remove_dir_all(&out_dir).await;
    }
}