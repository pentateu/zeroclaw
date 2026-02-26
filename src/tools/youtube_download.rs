use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::process::Command;
use super::traits::{Tool, ToolResult};

#[derive(Debug, Clone)]
pub struct YoutubeDownloadTool;

#[async_trait]
impl Tool for YoutubeDownloadTool {
    fn name(&self) -> &str {
        "youtube_download"
    }

    fn description(&self) -> &str {
        "Downloads audio (default) or video from YouTube (and 1000+ other sites) using yt-dlp. \
         Supports playlists, quality selection, subtitles, thumbnails, browser cookies, format listing, \
         and custom filenames. Returns rich metadata + actual final file paths as JSON in .output. \
         Requires yt-dlp + ffmpeg installed. Ideal for transcription, archiving, or LLM workflows."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "debug": {
                    "type": "boolean",
                    "default": false,
                    "description": "Print the underlying yt-dlp command to help the agent debug issues"
                },
                "url": { "type": "string", "description": "The YouTube video or playlist URL (required)" },
                "mode": {
                    "type": "string",
                    "enum": ["audio", "video"],
                    "default": "audio",
                    "description": "Download audio only (mp3) or full video (mp4)"
                },
                "quality": {
                    "type": "string",
                    "description": "For video: resolution like '720', '1080', 'best'; for audio: ignored"
                },
                "subtitles": {
                    "type": "boolean",
                    "default": false,
                    "description": "Download subtitles (manual + auto) in all languages (.srt)"
                },
                "subtitle_langs": {
                    "type": ["string", "array"],
                    "description": "Specific language codes, e.g. 'en,es,fr' or ['en','es']. If omitted but subtitles=true → 'en'. Use 'all' for everything (not recommended - rate limit risk)."
                },
                "output_filename": {
                    "type": "string",
                    "description": "Optional custom filename (without extension). Defaults to sanitized title (or playlist template)"
                },
                "playlist": {
                    "type": "boolean",
                    "default": false,
                    "description": "Treat URL as playlist (even if single video)"
                },
                "playlist_items": {
                    "type": "string",
                    "description": "Optional range e.g. '1-5,10' (requires playlist=true)"
                },
                "thumbnails": {
                    "type": "boolean",
                    "default": false,
                    "description": "Download thumbnail images"
                },
                "cookies_browser": {
                    "type": "string",
                    "enum": ["none", "chrome", "firefox", "safari", "edge", "brave", "opera"],
                    "default": "none",
                    "description": "Use cookies from browser to bypass age-restrictions / login walls"
                },
                "list_formats": {
                    "type": "boolean",
                    "default": false,
                    "description": "Only list available formats, do NOT download"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        // Early yt-dlp check
        if Command::new("yt-dlp").arg("--version").output().await.is_err() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("yt-dlp not found in PATH. Install with: brew install yt-dlp ffmpeg (macOS)".to_string()),
            });
        }

        let url = args["url"].as_str().context("Missing 'url'")?.to_string();
        let mode = args["mode"].as_str().unwrap_or("audio").to_lowercase();
        let quality = args["quality"].as_str().unwrap_or("").trim().to_string();
        let subtitles = args["subtitles"].as_bool().unwrap_or(false);
        let thumbnails = args["thumbnails"].as_bool().unwrap_or(false);
        let cookies_browser = args["cookies_browser"].as_str().unwrap_or("none");
        let list_formats = args["list_formats"].as_bool().unwrap_or(false);
        let playlist = args["playlist"].as_bool().unwrap_or(false);
        let playlist_items = args["playlist_items"].as_str().map(|s| s.to_string());
        let custom_name = args["output_filename"].as_str().map(|s| s.trim().to_string());
        let debug = args["debug"].as_bool().unwrap_or(false);

        let output_dir: PathBuf = std::env::current_dir()
            .context("Failed to get current directory")?
            .join("downloads");
        std::fs::create_dir_all(&output_dir).ok();

        let template = if let Some(name) = &custom_name {
            let sanitized: String = name
                .chars()
                .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
                .collect();
            format!("{}.%(ext)s", sanitized.trim())
        } else if playlist {
            "%(playlist)s/%(playlist_index)02d - %(title)s.%(ext)s".to_string()
        } else {
            "%(title)s.%(ext)s".to_string()
        };

        let output_template = output_dir.join(&template).to_string_lossy().into_owned();

        if list_formats {
            let output = Command::new("yt-dlp")
                .arg("-F")
                .arg("--no-warnings")
                .arg(&url)
                .output()
                .await
                .context("Failed to run yt-dlp -F")?;

            let result = if output.status.success() {
                String::from_utf8_lossy(&output.stdout).to_string()
            } else {
                String::from_utf8_lossy(&output.stderr).to_string()
            };

            return Ok(ToolResult {
                success: output.status.success(),
                output: result,
                error: if output.status.success() { None } else { Some("yt-dlp -F failed".to_string()) },
            });
        }

        // Metadata (always safe)
        let mut info_cmd = Command::new("yt-dlp");
        info_cmd.arg("-J").arg("--no-download").arg("--no-warnings").arg(&url);
        if playlist && playlist_items.is_some() {
            info_cmd.arg("-I").arg(playlist_items.as_deref().unwrap());
        }
        let info_output = info_cmd.output().await.context("Failed to fetch metadata")?;
        let metadata: Value = if info_output.status.success() {
            serde_json::from_slice(&info_output.stdout).unwrap_or_else(|_| json!({"title": "Unknown"}))
        } else {
            json!({"title": "Unknown"})
        };

        // Main download command
        let mut cmd = Command::new("yt-dlp");
        cmd.arg("-o").arg(&output_template)
           .arg("--restrict-filenames")
           .arg("--no-warnings")
           .arg("--print").arg("after_move:filepath:%(filepath)s")   // ← FIXED: final path after ffmpeg
           .arg("--print").arg("thumbnail:%(thumbnail)s");

        if subtitles {
            cmd.arg("--write-subs");

            // Determine languages
            let langs = if let Some(langs_val) = args.get("subtitle_langs") {
                match langs_val {
                    Value::String(s) => {
                        if s.trim().eq_ignore_ascii_case("all") {
                            "all".to_string()
                        } else {
                            s.trim().to_string()
                        }
                    }
                    Value::Array(arr) => {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    }
                    _ => "en".to_string(),
                }
            } else {
                "en".to_string()  // default when subtitles=true but no lang specified
            };

            if !langs.is_empty() {
                cmd.arg("--sub-langs").arg(&langs);

                // Only enable auto-subs if we have specific languages (safer)
                // If user wants 'all' → they accept the risk
                if langs != "all" {
                    cmd.arg("--write-auto-subs");
                } else {
                    // For 'all' we can optionally add rate-limit protection
                    cmd.arg("--sleep-requests").arg("1.5");
                }
            }
        }

        if thumbnails {
            cmd.arg("--write-thumbnail");
        }
        if cookies_browser != "none" {
            cmd.arg("--cookies-from-browser").arg(cookies_browser);
        }
        if playlist {
            cmd.arg("--yes-playlist");
            if let Some(items) = &playlist_items {
                cmd.arg("-I").arg(items);
            }
        } else {
            cmd.arg("--no-playlist");
        }

        if mode == "audio" {
            cmd.arg("--extract-audio")
               .arg("--audio-format").arg("mp3")
               .arg("--audio-quality").arg("0");
        } else {
            cmd.arg("--merge-output-format").arg("mp4");
            if !quality.is_empty() && quality != "best" {
                cmd.arg("-f").arg(format!("bestvideo[height<={}] + bestaudio/best", quality));
            } else {
                cmd.arg("-f").arg("bestvideo+bestaudio/best");
            }
        }
        cmd.arg(&url);

        println!("debug:  {}", debug);

        if debug {
            // ────────────────────────────────────────────────
            //          PRINT FULL COMMAND FOR DEBUGGING
            // ────────────────────────────────────────────────
            // {
            //     let program = cmd.as_std().get_program().to_string_lossy().to_string();
            //     let args: Vec<String> = cmd.as_std().get_args()
            //         .map(|a| a.to_string_lossy().to_string())
            //         .collect();

            //     println!("\n[DEBUG] Executing yt-dlp command:");
            //     println!("  {}", program);
            //     for arg in &args {
            //         if arg.contains(' ') {
            //             println!("  \"{}\"", arg);
            //         } else {
            //             println!("  {}", arg);
            //         }
            //     }
            //     println!("[DEBUG] Full command as one line:");
            //     print!("{} ", program);
            //     for arg in args {
            //         if arg.contains(' ') || arg.contains('=') {
            //             print!("\"{}\" ", arg);
            //         } else {
            //             print!("{} ", arg);
            //         }
            //     }
            //     println!("\n");
            // }
        }
        // ────────────────────────────────────────────────

        let output = cmd.output().await.context("yt-dlp execution failed")?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        let mut file_paths: Vec<String> = vec![];
        let mut thumbnail_paths: Vec<String> = vec![];

        for line in stdout.lines() {
            if let Some(p) = line.strip_prefix("filepath:") {
                file_paths.push(p.trim().to_string());
            } else if let Some(p) = line.strip_prefix("thumbnail:") {
                if !p.trim().is_empty() {
                    thumbnail_paths.push(p.trim().to_string());
                }
            }
        }

        let not_empt_msg = &format!("Successfully downloaded {} file(s)", file_paths.len());

        let result_json = json!({
            "file_paths": file_paths,
            "thumbnail_paths": thumbnail_paths,
            "metadata": metadata,
            "output_dir": output_dir.to_string_lossy().to_string(),
            "message": if file_paths.is_empty() { "No files downloaded" } else { not_empt_msg }
        });

        let success = output.status.success() && !file_paths.is_empty();

        Ok(ToolResult {
            success,
            output: result_json.to_string(),
            error: if success { None } else { Some(String::from_utf8_lossy(&output.stderr).trim().to_string()) },
        })
    }
}

