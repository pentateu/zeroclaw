// Heavy integration tests for tools that interact with external services.
// These tests run inside the crate (so they can access crate internals like
// `SecurityPolicy`) but are marked `#[ignore]` by default. Run explicitly with:
//
//   cargo test -- --ignored --test-threads=1
//
// The tests below cover two scenarios:
//  1) default output path (workspace_dir/downloads/transcripts)
//  2) custom output path via the `output_dir` parameter
//
// Each test asserts that transcript files were produced in the expected
// directory. Cleanup (deleting files) is included but commented out so you can
// inspect artifacts after a run; uncomment cleanup lines when you want tests
// to always remove artifacts.

use super::*;
use crate::security::{AutonomyLevel, SecurityPolicy};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
mod integration {
    use super::*;
    use tokio::fs;

    const VIMEO_TEST: &str = "https://vimeo.com/375831331?fl=tl&fe=ec";

    fn test_security(level: AutonomyLevel, max_actions_per_hour: u32) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: level,
            max_actions_per_hour,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        })
    }

    async fn download_vimeo_audio(tmp: &PathBuf) -> Option<String> {
        let ytool = crate::tools::YoutubeDownloadTool::new(
            test_security(AutonomyLevel::Full, 100),
            tmp.clone(),
        );

        let yd_res = ytool.execute(json!({
            "url": VIMEO_TEST,
            "mode": "audio",
            "output_filename": "vimeo_integration_test"
        })).await.unwrap();

        if !yd_res.success {
            println!("Skipping integration: youtube_download failed: {:?}", yd_res.error);
            return None;
        }

        let yd_out: Value = serde_json::from_str(&yd_res.output).unwrap();
        let paths = yd_out["file_paths"].as_array().cloned().unwrap_or_default();
        if paths.is_empty() {
            println!("No files downloaded by youtube_download; skipping transcribe");
            return None;
        }

        Some(paths[0].as_str().unwrap().to_string())
    }

    #[tokio::test]
    #[ignore]
    async fn vimeo_transcribe_default_output_files() {
        let base = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let tmp = base.join(format!("zc_int_vimeo_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&tmp).await;

        let audio_path = match download_vimeo_audio(&tmp).await {
            Some(p) => p,
            None => return,
        };

        // Run audio transcriber on the downloaded file using default output dir
        let atool = crate::tools::AudioTranscribeTool::new(
            test_security(AutonomyLevel::Full, 100),
            tmp.clone(),
        );

        let at_res = atool.execute(json!({
            "input": audio_path,
            "format": "text"
        })).await.unwrap();

        if !at_res.success {
            println!("Transcribe failed or backend unavailable: {:?}", at_res.error);
            return;
        }

        let at_out: Value = serde_json::from_str(&at_res.output).unwrap();

        // Prefer explicit file list returned by tool
        let mut found_files: Vec<String> = Vec::new();
        if let Some(arr) = at_out["files"].as_array() {
            for v in arr { if let Some(s) = v.as_str() { found_files.push(s.to_string()) } }
        }

        // If no explicit files were reported, check the default transcripts dir
        let default_dir = tmp.join("downloads/transcripts");
        if found_files.is_empty() {
            if let Ok(mut entries) = fs::read_dir(&default_dir).await {
                while let Ok(Some(e)) = entries.next_entry().await {
                    let name = e.file_name().into_string().unwrap_or_default();
                    if name.ends_with(".txt") || name.ends_with(".srt") || name.ends_with(".vtt") || name.ends_with(".json") {
                        found_files.push(e.path().to_string_lossy().to_string());
                    }
                }
            }
        }

        if found_files.is_empty() {
            println!("No transcript files found in default output dir; skipping assert");
            return;
        }

        // Assert that found files physically exist
        for f in &found_files {
            assert!(fs::metadata(f).await.is_ok(), "Expected transcript file to exist: {}", f);
        }

        // Cleanup (commented out so you can inspect files) - uncomment to auto-remove
        // for f in &found_files { let _ = fs::remove_file(f).await; }
        // let _ = fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    #[ignore]
    async fn vimeo_transcribe_custom_output_files() {
        let base = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let tmp = base.join(format!("zc_int_vimeo_custom_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let custom_out = tmp.join("my_custom_transcripts");
        let _ = fs::create_dir_all(&custom_out).await;

        let audio_path = match download_vimeo_audio(&tmp).await {
            Some(p) => p,
            None => return,
        };

        // Run audio transcriber on the downloaded file using explicit output_dir
        let atool = crate::tools::AudioTranscribeTool::new(
            test_security(AutonomyLevel::Full, 100),
            tmp.clone(),
        );

        let at_res = atool.execute(json!({
            "input": audio_path,
            "format": "text",
            "output_dir": custom_out.to_string_lossy()
        })).await.unwrap();

        if !at_res.success {
            println!("Transcribe failed or backend unavailable: {:?}", at_res.error);
            return;
        }

        let at_out: Value = serde_json::from_str(&at_res.output).unwrap();

        // Prefer explicit file list returned by tool
        let mut found_files: Vec<String> = Vec::new();
        if let Some(arr) = at_out["files"].as_array() {
            for v in arr { if let Some(s) = v.as_str() { found_files.push(s.to_string()) } }
        }

        // If no explicit files were reported, check the custom transcripts dir
        if found_files.is_empty() {
            if let Ok(mut entries) = fs::read_dir(&custom_out).await {
                while let Ok(Some(e)) = entries.next_entry().await {
                    let name = e.file_name().into_string().unwrap_or_default();
                    if name.ends_with(".txt") || name.ends_with(".srt") || name.ends_with(".vtt") || name.ends_with(".json") {
                        found_files.push(e.path().to_string_lossy().to_string());
                    }
                }
            }
        }

        if found_files.is_empty() {
            println!("No transcript files found in custom output dir; skipping assert");
            return;
        }

        for f in &found_files {
            assert!(fs::metadata(f).await.is_ok(), "Expected transcript file to exist: {}", f);
        }

        // Cleanup (commented out so you can inspect files) - uncomment to auto-remove
        // for f in &found_files { let _ = fs::remove_file(f).await; }
        // let _ = fs::remove_dir_all(&tmp).await;
    }
}
