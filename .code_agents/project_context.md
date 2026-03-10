## Project Context: ZeroClaw Video Knowledge Agent

**Project Name**: ZeroClaw  
**Overall Vision**: ZeroClaw is a lightweight, modular, fully local-first (or hybrid) runtime/infrastructure for building autonomous AI agents and workflows. It emphasizes speed, small footprint, easy swapping of models/tools/providers, and deployment flexibility (local laptop → server → cloud). The system is designed to run complex, multi-step agentic tasks without heavy external dependencies.

**Current Focus Area**: Video Knowledge & Search Agent  
We are building a powerful video understanding & retrieval system inside ZeroClaw. The goal is to turn large collections of video files (lectures, meetings, tutorials, podcasts with video, screencasts, etc.) into a searchable, queryable knowledge base — using AI to transcribe, understand, segment, and retrieve the most relevant moments/clips.

** Core Components Being Designed**:

**Ingestion Pipeline** 

**Video Downloader** V1 done- /home/rafael/Development/zeroclaw/src/tools/youtube_download.rs

- Takes raw video/audio files (any format)  

**audio  transcriber** V1 done /home/rafael/Development/zeroclaw/src/tools/audio_transcribe.rs

 Extracts audio → high-quality transcription (with timestamps)  

 **Ingestion Video Transcription** (`ingest_video_transcription.md`)  

   - Optional: speaker diarization, language detection, entity recognition  
   - Chunks transcript into searchable pieces  
   - Generates vector embeddings (for semantic search)  
   - Stores everything (transcripts, metadata, embeddings) in an efficient, local-friendly database setup  - in ZeroClaw's existing SQLite-based vector database setup
   - Designed to be resumable, idempotent, parallelizable, and cost-effective (especially important when running locally or on limited hardware)

 **Query & Retrieval Engine** (`query_video_clips.md`)  
   - Natural-language queries from the user/agent ("show me the part where they explain async Rust", "find all moments someone says 'zero claw'", "summarize discussions about privacy in the last 3 meetings")  
   - Searches across transcripts + embeddings → returns ranked video segments/clips  
   - Can return: exact timestamps, short preview clips (extracted on-the-fly or pre-generated), summaries, or full context  
   - Aims for fast response times even on large libraries (thousands of hours of video)

 **Clipper** once a topic is selected, and specific videos selected via prompt, we need a tool that will go and collect clips. The agent will download the video (if not alrady locally, usualy it wont be since we first just download audio) and then clipt the specific part (given the start and end timestamps of the clip) and save that in the current project/task folder. Alongside the clip we save an MD file with all the info about the that clip. the transcirption, key works, topics, the classifications (youtube red flags/cons or pros about this clip).

**Video Editor** once clips are saved we might want to assemble a final video by adding a intro clip, and a final clip.. and create a final video to be published on youtube. 

**Key Constraints & Design Principles** (as of March 2026):
- Prefer **local-first** execution when possible (Whisper.cpp, local LLMs/embeddings, embedded vector DB like Chroma/LanceDB/PGVector local)
- Graceful fallback/hybrid to cloud APIs (OpenAI Whisper, Grok API, etc.) when local quality/speed is insufficient
- Keep memory & disk usage reasonable on developer laptops (e.g. 16–32 GB RAM machines)
- No mandatory internet dependency for core loops (ingestion and querying should work offline after models are downloaded)
- Strong emphasis on **accuracy + timestamp precision** — wrong timestamps ruin the experience
- Privacy-first: raw video and transcripts never leave the user's machine unless explicitly configured
- Modular & swappable: easy to change transcription model, embedding model, vector store, clip extraction method
- Production-minded even in early stage: observability, error recovery, resumability, logging, metrics from day one

**Target Use Cases**:
- Personal video library search (YouTube downloads, Zoom recordings, course videos)
- Team knowledge base (recorded standups, customer calls, training sessions)
- Research / content creation (quickly find quotes, examples, explanations in long videos)
- Agentic workflows where other ZeroClaw agents need to "remember" or reference video content

**Non-Goals (for this phase)**:
- Real-time / live-stream transcription
- Multi-user / SaaS platform features
- Advanced computer vision (object detection, OCR on screen, face recognition) — focus stays on audio + transcript for now

Success looks like:
- Ingest 1 hour of typical meeting/lecture video in < 5–10 min wall-clock time on a decent laptop with GPU
- Query latency < 3–5 seconds for semantic search over ~100 hours of content
- Returned clips are accurate within ±5 seconds of the real moment
- System doesn't crash or corrupt state on bad files, interruptions, or out-of-memory situations