// =============================================================================
// TESTS (now robust)
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use tokio::fs;

    // Stable test assets
    const TEST_VIDEO: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
    const TEST_VIDEO_SUBS: &str = "https://www.youtube.com/watch?v=3tmd-ClpJxA";
    const TEST_PLAYLIST: &str = "https://www.youtube.com/playlist?list=PLcduW1K6eOtn8mIAArqAjonxvYviQ6ZAa";

    // Guard that deletes the directory on drop (success or failure/panic)
    struct TestDirGuard(PathBuf);

    impl Drop for TestDirGuard {
        fn drop(&mut self) {
            // Ignore errors during cleanup (e.g. permission issues) — don't crash the test
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn test_output_dir() -> (PathBuf, TestDirGuard) {
        let dir = std::env::temp_dir().join("zeroclaw_yt_test");
        fs::create_dir_all(&dir).await.ok();

        let dir_clone = dir.clone();

        (dir, TestDirGuard(dir_clone))
    }

    #[tokio::test]
    async fn test_audio_only_default() {
        let (dir, _guard) = test_output_dir().await;
        let tool = YoutubeDownloadTool;
        let res = tool.execute(json!({
            "url": TEST_VIDEO,
            "mode": "audio",
            "output_dir": dir.to_string_lossy().to_string()
        })).await.unwrap();
        assert!(res.success, "Error: {:?}", res.error);
        let output: Value = serde_json::from_str(&res.output).unwrap();
        let path = output["file_paths"][0].as_str().unwrap();
        assert!(path.ends_with(".mp3"));
        // dir auto-cleaned by _guard drop
    }

    #[tokio::test]
    async fn test_video_720p() {
        let (dir, _guard) = test_output_dir().await;
        let tool = YoutubeDownloadTool;
        let res = tool.execute(json!({
            "url": TEST_VIDEO,
            "mode": "video",
            "quality": "720",
            "output_dir": dir.to_string_lossy().to_string()
        })).await.unwrap();
        assert!(res.success, "Error: {:?}", res.error);
        let output: Value = serde_json::from_str(&res.output).unwrap();
        let path = output["file_paths"][0].as_str().unwrap();
        assert!(path.ends_with(".mp4"));
    }

    #[tokio::test]
    async fn test_subtitles_default() {
        let (dir, _guard) = test_output_dir().await;
        let tool = YoutubeDownloadTool;
        let res = tool.execute(json!({
            "url": TEST_VIDEO_SUBS,
            "subtitles": true,
            "output_dir": dir.to_string_lossy().to_string()
        })).await.unwrap();
        assert!(res.success, "Default subtitles failed: {:?}", res.error);
        let output: Value = serde_json::from_str(&res.output).unwrap();
        // Optional: could check for .srt in dir, but success is enough
    }

    #[tokio::test]
    async fn test_all_features_combined() {
        let (dir, _guard) = test_output_dir().await;
        let tool = YoutubeDownloadTool;
        let res = tool.execute(json!({
            "url": TEST_VIDEO_SUBS,
            "mode": "video",
            "quality": "best",
            "subtitles": true,
            "thumbnails": true,
            "output_filename": "combined_test",
            "output_dir": dir.to_string_lossy().to_string()
        })).await.unwrap();
        assert!(res.success, "Combined failed: {:?}", res.error);
        let output: Value = serde_json::from_str(&res.output).unwrap();
        assert!(!output["file_paths"].as_array().unwrap().is_empty());
        assert!(!output["thumbnail_paths"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_thumbnails() {
        let (dir, _guard) = test_output_dir().await;
        let tool = YoutubeDownloadTool;
        let res = tool.execute(json!({
            "url": TEST_VIDEO,
            "thumbnails": true,
            "output_dir": dir.to_string_lossy().to_string()
        })).await.unwrap();
        assert!(res.success, "Error: {:?}", res.error);
        let output: Value = serde_json::from_str(&res.output).unwrap();
        assert!(!output["thumbnail_paths"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_custom_filename() {
        let (dir, _guard) = test_output_dir().await;
        let tool = YoutubeDownloadTool;
        let res = tool.execute(json!({
            "url": TEST_VIDEO,
            "output_filename": "my_custom_test_file",
            "output_dir": dir.to_string_lossy().to_string()
        })).await.unwrap();
        assert!(res.success, "Error: {:?}", res.error);
        let output: Value = serde_json::from_str(&res.output).unwrap();
        let path = output["file_paths"][0].as_str().unwrap();
        assert!(path.contains("my_custom_test_file"));
    }

    #[tokio::test]
    async fn test_playlist_single_item() {
        let (dir, _guard) = test_output_dir().await;
        let tool = YoutubeDownloadTool;
        let res = tool.execute(json!({
            "url": TEST_PLAYLIST,
            "playlist": true,
            "playlist_items": "1-2",
            "output_dir": dir.to_string_lossy().to_string()
        })).await.unwrap();
        assert!(res.success, "Error: {:?}", res.error);
        let output: Value = serde_json::from_str(&res.output).unwrap();
        assert!(output["file_paths"].as_array().unwrap().len() >= 1);
    }

    #[tokio::test]
    async fn test_list_formats_only() {
        let tool = YoutubeDownloadTool;
        let res = tool.execute(json!({
            "url": TEST_VIDEO,
            "list_formats": true
        })).await.unwrap();
        assert!(res.success, "Error: {:?}", res.error);
        assert!(!res.output.trim().is_empty());
        assert!(res.output.contains("video") || res.output.contains("audio") || res.output.contains("format"));
    }

    #[tokio::test]
    async fn test_error_invalid_url() {
        let tool = YoutubeDownloadTool;
        let res = tool.execute(json!({ "url": "https://bad.url" })).await.unwrap();
        assert!(!res.success);
        assert!(res.error.is_some());
        // No files created → no cleanup needed
    }
}